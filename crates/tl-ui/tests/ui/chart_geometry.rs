//! Chart geometry unit tests (tasklens-yrf.4, yrf.5 and yrf.14, ADR-009 rules 2 to 7; UI-PRF-007;
//! R-051).

use std::sync::Arc;

use gpui_kit::{Hsla, hsla};
use smallvec::SmallVec;
use tl_model::series::{Sample, SampleFlags, SeriesView};
use tl_ui::chart::geometry::{
    BuildInput, Pt, Run, Scratch, TimeMap, bucket_ns, build, build_cell, decimate, emit_run,
    finish_grid, is_stale, mean_samples, right_edge_ns, split_runs, stale_age_ns, window_start,
    y_of,
};
use tl_ui::chart::{ChartCache, ChartSeries, ChartSpec, PathSet, YScale};

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
        curved: false,
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

fn stack_build(
    series: &[(ChartSeries, Option<&SeriesView>)],
    map: TimeMap,
    now: u64,
    y_max: f32,
    cache: &mut ChartCache,
    scratch: &mut Scratch,
) {
    let input = BuildInput {
        series,
        map,
        plot_height: 100.0,
        y_min: 0.0,
        y_max,
        now_boot_ns: now,
        stack: true,
        curved: false,
    };
    build(&input, cache, scratch);
}

fn ys(set: &PathSet) -> impl Iterator<Item = f32> + '_ {
    set.path.vertices.iter().map(|v| v.xy_position.y.as_f32())
}

fn y_span(set: &PathSet) -> (f32, f32) {
    ys(set).fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)))
}

fn has_vertex(set: &PathSet, x: f32, y: f32) -> bool {
    set.path.vertices.iter().any(|v| {
        (v.xy_position.x.as_f32() - x).abs() < 0.01 && (v.xy_position.y.as_f32() - y).abs() < 0.01
    })
}

fn colour(i: usize) -> Hsla {
    hsla(i as f32 / 40.0, 1.0, 0.5, 1.0)
}

/// Stacked bands (tasklens-yrf.14, PERF-030).
#[test]
fn stacked_bands_cumulative() {
    let vals = [10.0, 20.0, 30.0];
    let data: Vec<Vec<Sample>> = vals
        .iter()
        .map(|&v| series(50, 100 * MS, 10 * SEC, move |_| v))
        .collect();
    let map = exact_map(&data[0]);
    let views: Vec<SeriesView> = data.iter().map(|s| view(s.clone(), 100 * MS)).collect();
    let spec: Vec<(ChartSeries, Option<&SeriesView>)> = views
        .iter()
        .enumerate()
        .map(|(i, v)| (line(Some(colour(i)), colour(i), false), Some(v)))
        .collect();
    let mut cache = ChartCache::default();
    // `Stacked(3)`: 0..300.
    stack_build(
        &spec,
        map,
        data[0][49].t_ns,
        300.0,
        &mut cache,
        &mut Scratch::default(),
    );
    let y = |v: f32| y_of(v, 0.0, 300.0, 100.0);

    assert_eq!(cache.fills.len(), 3);
    assert_eq!(cache.strokes.len(), 3);
    // Each band goes from its top line down to the band below it.
    let close =
        |(a, b): (f32, f32), (c, d): (f32, f32)| (a - c).abs() < 0.01 && (b - d).abs() < 0.01;
    assert!(close(y_span(&cache.fills[2]), (y(60.0), y(30.0))));
    assert!(close(y_span(&cache.fills[1]), (y(30.0), y(10.0))));
    assert!(close(y_span(&cache.fills[0]), (y(10.0), y(0.0))));
    // Strokes are 1 px wide around each band's top line.
    for (k, top) in [10.0, 30.0, 60.0].into_iter().enumerate() {
        let (lo, hi) = y_span(&cache.strokes[k]);
        assert!(((lo + hi) / 2.0 - y(top)).abs() < 0.01, "band {k}");
    }
}

#[test]
fn overlaid_merges_to_one_stroke() {
    let line_colour = hsla(0.55, 0.8, 0.5, 1.0 / 16.0);
    let data: Vec<Vec<Sample>> = (0..16)
        .map(|k| series(50, 100 * MS, 10 * SEC, move |i| noise(i + k * 7)))
        .collect();
    let map = exact_map(&data[0]);
    let views: Vec<SeriesView> = data.iter().map(|s| view(s.clone(), 100 * MS)).collect();
    let spec: Vec<(ChartSeries, Option<&SeriesView>)> = views
        .iter()
        .map(|v| (line(None, line_colour, false), Some(v)))
        .collect();
    let cache = run_build(&spec, map, data[0][49].t_ns);
    assert_eq!(cache.strokes.len(), 1);
    assert!(cache.fills.is_empty());
}

