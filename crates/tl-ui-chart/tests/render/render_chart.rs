//! Chart render goldens (E19, tasklens-yrf.13, ADR-013, UI-PRF-001, UI-PRF-002): one chart with
//! a gap and a spike, and a 16-cell CPU grid with one offline cell, rendered on lavapipe at
//! `ui_scale` 1.0 and 1.1 and compared with `tests/golden/chart_gap_spike_<scale>.png` and
//! `cpu_grid_16_<scale>.png`. The power draw charts (tasklens-1vwxr) render low and high watts
//! with the value ramp: `power_cpu_dark.png`, `power_gpu_dark.png` and `power_gpu_light.png`.
//!
//! Value colours (tasklens-j477q): the gap-and-spike chart and the CPU grid above render with
//! `appearance.color_charts_by_value` off, so they still match the flat goldens from before the
//! ramps. With the setting on, `chart_gap_spike_ramp_100.png` and `cpu_grid_16_ramp_100.png` show
//! the CPU ramp, and `ramp_<kind>_<theme>.png` one chart per kind: GPU utilisation, disk active
//! time and memory in use.
//! `TL_UPDATE_GOLDEN=1` rewrites them.
//!
//! The data is a fixed sine with seeded noise, never read from the host. Sample times sit on a
//! fixed phase of a 60 s boundary a little in the future, so the chart is never stale while it
//! renders and the 6 s grid lines land on the same pixels on every run.
//!
//! Run with `cargo test -p tl-ui-chart --features render-tests --test render`.

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
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::theme::{GraphPalette, ValueRampKind, current, metrics};
use tl_ui_chart::chart::view::ChartView;
use tl_ui_chart::chart::{
    ChartGridSpec, ChartGridView, ChartLabels, ChartSeries, ChartSpec, GridCellSpec, GridStyle,
    LegendStyle, ValueAlign, ValueFormat, YScale,
};
use tl_ui_pagehost::render::{RenderHarness, run_on_lavapipe};
use tl_ui_settings::settings::{Settings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;
use tl_ui_testkit::render::RgbaImage;

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
/// Where this crate's golden PNGs live.
const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

/// Compares `img` with `tests/golden/<name>.png`, or writes it when `TL_UPDATE_GOLDEN=1`.
fn assert_golden(name: &str, img: &RgbaImage) {
    tl_ui_pagehost::render::assert_golden(std::path::Path::new(GOLDEN_DIR), name, img);
}

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
    let graph = current(cx).graph;
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
            vertical_at: None,
        },
        smooth: false,
        ..ChartSpec::default()
    };
    let labels = ChartLabels {
        heading: Some("Utilization".into()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: "Utilization".into(),
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
    tl_ui_base::i18n::init_strings(cx, "en-US");
    let m = *metrics(cx);
    let graph = current(cx).graph;
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

/// Installs the settings global with `appearance.color_charts_by_value = on` (tasklens-j477q).
fn set_value_colors(on: bool, cx: &mut App) {
    let client: Arc<dyn EngineClient> = FakeEngine::new();
    cx.set_global(EngineGlobal(client));
    let mut settings = Settings::default();
    settings.appearance.color_charts_by_value = on;
    cx.set_global(BootSettings {
        settings,
        state: UiState::default(),
        persist: None,
        startup_toasts: Vec::new(),
    });
    TlSettings::init(cx);
}

/// `root` with value colours `on`.
fn colored<V: 'static>(
    on: bool,
    root: fn(&mut Window, &mut App) -> Entity<V>,
) -> impl FnOnce(&mut Window, &mut App) -> Entity<V> {
    move |window, cx| {
        set_value_colors(on, cx);
        root(window, cx)
    }
}

fn check_chart(scale: f32, on: bool, name: &str) {
    let mut harness = RenderHarness::new();
    harness.set_theme("Default Dark");
    harness.set_scale(scale);
    let img = harness.render(size(px(CHART_W), px(CHART_H)), colored(on, chart_root));
    assert_golden(name, &img);
}

fn check_grid(scale: f32, on: bool, name: &str) {
    let mut harness = RenderHarness::new();
    harness.set_theme("Default Dark");
    harness.set_scale(scale);
    let img = harness.render(
        size(px(GRID_W), px(grid_window_h())),
        colored(on, grid_root),
    );
    assert_golden(name, &img);
}

/// Value colours off: the flat golden from before the ramps (tasklens-j477q).
#[test]
fn render_chart_gap_and_spike_1_0() {
    run_on_lavapipe("render_chart::render_chart_gap_and_spike_1_0", || {
        check_chart(1.0, false, "chart_gap_spike_100")
    });
}

#[test]
fn render_chart_gap_and_spike_1_1() {
    run_on_lavapipe("render_chart::render_chart_gap_and_spike_1_1", || {
        check_chart(1.1, false, "chart_gap_spike_110")
    });
}

#[test]
fn render_cpu_grid_16_1_0() {
    run_on_lavapipe("render_chart::render_cpu_grid_16_1_0", || {
        check_grid(1.0, false, "cpu_grid_16_100")
    });
}

#[test]
fn render_cpu_grid_16_1_1() {
    run_on_lavapipe("render_chart::render_cpu_grid_16_1_1", || {
        check_grid(1.1, false, "cpu_grid_16_110")
    });
}

/// Value colours on: the CPU chart with its CPU ramp (tasklens-j477q).
#[test]
fn render_chart_gap_and_spike_ramp() {
    run_on_lavapipe("render_chart::render_chart_gap_and_spike_ramp", || {
        check_chart(1.0, true, "chart_gap_spike_ramp_100")
    });
}

/// Value colours on: every core cell with the CPU ramp, row by row (tasklens-j477q).
#[test]
fn render_cpu_grid_16_ramp() {
    run_on_lavapipe("render_chart::render_cpu_grid_16_ramp", || {
        check_grid(1.0, true, "cpu_grid_16_ramp_100")
    });
}

// Power draw charts with the value ramp (tasklens-1vwxr).

/// The power fixture: about 20 W for the older 250 samples, a climb over 50 samples, then about
/// 178 W, each with a small fixed wobble. Never above 180 W, so the open sticky axis ends at
/// 200 W as the GPU axis does.
fn power_values(scale: f32) -> Vec<f32> {
    (0..SAMPLES)
        .map(|i| {
            let wobble = 2.0 * (i as f32 * 0.21).sin();
            let base = match i {
                0..250 => 20.0,
                250..300 => 20.0 + (i - 250) as f32 / 50.0 * 158.0,
                _ => 178.0,
            };
            (scale * (base + wobble)).clamp(0.0, 180.0)
        })
        .collect()
}

fn metric(device: u16, metric: MetricId) -> SeriesId {
    SeriesId {
        device: DeviceIdx(device),
        metric,
    }
}

/// Power samples ending at `newest`, 100 ms apart, with no gap.
fn power_samples(values: &[f32], newest: u64) -> Arc<[Sample]> {
    let last = values.len() - 1;
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| Sample::valid(newest - (last - i) as u64 * PERIOD_NS, v))
        .collect()
}

