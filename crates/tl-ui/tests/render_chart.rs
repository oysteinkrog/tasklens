//! Chart render goldens (E19, tasklens-yrf.13, ADR-013, UI-PRF-001, UI-PRF-002): one chart with
//! a gap and a spike, and a 16-cell CPU grid with one offline cell, rendered on lavapipe at
//! `ui_scale` 1.0 and 1.1 and compared with `tests/golden/chart_gap_spike_<scale>.png` and
//! `cpu_grid_16_<scale>.png`. `TL_UPDATE_GOLDEN=1` rewrites them.
//!
//! The data is a fixed sine with seeded noise, never read from the host. Sample times sit on a
//! fixed phase of a 60 s boundary a little in the future, so the chart is never stale while it
//! renders and the 6 s grid lines land on the same pixels on every run.
//!
//! Run with `cargo test -p tl-ui --features render-tests,test-hooks --test render_chart`.

#![cfg(feature = "render-tests")]

#[cfg(not(feature = "test-hooks"))]
compile_error!("render_chart needs --features render-tests,test-hooks (R-291)");

use std::sync::Arc;

use gpui_kit::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
    size,
};
use smallvec::{SmallVec, smallvec};
use tl_model::client::EngineClient;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SampleFlags, SeriesView};
use tl_model::time::now_boot_ns;
use tl_ui::chart::view::ChartView;
use tl_ui::chart::{
    ChartGridSpec, ChartGridView, ChartLabels, ChartSeries, ChartSpec, GridCellSpec, GridStyle,
    LegendStyle, ValueAlign, ValueFormat, YScale,
};
use tl_ui::hub::SnapshotHub;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::testing::render::{RenderHarness, assert_golden, run_on_lavapipe};
use tl_ui::theme::{current, metrics};

const MS: u64 = 1_000_000;
const S: u64 = 1_000 * MS;

/// Samples per series, 100 ms apart.
const SAMPLES: usize = 600;
const PERIOD_NS: u64 = 100 * MS;
/// The first sample after the gap carries `GAP_BEFORE`.
const GAP_INDEX: usize = 300;
/// Time missing before `GAP_INDEX`, as after a suspend, so the gap is wide enough to see.
const GAP_NS: u64 = 5 * S;
/// The one sample at 100.
const SPIKE_INDEX: usize = 450;
/// Noise seed and standard deviation.
const SEED: u64 = 42;
const NOISE_SIGMA: f32 = 2.0;

/// Single chart window at `ui_scale` 1.0.
const CHART_W: f32 = 400.;
const CHART_H: f32 = 200.;

/// CPU grid: 16 cells in a 1000 px window lay out as 6 × 3.
const CELLS: u16 = 16;
const OFFLINE_CELL: u16 = 5;
const GRID_W: f32 = 1000.;
const CELL_MIN_W: f32 = 120.;
const CELL_H: f32 = 112.;

/// Newest sample time: 3.4 s past a 60 s boundary at least a minute ahead of the clock. Only the
/// phase against 6 s shows in the image, so every run draws the same pixels.
fn newest_ns() -> u64 {
    (now_boot_ns() / (60 * S) + 2) * 60 * S + 3_400 * MS
}

/// Box-Muller normal noise with standard deviation `sigma`.
fn normal(rng: &mut fastrand::Rng, sigma: f32) -> f32 {
    let u1 = rng.f32().max(f32::MIN_POSITIVE);
    let u2 = rng.f32();
    sigma * (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

/// The fixture values: a 15 s sine around 50 with amplitude 30, shifted by `phase` radians,
/// plus seeded noise, clamped to 0..100, and 100 at `SPIKE_INDEX`.
fn values(phase: f32) -> Vec<f32> {
    let mut rng = fastrand::Rng::with_seed(SEED);
    (0..SAMPLES)
        .map(|i| {
            let noise = normal(&mut rng, NOISE_SIGMA);
            if i == SPIKE_INDEX {
                return 100.0;
            }
            let t = i as f32 / 150.0;
            (50.0 + 30.0 * (std::f32::consts::TAU * t + phase).sin() + noise).clamp(0.0, 100.0)
        })
        .collect()
}

/// `values` as samples ending at `newest`, 100 ms apart, with `GAP_NS` missing before
/// `GAP_INDEX` and `GAP_BEFORE` on that sample.
fn samples(values: &[f32], newest: u64) -> Arc<[Sample]> {
    let last = values.len() - 1;
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let back = (last - i) as u64 * PERIOD_NS + if i < GAP_INDEX { GAP_NS } else { 0 };
            let mut s = Sample::valid(newest - back, v);
            if i == GAP_INDEX {
                s.flags |= SampleFlags::GAP_BEFORE;
            }
            s
        })
        .collect()
}

fn view(id: SeriesId, samples: Arc<[Sample]>) -> Arc<SeriesView> {
    Arc::new(SeriesView {
        id,
        version: 1,
        period_ns: PERIOD_NS,
        samples,
    })
}

fn total(device: u16) -> SeriesId {
    SeriesId {
        device: DeviceIdx(device),
        metric: MetricId::CPU_TOTAL,
    }
}

fn kernel(device: u16) -> SeriesId {
    SeriesId {
        device: DeviceIdx(device),
        metric: MetricId::CPU_KERNEL,
    }
}