#[test]
fn stacked_gap_in_middle_series() {
    let vals = [10.0, 20.0, 30.0];
    let mut data: Vec<Vec<Sample>> = vals
        .iter()
        .map(|&v| series(200, 100 * MS, 10 * SEC, move |_| v))
        .collect();
    data[1][100] = Sample::gap(data[1][100].t_ns);
    // The other series jitter by up to half a period and still stack by bucket.
    for s in &mut data[2] {
        s.t_ns += 40 * MS;
    }
    let map = exact_map(&data[0]);
    let views: Vec<SeriesView> = data.iter().map(|s| view(s.clone(), 100 * MS)).collect();
    let spec: Vec<(ChartSeries, Option<&SeriesView>)> = views
        .iter()
        .enumerate()
        .map(|(i, v)| (line(Some(colour(i)), colour(i), false), Some(v)))
        .collect();
    let mut cache = ChartCache::default();
    stack_build(
        &spec,
        map,
        data[0][199].t_ns,
        300.0,
        &mut cache,
        &mut Scratch::default(),
    );
    let y = |v: f32| y_of(v, 0.0, 300.0, 100.0);
    let x = |s: &Sample| map.x_local(s.t_ns);

    // Band 2 at bucket 100 is s0 + s2 over a floor of s0, and its neighbours are full sums.
    let band2 = &cache.fills[2];
    assert!(has_vertex(band2, x(&data[2][100]), y(40.0)));
    assert!(has_vertex(band2, x(&data[2][100]), y(10.0)));
    assert!(has_vertex(band2, x(&data[2][99]), y(60.0)));
    assert!(has_vertex(band2, x(&data[2][101]), y(60.0)));
    // Band 1 has its own gap there: no vertex at bucket 100.
    let x100 = x(&data[1][100]);
    assert!(xs(&cache.fills[1]).all(|v| (v - x100).abs() > 0.01));
    assert!(xs(&cache.fills[1]).any(|v| v < x100) && xs(&cache.fills[1]).any(|v| v > x100));
    // Dotted gaps come from series 0, which has none.
    assert!(cache.gaps.is_empty());
}

/// Three bands of 10, 20 and 30 on a 1 s grid, built stacked to 100.
fn stack_three(data: &[Vec<Sample>]) -> ChartCache {
    let map = exact_map(&data[0]);
    let views: Vec<SeriesView> = data.iter().map(|s| view(s.clone(), SEC)).collect();
    let spec: Vec<(ChartSeries, Option<&SeriesView>)> = views
        .iter()
        .enumerate()
        .map(|(i, v)| (line(Some(colour(i)), colour(i), false), Some(v)))
        .collect();
    let mut cache = ChartCache::default();
    let now = data[0][data[0].len() - 1].t_ns;
    stack_build(&spec, map, now, 100.0, &mut cache, &mut Scratch::default());
    cache
}

/// A slow sweep ends 0.6 s late and the next one 0.1 s late, so two samples of each series fall
/// in one period slot. Each lower sample counts once in the floor above it (tasklens-o55.53,
/// UI-PRF-060).
#[test]
fn stacked_same_slot_counts_once() {
    let vals = [10.0, 20.0, 30.0];
    let data: Vec<Vec<Sample>> = vals
        .iter()
        .map(|&v| {
            let mut s = series(60, SEC, 10 * SEC, move |_| v);
            s[30].t_ns += 600 * MS;
            s[31].t_ns += 100 * MS;
            s
        })
        .collect();
    let map = exact_map(&data[0]);
    let cache = stack_three(&data);
    let y = |v: f32| y_of(v, 0.0, 100.0, 100.0);

    for i in [30, 31] {
        let x = map.x_local(data[0][i].t_ns);
        assert!(
            has_vertex(&cache.fills[1], x, y(10.0)),
            "band 1 floor at {i}"
        );
        assert!(has_vertex(&cache.fills[1], x, y(30.0)), "band 1 top at {i}");
        assert!(
            has_vertex(&cache.fills[2], x, y(30.0)),
            "band 2 floor at {i}"
        );
        assert!(has_vertex(&cache.fills[2], x, y(60.0)), "band 2 top at {i}");
    }
    for (k, top) in [10.0, 30.0, 60.0].into_iter().enumerate() {
        assert!(
            ys(&cache.fills[k]).all(|v| v >= y(top) - 0.01),
            "band {k} above its top"
        );
    }
}

/// A sweep late past the stale limit breaks every band at the same time: one empty column, no
/// partial stack (tasklens-o55.53, UI-PRF-060).
#[test]
fn stacked_all_bands_gap_together() {
    let vals = [10.0, 20.0, 30.0];
    let data: Vec<Vec<Sample>> = vals
        .iter()
        .map(|&v| {
            let mut s = series(60, SEC, 10 * SEC, move |_| v);
            s[30].flags |= SampleFlags::GAP_BEFORE;
            s
        })
        .collect();
    let map = exact_map(&data[0]);
    let cache = stack_three(&data);
    let (x29, x30) = (map.x_local(data[0][29].t_ns), map.x_local(data[0][30].t_ns));
    for (k, fill) in cache.fills.iter().enumerate() {
        assert!(
            xs(fill).all(|v| v <= x29 + 0.01 || v >= x30 - 0.01),
            "band {k} draws inside the gap"
        );
        assert!(xs(fill).any(|v| (v - x29).abs() < 0.01), "band {k} before");
        assert!(xs(fill).any(|v| (v - x30).abs() < 0.01), "band {k} after");
    }
}