fn power_labels() -> ChartLabels {
    ChartLabels {
        heading: Some("Power draw".into()),
        value_fmt: ValueFormat::Watts,
        value_align: ValueAlign::Right,
        range_word: "Power draw".into(),
        legend: SmallVec::new(),
        show_axis_max: true,
    }
}

fn power_grid() -> GridStyle {
    GridStyle {
        lines: true,
        baseline: true,
        outline: true,
        rounded: false,
        vertical_at: None,
    }
}

/// The CPU page's Power draw chart: package power filled, core power dashed, open sticky axis.
fn power_cpu_root(_: &mut Window, cx: &mut App) -> Entity<Root<ChartView>> {
    let graph = current(cx).graph;
    let spec = ChartSpec {
        series: smallvec![
            ChartSeries {
                id: metric(0, MetricId::CPU_POWER_PKG),
                fill: Some(graph.cpu_fill),
                line: graph.cpu_line,
                ..ChartSeries::default()
            },
            ChartSeries {
                id: metric(0, MetricId::CPU_POWER_CORE),
                fill: None,
                line: graph.kernel_line,
                dashed: true,
                ..ChartSeries::default()
            },
        ],
        y: YScale::StickyUp { hw_max: None },
        grid: power_grid(),
        ..ChartSpec::default()
    };
    let hub = hub(cx);
    let newest = newest_ns();
    let pkg = view(
        metric(0, MetricId::CPU_POWER_PKG),
        power_samples(&power_values(1.0), newest),
    );
    let cores = view(
        metric(0, MetricId::CPU_POWER_CORE),
        power_samples(&power_values(0.6), newest),
    );
    let chart = cx.new(|cx| {
        let mut v = ChartView::new(spec, power_labels(), &hub, cx);
        v.set_series_for_test(smallvec![Some(pkg), Some(cores)], cx);
        v
    });
    cx.new(|_| Root { child: chart })
}

/// The GPU page's Power chart: one filled series on a 200 W axis (the board's power limit).
fn power_gpu_root(_: &mut Window, cx: &mut App) -> Entity<Root<ChartView>> {
    let graph = current(cx).graph;
    let spec = ChartSpec {
        series: smallvec![ChartSeries {
            id: metric(1, MetricId::GPU_POWER),
            fill: Some(graph.gpu_fill),
            line: graph.gpu_line,
            ..ChartSeries::default()
        }],
        y: YScale::StickyUp {
            hw_max: Some(200.0),
        },
        grid: power_grid(),
        ..ChartSpec::default()
    };
    let hub = hub(cx);
    let s = view(
        metric(1, MetricId::GPU_POWER),
        power_samples(&power_values(1.0), newest_ns()),
    );
    let chart = cx.new(|cx| {
        let mut v = ChartView::new(spec, power_labels(), &hub, cx);
        v.set_series_for_test(smallvec![Some(s)], cx);
        v
    });
    cx.new(|_| Root { child: chart })
}