/// Total and kernel (0.2 × total) series for `device`, with the sine shifted by `phase`.
fn total_and_kernel(device: u16, phase: f32, newest: u64) -> (Arc<SeriesView>, Arc<SeriesView>) {
    let t = values(phase);
    let k: Vec<f32> = t.iter().map(|v| 0.2 * v).collect();
    (
        view(total(device), samples(&t, newest)),
        view(kernel(device), samples(&k, newest)),
    )
}

fn hub(cx: &mut App) -> Entity<SnapshotHub> {
    let client: Arc<dyn EngineClient> = FakeEngine::new();
    cx.new(|cx| SnapshotHub::new(client, cx))
}

fn legend() -> SmallVec<[(gpui_kit::SharedString, LegendStyle); 2]> {
    smallvec![
        ("Total".into(), LegendStyle::Solid),
        ("Kernel".into(), LegendStyle::Dashed),
    ]
}

/// The theme's window background behind one child view.
struct Root<V: 'static> {
    child: Entity<V>,
}

impl<V: Render> Render for Root<V> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(current(cx).window_bg)
            .child(self.child.clone())
    }
}

/// The 400 × 200 utilization chart: filled total, dashed kernel, gap and spike.
fn chart_root(_: &mut Window, cx: &mut App) -> Entity<Root<ChartView>> {
    let graph = current(cx).graph.clone();
    let spec = ChartSpec {
        series: smallvec![
            ChartSeries {
                id: total(0),
                fill: Some(graph.cpu_fill),
                line: graph.cpu_line,
                ..ChartSeries::default()
            },
            ChartSeries {
                id: kernel(0),
                fill: None,
                line: graph.kernel_line,
                dashed: true,
                ..ChartSeries::default()
            },
        ],
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        grid: GridStyle {
            lines: true,
            baseline: true,
            outline: true,
            rounded: false,
        },
        smooth: false,
        ..ChartSpec::default()
    };
    let labels = ChartLabels {
        heading: Some("Utilization".into()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: "Utilization",
        legend: legend(),
        show_axis_max: true,
    };
    let hub = hub(cx);
    let (t, k) = total_and_kernel(0, 0.0, newest_ns());
    let chart = cx.new(|cx| {
        let mut v = ChartView::new(spec, labels, &hub, cx);
        v.set_series_for_test(smallvec![Some(t), Some(k)], cx);
        v
    });
    cx.new(|_| Root { child: chart })
}

/// The 16-cell CPU grid with kernel overlays; cell 5 is offline and has no series. Sizes follow
/// `ui_scale`, as the window does.
fn grid_root(_: &mut Window, cx: &mut App) -> Entity<Root<ChartGridView>> {
    tl_ui::i18n::init_strings(cx, "en-US");
    let m = *metrics(cx);
    let graph = current(cx).graph.clone();
    let spec = ChartGridSpec {
        cells: (0..CELLS)
            .map(|i| GridCellSpec {
                heading: format!("CPU {i}").into(),
                fill_series: total(i),
                overlay_series: Some(kernel(i)),
                offline: i == OFFLINE_CELL,
                mark: None,
                tooltip: None,
            })
            .collect(),
        fill: graph.cpu_fill,
        line: graph.cpu_line,
        overlay_line: graph.kernel_line,
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        smooth: false,
        cell_min_w: px(CELL_MIN_W * m.ui_scale),
        cell_h: px(CELL_H * m.ui_scale),
        gap: m.sp[4],
        legend: legend(),
        value_fmt: ValueFormat::Percent,
        ..ChartGridSpec::default()
    };
    let hub = hub(cx);
    let newest = newest_ns();
    let series = (0..CELLS)
        .map(|i| {
            if i == OFFLINE_CELL {
                return (None, None);
            }
            let (t, k) = total_and_kernel(i, f32::from(i) * 0.7, newest);
            (Some(t), Some(k))
        })
        .collect();
    let grid = cx.new(|cx| {
        let mut v = ChartGridView::new(spec, &hub, cx);
        v.set_series_for_test(series, cx);
        v
    });
    cx.new(|_| Root { child: grid })
}

/// Height of the 6 × 3 grid at `ui_scale` 1.0: 3 × 112 + 2 × 8.
fn grid_window_h() -> f32 {
    3. * CELL_H + 2. * 8.
}

fn check_chart(scale: f32, name: &str) {
    let mut harness = RenderHarness::new();
    harness.set_theme("Default Dark");
    harness.set_scale(scale);
    let img = harness.render(size(px(CHART_W), px(CHART_H)), chart_root);
    assert_golden(name, &img);
}

fn check_grid(scale: f32, name: &str) {
    let mut harness = RenderHarness::new();
    harness.set_theme("Default Dark");
    harness.set_scale(scale);
    let img = harness.render(size(px(GRID_W), px(grid_window_h())), grid_root);
    assert_golden(name, &img);
}

#[test]
fn render_chart_gap_and_spike_1_0() {
    run_on_lavapipe("render_chart_gap_and_spike_1_0", || {
        check_chart(1.0, "chart_gap_spike_100")
    });
}

#[test]
fn render_chart_gap_and_spike_1_1() {
    run_on_lavapipe("render_chart_gap_and_spike_1_1", || {
        check_chart(1.1, "chart_gap_spike_110")
    });
}

#[test]
fn render_cpu_grid_16_1_0() {
    run_on_lavapipe("render_cpu_grid_16_1_0", || {
        check_grid(1.0, "cpu_grid_16_100")
    });
}

#[test]
fn render_cpu_grid_16_1_1() {
    run_on_lavapipe("render_cpu_grid_16_1_1", || {
        check_grid(1.1, "cpu_grid_16_110")
    });
}