#[test]
fn many_series_no_per_rebuild_allocation() {
    let data: Vec<Vec<Sample>> = (0..32)
        .map(|k| series(600, 100 * MS, 10 * SEC, move |i| noise(i + k) / 32.0))
        .collect();
    let views: Vec<SeriesView> = data.iter().map(|s| view(s.clone(), 100 * MS)).collect();
    let spec = ChartSpec {
        series: (0..32)
            .map(|i| line(Some(colour(i)), colour(i), false))
            .collect(),
        y: YScale::Stacked(32),
        stack: true,
        ..ChartSpec::default()
    };
    assert!(spec.series.spilled());
    let series_ptr = spec.series.as_ptr();

    let map = exact_map(&data[0]);
    let now = data[0][599].t_ns;
    let mut cache = ChartCache::default();
    let mut scratch = Scratch::default();
    let mut pairs: Vec<(ChartSeries, Option<&SeriesView>)> = Vec::with_capacity(32);
    let mut fills_ptr = None;
    for _ in 0..100 {
        pairs.clear();
        pairs.extend(spec.series.iter().cloned().zip(views.iter().map(Some)));
        stack_build(&pairs, map, now, 3200.0, &mut cache, &mut scratch);
        assert_eq!(cache.fills.len(), 32);
        assert!(cache.fills.spilled());
        // The fill list keeps its first heap buffer.
        assert_eq!(
            *fills_ptr.get_or_insert(cache.fills.as_ptr()),
            cache.fills.as_ptr()
        );
        assert!(spec.series.spilled());
        assert_eq!(spec.series.as_ptr(), series_ptr);
    }
}

/// Y scale rules and axis text (tasklens-yrf.6, UI-PRF-006).
mod scale {
    use tl_model::settings::NetworkUnit;
    use tl_ui::chart::scale::{ScaleResult, ScaleState, compute};
    use tl_ui::chart::{ValueFormat, YScale};

    const SEC: u64 = 1_000_000_000;
    const MIB: f32 = 1024.0 * 1024.0;

    fn run(y: YScale, vmin: f32, vmax: f32, now: u64, state: &mut ScaleState) -> ScaleResult {
        let mut text = String::new();
        compute(
            &y,
            vmin,
            vmax,
            now,
            state,
            ValueFormat::Watts,
            NetworkUnit::Bits,
            &mut text,
        )
    }

    fn text(r: &ScaleResult) -> Option<&str> {
        r.axis_text.as_ref().map(|t| t.as_str())
    }

    fn rate_state(ceiling: f32) -> ScaleState {
        ScaleState {
            ceiling,
            initialised: true,
            ..ScaleState::default()
        }
    }

    const BITS: YScale = YScale::AutoRate { iec: false };
    /// Samples are bytes per second: `n / B` is `n` bits per second.
    const B: f32 = 8.0;

    #[test]
    fn autorate_rises_immediately() {
        let mut s = ScaleState::default();
        let r = run(BITS, 0.0, 3.2e6 / B, 0, &mut s);
        assert_eq!(r.y_min, 0.0);
        assert_eq!(r.y_max, 5e6 / B);
        assert_eq!(text(&r), Some("5 Mbps"));

        let mut s = rate_state(5e6 / B);
        let r = run(BITS, 0.0, 7e6 / B, SEC, &mut s);
        assert_eq!(r.y_max, 1e7 / B);
        assert_eq!(text(&r), Some("10 Mbps"));
    }

    #[test]
    fn autorate_falls_after_10s_below_half() {
        let mut s = rate_state(1e7 / B);
        assert_eq!(run(BITS, 0.0, 2e6 / B, 0, &mut s).y_max, 1e7 / B);
        assert_eq!(s.below_half_since, Some(0));
        assert_eq!(run(BITS, 0.0, 2e6 / B, 9 * SEC, &mut s).y_max, 1e7 / B);
        assert_eq!(s.below_half_since, Some(0));
        let r = run(BITS, 0.0, 2e6 / B, 10 * SEC, &mut s);
        assert_eq!(r.y_max, 2e6 / B);
        assert_eq!(text(&r), Some("2 Mbps"));
        assert_eq!(s.below_half_since, None);
    }