/// The pixel at plot row `watts` of a 200 W axis in logical column `x`. The image is in
/// device px, so logical px are scaled by the image width over `CHART_W`.
fn power_pixel(img: &RgbaImage, x: f32, watts: f32) -> [u8; 4] {
    let k = img.width() as f32 / CHART_W;
    // The plot is the 400 × 200 chart inset by its 1 px outline.
    let y = 1.0 + (1.0 - watts / 200.0) * 198.0;
    img.get_pixel((x * k).round() as u32, (y * k).round() as u32)
        .0
}

/// Renders a power chart, compares it with its golden and checks the ramp in the pixels: in a
/// column at about 178 W, the fill near 170 W is warm (more red than blue) and the fill near
/// 15 W is cool (more blue than red).
fn check_power(
    theme: &str,
    name: &str,
    root: fn(&mut Window, &mut App) -> Entity<Root<ChartView>>,
) {
    let mut harness = RenderHarness::new();
    harness.set_theme(theme);
    harness.set_scale(1.0);
    let img = harness.render(size(px(CHART_W), px(CHART_H)), root);
    assert_golden(name, &img);
    // Column 350 is in the newest 40 % of the plot, where the value is about 178 W.
    let warm = power_pixel(&img, 350.0, 170.0);
    let cool = power_pixel(&img, 350.0, 15.0);
    assert!(
        i32::from(warm[0]) > i32::from(warm[2]) + 40,
        "{name}: the fill near 170 W is warm: {warm:?}"
    );
    assert!(
        i32::from(cool[2]) > i32::from(cool[0]) + 20,
        "{name}: the fill near 15 W is cool: {cool:?}"
    );
}

#[test]
fn render_power_cpu_dark() {
    run_on_lavapipe("render_chart::render_power_cpu_dark", || {
        check_power("Default Dark", "power_cpu_dark", power_cpu_root)
    });
}

#[test]
fn render_power_gpu_dark() {
    run_on_lavapipe("render_chart::render_power_gpu_dark", || {
        check_power("Default Dark", "power_gpu_dark", power_gpu_root)
    });
}

#[test]
fn render_power_gpu_light() {
    run_on_lavapipe("render_chart::render_power_gpu_light", || {
        check_power("Default Light", "power_gpu_light", power_gpu_root)
    });
}

// One chart per value ramp kind (tasklens-j477q).

const GIB: f32 = (1u64 << 30) as f32;

/// The ramp fixture of a kind: about 10 % of `axis` for the older 250 samples, a climb, then
/// about 89 %: the power fixture's shape on another axis.
fn kind_values(axis: f32) -> Vec<f32> {
    power_values(1.0).iter().map(|w| w / 200.0 * axis).collect()
}

/// One filled series of `id` on `y` with the device's fill and line colours.
fn kind_root(
    id: SeriesId,
    y: YScale,
    axis: f32,
    fmt: ValueFormat,
    heading: &'static str,
    colors: fn(&GraphPalette) -> (gpui_kit::Hsla, gpui_kit::Hsla),
    cx: &mut App,
) -> Entity<Root<ChartView>> {
    set_value_colors(true, cx);
    let (fill, line) = colors(&current(cx).graph);
    let spec = ChartSpec {
        series: smallvec![ChartSeries {
            id,
            fill: Some(fill),
            line,
            ..ChartSeries::default()
        }],
        y,
        grid: power_grid(),
        ..ChartSpec::default()
    };
    let labels = ChartLabels {
        heading: Some(heading.into()),
        value_fmt: fmt,
        value_align: ValueAlign::Right,
        range_word: heading.into(),
        legend: SmallVec::new(),
        show_axis_max: true,
    };
    let hub = hub(cx);
    let s = view(id, power_samples(&kind_values(axis), newest_ns()));
    let chart = cx.new(|cx| {
        let mut v = ChartView::new(spec, labels, &hub, cx);
        v.set_series_for_test(smallvec![Some(s)], cx);
        v
    });
    cx.new(|_| Root { child: chart })
}

const PERCENT: YScale = YScale::Fixed {
    min: 0.0,
    max: 100.0,
};

fn gpu_root(_: &mut Window, cx: &mut App) -> Entity<Root<ChartView>> {
    let id = metric(1, MetricId::GPU_UTIL);
    kind_root(
        id,
        PERCENT,
        100.0,
        ValueFormat::Percent,
        "GPU",
        |g| (g.gpu_fill, g.gpu_line),
        cx,
    )
}