    #[test]
    fn autorate_fall_timer_resets_above_half() {
        let mut s = rate_state(1e7 / B);
        run(BITS, 0.0, 2e6 / B, 0, &mut s);
        assert_eq!(s.below_half_since, Some(0));
        let r = run(BITS, 0.0, 6e6 / B, 5 * SEC, &mut s);
        assert_eq!(s.below_half_since, None);
        assert_eq!(r.y_max, 1e7 / B);
        // The timer starts again from the next low sample.
        run(BITS, 0.0, 2e6 / B, 6 * SEC, &mut s);
        assert_eq!(run(BITS, 0.0, 2e6 / B, 15 * SEC, &mut s).y_max, 1e7 / B);
        assert_eq!(run(BITS, 0.0, 2e6 / B, 16 * SEC, &mut s).y_max, 2e6 / B);
    }

    #[test]
    fn autorate_iec_pow2_text() {
        let mut s = ScaleState::default();
        let r = run(YScale::AutoRate { iec: true }, 0.0, 3.0 * MIB, 0, &mut s);
        assert_eq!(r.y_max, 4.0 * MIB);
        assert_eq!(text(&r), Some("4 MiB/s"));

        // No samples: the minimum ceiling, 1 KiB/s.
        let mut s = ScaleState::default();
        let r = run(YScale::AutoRate { iec: true }, 0.0, 0.0, 0, &mut s);
        assert_eq!(r.y_max, 1024.0);
        assert_eq!(text(&r), Some("1 KiB/s"));
        let mut s = ScaleState::default();
        assert_eq!(run(BITS, 0.0, f32::NAN, 0, &mut s).y_max, 1000.0 / 8.0);
    }

    #[test]
    fn sticky_temp_initial_range() {
        let mut s = ScaleState::default();
        let r = run(YScale::StickyTemp, 0.0, 0.0, 0, &mut s);
        assert_eq!((r.y_min, r.y_max), (35.0, 45.0));
        assert_eq!(text(&r), Some("45 °C"));
        // Values inside the start range keep it.
        let r = run(YScale::StickyTemp, 38.0, 42.0, SEC, &mut s);
        assert_eq!((r.y_min, r.y_max), (35.0, 45.0));
    }

    #[test]
    fn sticky_temp_rounds_to_5() {
        let mut s = ScaleState::default();
        let r = run(YScale::StickyTemp, 40.0, 71.0, 0, &mut s);
        assert_eq!(r.y_max, 75.0);
        assert_eq!(text(&r), Some("75 °C"));
        let r = run(YScale::StickyTemp, 27.0, 50.0, SEC, &mut s);
        assert_eq!((r.y_min, r.y_max), (25.0, 75.0));
        // 48 is below half of 25..75; the max falls to 50 after 10 s.
        assert_eq!(
            run(YScale::StickyTemp, 30.0, 48.0, 2 * SEC, &mut s).y_max,
            75.0
        );
        let r = run(YScale::StickyTemp, 30.0, 48.0, 12 * SEC, &mut s);
        assert_eq!((r.y_min, r.y_max), (25.0, 50.0));
    }

    #[test]
    fn commit_axis_text_only_above_limit() {
        let mut s = ScaleState::default();
        let r = run(YScale::Commit(100.0), 0.0, 120.0, 0, &mut s);
        assert_eq!(r.y_max, 120.0);
        assert_eq!(text(&r), Some("120%"));
        let r = run(YScale::Commit(100.0), 0.0, 80.0, 0, &mut s);
        assert_eq!(r.y_max, 100.0);
        assert_eq!(text(&r), None);
    }

    #[test]
    fn symmetric_label() {
        let mut s = ScaleState::default();
        let r = run(YScale::Symmetric, -12.4, 3.0, 0, &mut s);
        assert_eq!((r.y_min, r.y_max), (-20.0, 20.0));
        assert_eq!(text(&r), Some("±20 W"));
        let r = run(YScale::Symmetric, 0.0, 0.0, 0, &mut s);
        assert_eq!((r.y_min, r.y_max), (-1.0, 1.0));
    }

    #[test]
    fn stacked_label() {
        let mut s = ScaleState::default();
        let r = run(YScale::Stacked(16), 0.0, 0.0, 0, &mut s);
        assert_eq!((r.y_min, r.y_max), (0.0, 1600.0));
        assert_eq!(text(&r), Some("1600%"));
    }

    #[test]
    fn link_speed_text() {
        let mut s = ScaleState::default();
        // 1 Gbps tops out at 1.25e8 B/s.
        let r = run(YScale::LinkSpeed(1e9), 0.0, 5e7, 0, &mut s);
        assert_eq!((r.y_min, r.y_max), (0.0, 1.25e8));
        assert_eq!(text(&r), Some("1 Gbps"));
        let r = run(YScale::LinkSpeed(2.5e9), 0.0, 0.0, 0, &mut s);
        assert_eq!(text(&r), Some("2.5 Gbps"));
    }

    #[test]
    fn fixed_and_sticky_up() {
        let mut s = ScaleState::default();
        let r = run(
            YScale::Fixed {
                min: 0.0,
                max: 100.0,
            },
            0.0,
            50.0,
            0,
            &mut s,
        );
        assert_eq!((r.y_min, r.y_max, text(&r)), (0.0, 100.0, None));
        let r = run(YScale::FixedTotal(0.0), 0.0, 50.0, 0, &mut s);
        assert_eq!((r.y_min, r.y_max), (0.0, 1.0));

        let r = run(
            YScale::StickyUp {
                hw_max: Some(300.0),
            },
            0.0,
            50.0,
            0,
            &mut s,
        );
        assert_eq!((r.y_max, text(&r)), (300.0, Some("300 W")));

        let mut s = ScaleState::default();
        let up = YScale::StickyUp { hw_max: None };
        assert_eq!(run(up, 0.0, 1234.0, 0, &mut s).y_max, 2000.0);
        // Never falls.
        let r = run(up, 0.0, 10.0, 60 * SEC, &mut s);
        assert_eq!((r.y_max, text(&r)), (2000.0, Some("2000 W")));
    }
}

/// Curved lines (tasklens-yrf.18, PERF-021): `pts` emitted curved on a 0..100 scale, 100 px high,
/// 100 px per second.
fn curved(pts: &[Pt]) -> Vec<(f32, f32)> {
    let newest = pts[pts.len() - 1].t_ns;
    let input = BuildInput {
        series: &[],
        map: TimeMap {
            anchor_ns: newest,
            right_edge_ns: newest,
            range_ns: newest - pts[0].t_ns,
            plot_width: ((newest - pts[0].t_ns) / (10 * MS)) as f32,
        },
        plot_height: 100.0,
        y_min: 0.0,
        y_max: 100.0,
        now_boot_ns: newest,
        stack: false,
        curved: true,
    };
    let mut out = Vec::new();
    emit_run(pts, true, &input, &mut Vec::new(), &mut out);
    out.iter().map(|p| (p.x.as_f32(), p.y.as_f32())).collect()
}

#[test]
fn monotone_curve_never_overshoots() {
    let pts: Vec<Pt> = [0.0, 100.0, 0.0, 100.0]
        .iter()
        .enumerate()
        .map(|(i, &value)| Pt {
            t_ns: i as u64 * SEC,
            value,
        })
        .collect();
    let out = curved(&pts);
    assert_eq!(out.len(), 1 + 3 * 4, "4 line segments per segment");
    for (x, y) in out {
        // y 0 is value 100 and y 100 is value 0.
        let value = 100.0 - y;
        assert!((0.0..=100.0).contains(&value), "value {value} at x {x}");
    }
}

#[test]
fn curve_through_collinear_points_is_straight() {
    let pts: Vec<Pt> = (0..10)
        .map(|i| Pt {
            t_ns: i as u64 * SEC,
            value: 10.0 + 7.5 * i as f32,
        })
        .collect();
    let out = curved(&pts);
    assert_eq!(out.len(), 1 + 9 * 4);
    // The line from (−900, 90) to (0, 22.5): x = (t − newest) × 100 px/s, y = 100 − value.
    let (x0, y0) = out[0];
    let (x1, y1) = out[out.len() - 1];
    for (x, y) in out {
        let on_line = y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        assert!(
            (y - on_line).abs() < 0.01,
            "({x}, {y}) is {on_line} on the line"
        );
    }
}

/// The summed area of a fill set's triangles. Equal to the polygon's area only when no two
/// triangles overlap and none is missing (tasklens-wlf.33).
fn fill_area(set: &PathSet) -> f32 {
    set.path
        .vertices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let p = |i: usize| (t[i].xy_position.x.as_f32(), t[i].xy_position.y.as_f32());
            let (a, b, c) = (p(0), p(1), p(2));
            ((b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)).abs() / 2.0
        })
        .sum()
}

/// Area between the polyline through `pts` and the baseline `h`, by trapezoids.
fn area_under(pts: &[(f32, f32)], h: f32) -> f32 {
    pts.windows(2)
        .map(|w| (w[1].0 - w[0].0) * ((h - w[0].1) + (h - w[1].1)) / 2.0)
        .sum()
}

/// Direct triangulation (tasklens-wlf.33): two triangles per segment, the exact area under the
/// line, nothing inside a gap, so no triangle overlaps another and none is missing.
#[test]
fn fill_triangles_cover_the_area_under_each_run() {
    let mut s = series(100, 100 * MS, 10 * SEC, noise);
    s[50].flags |= SampleFlags::GAP_BEFORE;
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let cache = run_build(&[(line(Some(BLUE), RED, false), Some(&v))], map, s[99].t_ns);

    let fill = &cache.fills[0];
    let xy = |s: &Sample| (map.x_local(s.t_ns), y_of(s.value, 0.0, 100.0, 100.0));
    let left: Vec<_> = s[..50].iter().map(xy).collect();
    let right: Vec<_> = s[50..].iter().map(xy).collect();
    let want = area_under(&left, 100.0) + area_under(&right, 100.0);
    let got = fill_area(fill);
    assert!((got - want).abs() < want * 1e-4, "area {got}, want {want}");
    // 49 segments in each run, 2 triangles of 3 vertices each.
    assert_eq!(fill.path.vertices.len(), 2 * 49 * 2 * 3);
    let (x49, x50) = (map.x_local(s[49].t_ns), map.x_local(s[50].t_ns));
    assert!(xs(fill).all(|x| x <= x49 + 0.01 || x >= x50 - 0.01));
}