fn disk_root(_: &mut Window, cx: &mut App) -> Entity<Root<ChartView>> {
    let id = metric(2, MetricId::DISK_ACTIVE);
    kind_root(
        id,
        PERCENT,
        100.0,
        ValueFormat::Percent,
        "Active time",
        |g| (g.disk_fill, g.disk_line),
        cx,
    )
}

fn mem_root(_: &mut Window, cx: &mut App) -> Entity<Root<ChartView>> {
    let id = metric(3, MetricId::MEM_USED);
    kind_root(
        id,
        YScale::FixedTotal(16.0 * GIB),
        16.0 * GIB,
        ValueFormat::Bytes,
        "Memory usage",
        |g| (g.mem_fill, g.mem_line),
        cx,
    )
}

/// The pixel at `frac` of the axis height in logical column `x`, in device px.
fn axis_pixel(img: &RgbaImage, x: f32, frac: f32) -> [u8; 4] {
    let k = img.width() as f32 / CHART_W;
    let y = 1.0 + (1.0 - frac) * 198.0;
    img.get_pixel((x * k).round() as u32, (y * k).round() as u32)
        .0
}

/// Renders a kind chart, compares it with its golden and checks the ramp in the pixels: in the
/// column at about 89 %, the fill near 85 % is near the theme's hot colour (red is its largest
/// channel) and the fill near 5 % is nearer the kind's calm colour than the hot one.
fn check_kind(
    theme: &str,
    name: &str,
    kind: ValueRampKind,
    root: fn(&mut Window, &mut App) -> Entity<Root<ChartView>>,
) {
    let mut harness = RenderHarness::new();
    harness.set_theme(theme);
    harness.set_scale(1.0);
    let img = harness.render(size(px(CHART_W), px(CHART_H)), root);
    assert_golden(name, &img);
    let hot = axis_pixel(&img, 350.0, 0.85);
    let cool = axis_pixel(&img, 350.0, 0.05);
    assert!(
        hot[0] > hot[1] && hot[0] > hot[2],
        "{name}: the fill near 85 % is hot: {hot:?}"
    );
    let ramp = harness_ramp(theme, kind);
    let d = |p: [u8; 4], c: gpui_kit::Rgba| {
        let f = |v: u8| f32::from(v) / 255.0;
        (f(p[0]) - c.r).abs() + (f(p[1]) - c.g).abs() + (f(p[2]) - c.b).abs()
    };
    assert!(
        d(cool, ramp[0]) < d(cool, ramp[2]),
        "{name}: the fill near 5 % is calm: {cool:?}"
    );
}

/// The ramp of `kind` in built-in theme `theme` as the fill shows it: each colour at
/// `FILL_ALPHA` over the theme's `chart_bg`, in sRGB.
fn harness_ramp(theme: &str, kind: ValueRampKind) -> [gpui_kit::Rgba; 3] {
    let graph = tl_ui_base::theme::builtin::load()
        .iter()
        .find(|d| &*d.name == theme)
        .and_then(|d| d.graph)
        .expect("a built-in theme with graph colours");
    let bg: gpui_kit::Rgba = graph.chart_bg.into();
    let a = tl_ui_chart::chart::ramp::FILL_ALPHA;
    graph.value_ramp(kind).map(|c| {
        let c: gpui_kit::Rgba = c.into();
        let l = |x: f32, y: f32| y + (x - y) * a;
        gpui_kit::Rgba {
            r: l(c.r, bg.r),
            g: l(c.g, bg.g),
            b: l(c.b, bg.b),
            a: 1.0,
        }
    })
}

#[test]
fn render_ramp_gpu_dark() {
    run_on_lavapipe("render_chart::render_ramp_gpu_dark", || {
        check_kind(
            "Default Dark",
            "ramp_gpu_dark",
            ValueRampKind::Gpu,
            gpu_root,
        )
    });
}

#[test]
fn render_ramp_disk_dark() {
    run_on_lavapipe("render_chart::render_ramp_disk_dark", || {
        check_kind(
            "Default Dark",
            "ramp_disk_dark",
            ValueRampKind::Disk,
            disk_root,
        )
    });
}

#[test]
fn render_ramp_mem_dark() {
    run_on_lavapipe("render_chart::render_ramp_mem_dark", || {
        check_kind(
            "Default Dark",
            "ramp_mem_dark",
            ValueRampKind::Memory,
            mem_root,
        )
    });
}

#[test]
fn render_ramp_mem_light() {
    run_on_lavapipe("render_chart::render_ramp_mem_light", || {
        check_kind(
            "Default Light",
            "ramp_mem_light",
            ValueRampKind::Memory,
            mem_root,
        )
    });
}