/// Direct stroke triangulation (tasklens-wlf.35): a straight 1 px line is two triangles per
/// segment, with no overlap and the exact rectangular area.
#[test]
fn stroke_triangles_cover_a_straight_line() {
    let s = series(11, 100 * MS, 10 * SEC, |_| 50.0);
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let cache = run_build(&[(line(None, RED, false), Some(&v))], map, s[10].t_ns);

    let stroke = &cache.strokes[0];
    assert_eq!(stroke.path.vertices.len(), 10 * 2 * 3);
    assert!((fill_area(stroke) - 100.0).abs() < 0.001);
    assert_eq!(stroke.path.bounds.origin.x.as_f32(), -100.0);
    assert_eq!(stroke.path.bounds.size.width.as_f32(), 100.0);
    assert_eq!(stroke.path.bounds.size.height.as_f32(), 1.0);
}

/// A sharp turn between two short segments folds its join, as lyon does: the inner bevel point
/// would sit about 20 px below the apex, past both 5 px segments, and draw a false spike.
#[test]
fn sharp_short_join_folds_without_a_spike() {
    let s = vec![
        Sample::valid(0, 50.0),
        Sample::valid(MS, 55.0),
        Sample::valid(2 * MS, 50.0),
    ];
    // 0.1 px per ms: the points are (-0.2, 50), (-0.1, 45) and (0, 50).
    let map = TimeMap {
        anchor_ns: 2 * MS,
        right_edge_ns: 2 * MS,
        range_ns: 1_000 * MS,
        plot_width: 100.0,
    };
    let v = view(s, MS);
    let cache = run_build(&[(line(None, RED, false), Some(&v))], map, 2 * MS);

    let stroke = &cache.strokes[0];
    // Two quads and no join triangle.
    assert_eq!(stroke.path.vertices.len(), 2 * 2 * 3);
    let (top, bottom) = y_span(stroke);
    assert!(top > 44.9, "top {top}");
    assert!(bottom < 50.1, "bottom {bottom}");
    assert!(stroke.path.bounds.size.height.as_f32() < 5.2);
}

#[test]
fn dashed_stroke_has_five_pixel_segments() {
    let s = series(11, 100 * MS, 10 * SEC, |_| 50.0);
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let cache = run_build(&[(line(None, RED, true), Some(&v))], map, s[10].t_ns);

    let stroke = &cache.strokes[0];
    assert!(stroke.dashed);
    assert_eq!(stroke.path.vertices.len(), 10 * 2 * 3);
    assert!((fill_area(stroke) - 50.0).abs() < 0.001);
}

/// GPUI's former dashed `PathBuilder` measured all subpaths as one path. The second run therefore
/// continues the first run's dash phase instead of starting with a fresh 5 px dash.
#[test]
fn dashed_stroke_phase_continues_across_runs() {
    let mut s = vec![
        Sample::valid(0, 50.0),
        Sample::valid(60 * MS, 50.0),
        Sample::valid(100 * MS, 50.0),
        Sample::valid(160 * MS, 50.0),
    ];
    s[2].flags |= SampleFlags::GAP_BEFORE;
    let map = TimeMap {
        anchor_ns: 0,
        right_edge_ns: 160 * MS,
        range_ns: 1_000 * MS,
        plot_width: 100.0,
    };
    let v = view(s, 100 * MS);
    let cache = run_build(&[(line(None, RED, true), Some(&v))], map, 160 * MS);

    let stroke = &cache.strokes[0];
    // Each run is 6 px. The aggregate 5-on/5-off pattern draws 5 px in the first run and the
    // final 2 px in the second: 7 px by the 1 px stroke width.
    assert!((fill_area(stroke) - 7.0).abs() < 0.001);
}

/// A one-point run goes through the dash pattern: it draws nothing in an off phase.
#[test]
fn dashed_single_point_run_respects_off_phase() {
    let mut s = vec![
        Sample::valid(0, 50.0),
        Sample::valid(60 * MS, 50.0),
        Sample::valid(100 * MS, 50.0),
        Sample::gap(130 * MS),
    ];
    s[2].flags |= SampleFlags::GAP_BEFORE;
    let map = TimeMap {
        anchor_ns: 0,
        right_edge_ns: 130 * MS,
        range_ns: 1_000 * MS,
        plot_width: 100.0,
    };
    let v = view(s, 100 * MS);
    let cache = run_build(&[(line(None, RED, true), Some(&v))], map, 130 * MS);

    // The first run draws 5 px, then 1 px of the off phase. The 1 px stub stays in the off phase.
    assert!((fill_area(&cache.strokes[0]) - 5.0).abs() < 0.001);
}

/// A run of one point has no width: no fill set, but the stroke still draws its 1 px dash.
#[test]
fn single_point_run_has_no_fill() {
    let mut s = series(3, 100 * MS, 10 * SEC, |_| 40.0);
    s[1] = Sample::gap(s[1].t_ns);
    s[2] = Sample::gap(s[2].t_ns);
    let map = TimeMap {
        anchor_ns: s[0].t_ns,
        right_edge_ns: s[0].t_ns,
        range_ns: 10 * SEC,
        plot_width: 1_000.0,
    };
    let v = view(s.clone(), 100 * MS);
    let cache = run_build(&[(line(Some(BLUE), RED, false), Some(&v))], map, s[0].t_ns);
    assert!(cache.fills.is_empty());
    assert_eq!(cache.strokes.len(), 1);
}

/// A stacked band fills exactly the area between its top line and its floor.
#[test]
fn stacked_band_area_is_top_minus_floor() {
    let data: Vec<Vec<Sample>> = [10.0, 20.0]
        .iter()
        .map(|&v| series(50, 100 * MS, 10 * SEC, move |i| v + (i % 3) as f32))
        .collect();
    let map = exact_map(&data[0]);
    let views: Vec<SeriesView> = data.iter().map(|s| view(s.clone(), 100 * MS)).collect();
    let spec: Vec<(ChartSeries, Option<&SeriesView>)> = views
        .iter()
        .enumerate()
        .map(|(i, v)| (line(Some(colour(i)), colour(i), false), Some(v)))
        .collect();
    let mut cache = ChartCache::default();
    stack_build(
        &spec,
        map,
        data[0][49].t_ns,
        100.0,
        &mut cache,
        &mut Scratch::default(),
    );
    let pts = |k: usize| -> Vec<(f32, f32)> {
        (0..50)
            .map(|i| {
                let sum: f32 = data[..=k].iter().map(|s| s[i].value).sum();
                (map.x_local(data[0][i].t_ns), y_of(sum, 0.0, 100.0, 100.0))
            })
            .collect()
    };
    let want = area_under(&pts(1), 100.0) - area_under(&pts(0), 100.0);
    let got = fill_area(&cache.fills[1]);
    assert!((got - want).abs() < want * 1e-4, "area {got}, want {want}");
}

/// Rebuilding reuses the fill vertex buffers: the second build's fill keeps the capacity.
#[test]
fn rebuild_reuses_fill_buffers() {
    let s = series(200, 100 * MS, 10 * SEC, noise);
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let spec = [(line(Some(BLUE), RED, false), Some(&v))];
    let input = BuildInput {
        series: &spec,
        map,
        plot_height: 100.0,
        y_min: 0.0,
        y_max: 100.0,
        now_boot_ns: s[199].t_ns,
        stack: false,
        curved: false,
    };
    let (mut cache, mut scratch) = (ChartCache::default(), Scratch::default());
    build(&input, &mut cache, &mut scratch);
    let first = cache.fills[0].path.vertices.as_ptr();
    build(&input, &mut cache, &mut scratch);
    assert_eq!(cache.fills[0].path.vertices.as_ptr(), first);
}

/// Rebuilding reuses the direct stroke vertex buffer too.
#[test]
fn rebuild_reuses_stroke_buffers() {
    let s = series(200, 100 * MS, 10 * SEC, noise);
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let spec = [(line(None, RED, false), Some(&v))];
    let input = BuildInput {
        series: &spec,
        map,
        plot_height: 100.0,
        y_min: 0.0,
        y_max: 100.0,
        now_boot_ns: s[199].t_ns,
        stack: false,
        curved: false,
    };
    let (mut cache, mut scratch) = (ChartCache::default(), Scratch::default());
    build(&input, &mut cache, &mut scratch);
    let first = cache.strokes[0].path.vertices.as_ptr();
    build(&input, &mut cache, &mut scratch);
    assert_eq!(cache.strokes[0].path.vertices.as_ptr(), first);
}

/// A missing total series leaves only the overlay path in the cache. Its buffer must return to
/// the overlay mesh on the next grid build instead of being consumed by the empty total mesh.
#[test]
fn grid_rebuild_reuses_overlay_buffer_when_total_is_absent() {
    let s = series(200, 100 * MS, 10 * SEC, noise);
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let spec = [
        (line(Some(BLUE), RED, false), None),
        (line(None, BLUE, false), Some(&v)),
    ];
    let input = BuildInput {
        series: &spec,
        map,
        plot_height: 100.0,
        y_min: 0.0,
        y_max: 100.0,
        now_boot_ns: s[199].t_ns,
        stack: false,
        curved: false,
    };
    let (mut cache, mut scratch) = (ChartCache::default(), Scratch::default());

    let mut fill = scratch.take_grid(&mut cache);
    build_cell(&input, (0.0, 0.0), 1_000.0, &mut fill, &mut scratch);
    finish_grid(
        &input,
        (BLUE, fill),
        RED,
        Some(BLUE),
        &mut cache,
        &mut scratch,
    );
    assert_eq!(cache.strokes.len(), 1);
    assert_eq!(cache.strokes[0].color, BLUE);
    let first = cache.strokes[0].path.vertices.as_ptr();

    let mut fill = scratch.take_grid(&mut cache);
    build_cell(&input, (0.0, 0.0), 1_000.0, &mut fill, &mut scratch);
    finish_grid(
        &input,
        (BLUE, fill),
        RED,
        Some(BLUE),
        &mut cache,
        &mut scratch,
    );
    assert_eq!(cache.strokes.len(), 1);
    assert_eq!(cache.strokes[0].path.vertices.as_ptr(), first);
}

/// `Scratch` and `ChartCache` are reusable public buffers. Switching them from an ordinary chart
/// build to a grid build discards the ordinary stroke instead of tripping a debug assertion.
#[test]
fn grid_build_accepts_cache_from_ordinary_chart() {
    let s = series(20, 100 * MS, 10 * SEC, noise);
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let spec = [(line(None, RED, false), Some(&v))];
    let input = BuildInput {
        series: &spec,
        map,
        plot_height: 100.0,
        y_min: 0.0,
        y_max: 100.0,
        now_boot_ns: s[19].t_ns,
        stack: false,
        curved: false,
    };
    let (mut cache, mut scratch) = (ChartCache::default(), Scratch::default());
    build(&input, &mut cache, &mut scratch);
    assert_eq!(cache.strokes.len(), 1);

    let mut fill = scratch.take_grid(&mut cache);
    build_cell(&input, (0.0, 0.0), 1_000.0, &mut fill, &mut scratch);
    finish_grid(&input, (BLUE, fill), RED, None, &mut cache, &mut scratch);
    assert_eq!(cache.strokes.len(), 1);
    assert_eq!(cache.strokes[0].color, RED);
}

#[test]
fn overview_core_sparkline_plots_second_means() {
    // 10 Hz samples from 0.05 s: 0/100 alternating, 3 buckets of 10 and a partial one.
    let samples = series(
        35,
        100 * MS,
        50 * MS,
        |i| if i % 2 == 0 { 0. } else { 100. },
    );
    let mut out = Vec::new();
    mean_samples(&samples, 0, SEC, &mut out);
    assert_eq!(out.len(), 4);
    assert_eq!(out[0].value, 50.);
    assert_eq!(out[0].t_ns, 950 * MS, "at the newest sample of the bucket");
    assert!(out.iter().all(|s| s.is_valid()));
    // An invalid sample stays and ends the bucket.
    let mut gap = series(5, 100 * MS, 0, |_| 10.);
    gap.insert(2, Sample::gap(150 * MS));
    mean_samples(&gap, 0, SEC, &mut out);
    assert_eq!(out.len(), 3);
    assert!(out[0].is_valid() && !out[1].is_valid() && out[2].is_valid());
}

/// UI-PRF-058: two series in one colour merge into one stroke, but each keeps its own newest
/// point, at its newest sample.
#[test]
fn newest_point_per_series_when_colours_match() {
    let a = series(20, 100 * MS, 10 * SEC, |_| 20.0);
    let b = series(20, 100 * MS, 10 * SEC, |_| 70.0);
    let map = exact_map(&a);
    let (va, vb) = (view(a.clone(), 100 * MS), view(b, 100 * MS));
    let cache = run_build(
        &[
            (line(None, RED, false), Some(&va)),
            (line(None, RED, false), Some(&vb)),
        ],
        map,
        a[19].t_ns,
    );
    assert_eq!(cache.strokes.len(), 1, "one merged stroke");
    let newest: Vec<_> = cache
        .newest
        .iter()
        .map(|(p, c)| (p.x.as_f32(), p.y.as_f32(), *c))
        .collect();
    assert_eq!(
        newest,
        vec![
            (0.0, y_of(20.0, 0.0, 100.0, 100.0), RED),
            (0.0, y_of(70.0, 0.0, 100.0, 100.0), RED),
        ]
    );
}

/// UI-PRF-058: a dashed line whose newest sample falls in a dash gap still puts the newest point
/// on that sample, not at the end of the last dash.
#[test]
fn newest_point_on_dashed_line_ends_at_the_sample() {
    // 3 samples 10 px apart: 20 px of 5 on, 5 off, so the last 5 px are a gap.
    let s = series(3, 100 * MS, 10 * SEC, |_| 50.0);
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let cache = run_build(&[(line(None, RED, true), Some(&v))], map, s[2].t_ns);
    let right = xs(&cache.strokes[0]).fold(f32::MIN, f32::max);
    assert!(
        right <= -4.0,
        "the last dash ends before the sample: {right}"
    );
    let (p, c) = cache.newest[0];
    assert_eq!((p.x.as_f32(), p.y.as_f32(), c), (0.0, 50.0, RED));
}
