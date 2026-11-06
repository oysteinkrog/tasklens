//! Power draw charts fill and draw series 0 with the theme's value ramp (tasklens-1vwxr).

use std::sync::Arc;

use gpui_kit::{
    AppContext, Context, Entity, Hsla, IntoElement, ParentElement, Render, Rgba, StyleRefinement,
    Styled, TestAppContext, VisualTestContext, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::client::EngineClient;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::time::now_boot_ns;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::theme::store::{self, ActivateSource};
use tl_ui_base::theme::{GraphPalette, current};
use tl_ui_chart::chart::ramp::{FILL_ALPHA, RampBand};
use tl_ui_chart::chart::{
    ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat, YScale,
};
use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};
use tl_ui_testkit::fake_engine::FakeEngine;

const S: u64 = 1_000_000_000;

pub(crate) const PKG: SeriesId = SeriesId {
    device: DeviceIdx(0),
    metric: MetricId::CPU_POWER_PKG,
};
const CORES: SeriesId = SeriesId {
    device: DeviceIdx(0),
    metric: MetricId::CPU_POWER_CORE,
};

/// Chart size in logical px. The plot is the chart inset by its 1 px outline: 1..399 × 1..199.
const W: f32 = 400.;
const H: f32 = 200.;
const PLOT_TOP: f32 = 1.;
const PLOT_BOTTOM: f32 = 199.;

pub(crate) fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
    });
}

pub(crate) fn labels(fmt: ValueFormat) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: fmt,
        value_align: ValueAlign::Hidden,
        range_word: "Power draw".into(),
        legend: smallvec![],
        show_axis_max: false,
    }
}

/// A power chart over `y`: series 0 filled with the theme's GPU colours, plus a dashed second
/// series when `cores`, as the CPU page's Power draw chart.
pub(crate) fn spec(y: YScale, cores: bool, g: &GraphPalette) -> ChartSpec {
    let mut series: SmallVec<[ChartSeries; 4]> = smallvec![ChartSeries {
        id: PKG,
        fill: Some(g.gpu_fill),
        line: g.gpu_line,
        ..ChartSeries::default()
    }];
    if cores {
        series.push(ChartSeries {
            id: CORES,
            fill: None,
            line: g.kernel_line,
            dashed: true,
            ..ChartSeries::default()
        });
    }
    ChartSpec {
        series,
        y,
        grid: GridStyle {
            lines: true,
            baseline: true,
            outline: true,
            rounded: false,
            vertical_at: None,
        },
        ..ChartSpec::default()
    }
}

/// 61 samples 1 s apart ending now: the older half at `low` W, the newer half at `high` W.
pub(crate) fn step(id: SeriesId, low: f32, high: f32) -> Arc<SeriesView> {
    let newest = now_boot_ns();
    let samples = (0..61u64)
        .map(|i| Sample::valid(newest - (60 - i) * S, if i < 30 { low } else { high }))
        .collect();
    Arc::new(SeriesView {
        id,
        version: 1,
        period_ns: S,
        samples,
    })
}

struct Root {
    _driver: Entity<AnimationDriver>,
    chart: Entity<ChartView>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(
            div().w(px(W)).h(px(H)).flex_none().child(
                self.chart
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
        )
    }
}

/// One chart in a window at scale 1, with `series` bound and drawn once.
pub(crate) fn draw_chart(
    cx: &mut TestAppContext,
    make_spec: impl FnOnce(&GraphPalette) -> ChartSpec,
    fmt: ValueFormat,
    series: SmallVec<[Option<Arc<SeriesView>>; 2]>,
) -> (Entity<ChartView>, &mut VisualTestContext) {
    init(cx);
    let (root, vcx) = cx.add_window_view(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        let client: Arc<dyn EngineClient> = FakeEngine::new();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        let spec = make_spec(&current(cx).graph);
        let chart = cx.new(|cx| ChartView::new(spec, labels(fmt), &hub, cx));
        Root {
            _driver: driver,
            chart,
        }
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.run_until_parked();
    let chart = vcx.read_entity(&root, |r, _| r.chart.clone());
    chart.update(vcx, |v, cx| v.set_series_for_test(series, cx));
    redraw(&chart, vcx);
    (chart, vcx)
}

pub(crate) fn redraw(chart: &Entity<ChartView>, vcx: &mut VisualTestContext) {
    chart.update(vcx, |_, cx| cx.notify());
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

pub(crate) fn ramps(chart: &Entity<ChartView>, vcx: &mut VisualTestContext) -> Vec<RampBand> {
    vcx.read_entity(chart, |v, _| v.path_ramps.borrow().to_vec())
}

pub(crate) fn solid(chart: &Entity<ChartView>, vcx: &mut VisualTestContext) -> Vec<Hsla> {
    vcx.read_entity(chart, |v, _| v.path_colors.borrow().to_vec())
}

pub(crate) fn rgba(c: Hsla) -> Rgba {
    c.into()
}

/// The colour a band's gradient gives at row `y` (logical px), mixed in sRGB as the GPUI path
/// shader does: `from` at the bottom of the band, `to` at its top.
pub(crate) fn band_color_at(band: &RampBand, y: f32) -> Rgba {
    let (top, bottom) = (
        f32::from(band.bounds.top()),
        f32::from(band.bounds.bottom()),
    );
    let t = ((bottom - y) / (bottom - top)).clamp(0.0, 1.0);
    let (a, b) = (rgba(band.from), rgba(band.to));
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    }
}

/// The colour of row `y` from the band that holds it.
pub(crate) fn color_at(bands: &[RampBand], y: f32) -> Rgba {
    let band = bands
        .iter()
        .find(|b| f32::from(b.bounds.top()) <= y && y <= f32::from(b.bounds.bottom()))
        .unwrap_or_else(|| panic!("no band holds row {y}: {bands:?}"));
    band_color_at(band, y)
}

/// The plot row of `watts` on a 0..`axis` W axis.
pub(crate) fn row_of(watts: f32, axis: f32) -> f32 {
    PLOT_TOP + (1.0 - watts / axis) * (PLOT_BOTTOM - PLOT_TOP)
}

pub(crate) fn dist(a: Rgba, b: Hsla) -> f32 {
    let b = rgba(b);
    ((a.r - b.r).powi(2) + (a.g - b.g).powi(2) + (a.b - b.b).powi(2)).sqrt()
}

pub(crate) fn with_alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

/// Values from 20 W to 180 W on a 200 W axis: the fill near 180 W is in the warm colour and
/// near 20 W in the cool colour. Fill and line each paint once per half of the plot.
#[gpui_kit::test]
fn power_fill_cool_at_20_w_warm_at_180_w(cx: &mut TestAppContext) {
    let (chart, vcx) = draw_chart(
        cx,
        |g| {
            spec(
                YScale::StickyUp {
                    hw_max: Some(200.0),
                },
                false,
                g,
            )
        },
        ValueFormat::Watts,
        smallvec![Some(step(PKG, 20.0, 180.0))],
    );
    let g = vcx.update(|_, cx| current(cx).graph);
    let bands = ramps(&chart, vcx);
    assert_eq!(
        bands.len(),
        4,
        "fill and line, each in two halves: {bands:?}"
    );
    assert!(solid(&chart, vcx).is_empty(), "no flat-coloured path");

    let (fill, line) = (&bands[..2], &bands[2..]);
    for half in [fill, line] {
        assert_eq!(
            half[0].bounds.bottom(),
            px(PLOT_BOTTOM),
            "lower half ends at 0 W"
        );
        assert_eq!(
            half[1].bounds.top(),
            px(PLOT_TOP),
            "upper half ends at the axis max"
        );
        assert_eq!(half[0].bounds.top(), half[1].bounds.bottom());
        assert_eq!(half[0].bounds.size.width, px(W - 2.));
    }
    let fa = FILL_ALPHA * g.gpu_fill.a;
    assert_eq!(fill[0].from, with_alpha(g.power_low, fa));
    assert_eq!(fill[0].to, with_alpha(g.power_mid, fa));
    assert_eq!(fill[1].from, with_alpha(g.power_mid, fa));
    assert_eq!(fill[1].to, with_alpha(g.power_high, fa));
    assert_eq!(line[0].from, g.power_low, "the line takes the full colours");
    assert_eq!(line[1].to, g.power_high);

    let hot = color_at(fill, row_of(180.0, 200.0));
    let cool = color_at(fill, row_of(20.0, 200.0));
    assert!(
        dist(hot, g.power_high) < dist(hot, g.power_low),
        "180 W is warm: {hot:?}"
    );
    assert!(
        dist(hot, g.power_high) < 0.25,
        "180 W is near power-high: {hot:?}"
    );
    assert!(
        dist(cool, g.power_low) < dist(cool, g.power_high),
        "20 W is cool: {cool:?}"
    );
    assert!(
        dist(cool, g.power_low) < 0.25,
        "20 W is near power-low: {cool:?}"
    );
}

/// The gradient stops follow the axis max: the bands still span 0 W to the axis max, so on a
/// 400 W axis 180 W sits below the middle, near the middle colour and not the warm one.
#[gpui_kit::test]
fn power_ramp_follows_axis_max(cx: &mut TestAppContext) {
    let (chart, vcx) = draw_chart(
        cx,
        |g| {
            spec(
                YScale::StickyUp {
                    hw_max: Some(400.0),
                },
                false,
                g,
            )
        },
        ValueFormat::Watts,
        smallvec![Some(step(PKG, 20.0, 180.0))],
    );
    let g = vcx.update(|_, cx| current(cx).graph);
    let bands = ramps(&chart, vcx);
    assert_eq!(
        bands.len(),
        2,
        "180 W of 400 W stays in the lower half: {bands:?}"
    );
    let lower = &bands[0];
    assert_eq!(lower.bounds.bottom(), px(PLOT_BOTTOM));
    let split = f32::from(lower.bounds.top());
    assert!(
        (split - (PLOT_TOP + PLOT_BOTTOM) / 2.0).abs() <= 0.5,
        "split at 200 W: {split}"
    );
    let at_180 = band_color_at(lower, row_of(180.0, 400.0));
    assert!(
        dist(at_180, g.power_mid) < dist(at_180, g.power_high)
            && dist(at_180, g.power_mid) < dist(at_180, g.power_low),
        "180 W of 400 W is nearest the middle colour: {at_180:?}"
    );
    let on_200 = color_at(&ramps_on_200(&g), row_of(180.0, 200.0));
    assert!(
        dist(on_200, g.power_high) < dist(at_180, g.power_high),
        "the same 180 W is warmer on a 200 W axis"
    );
}

/// The two fill bands a 200 W chart of the same plot draws, from the ramp itself.
fn ramps_on_200(g: &GraphPalette) -> [RampBand; 2] {
    let plot = gpui_kit::Bounds::new(
        gpui_kit::point(px(PLOT_TOP), px(PLOT_TOP)),
        gpui_kit::size(px(W - 2.), px(PLOT_BOTTOM - PLOT_TOP)),
    );
    tl_ui_chart::chart::ValueRamp {
        low: g.power_low,
        mid: g.power_mid,
        high: g.power_high,
    }
    .bands(plot, 1.0, 1.0)
}

/// A chart that stays in the lower half paints each ramped path once.
#[gpui_kit::test]
fn low_power_paints_lower_half_only(cx: &mut TestAppContext) {
    let (chart, vcx) = draw_chart(
        cx,
        |g| {
            spec(
                YScale::StickyUp {
                    hw_max: Some(200.0),
                },
                false,
                g,
            )
        },
        ValueFormat::Watts,
        smallvec![Some(step(PKG, 20.0, 40.0))],
    );
    let bands = ramps(&chart, vcx);
    assert_eq!(bands.len(), 2, "{bands:?}");
    assert!(bands.iter().all(|b| b.bounds.bottom() == px(PLOT_BOTTOM)));
    let before = vcx.read_entity(&chart, |v, _| v.path_paints.get());
    redraw(&chart, vcx);
    let paints = vcx.read_entity(&chart, |v, _| v.path_paints.get()) - before;
    assert_eq!(paints, 2, "one paint for the fill, one for the line");
}

/// The CPU page's Power draw chart: package power takes the ramp, the dashed core power line
/// keeps its own colour. The open sticky axis ends at 200 W for a 180 W peak.
#[gpui_kit::test]
fn cpu_power_dashed_cores_keep_colour(cx: &mut TestAppContext) {
    let (chart, vcx) = draw_chart(
        cx,
        |g| spec(YScale::StickyUp { hw_max: None }, true, g),
        ValueFormat::Watts,
        smallvec![Some(step(PKG, 20.0, 180.0)), Some(step(CORES, 10.0, 120.0))],
    );
    let g = vcx.update(|_, cx| current(cx).graph);
    assert_eq!(ramps(&chart, vcx).len(), 4);
    assert_eq!(
        solid(&chart, vcx),
        vec![g.kernel_line],
        "the dashed line is flat"
    );
}

/// Each built-in theme's power chart uses that theme's three power tokens.
#[gpui_kit::test]
fn each_builtin_theme_uses_its_power_tokens(cx: &mut TestAppContext) {
    let (chart, vcx) = draw_chart(
        cx,
        |g| {
            spec(
                YScale::StickyUp {
                    hw_max: Some(200.0),
                },
                false,
                g,
            )
        },
        ValueFormat::Watts,
        smallvec![Some(step(PKG, 20.0, 180.0))],
    );
    let mut seen = Vec::new();
    for name in ["Default Dark", "Default Light"] {
        let g = vcx.update(|_, cx| {
            store::activate(name, ActivateSource::User, cx).expect("built-in theme");
            current(cx).graph
        });
        redraw(&chart, vcx);
        let bands = ramps(&chart, vcx);
        assert_eq!(bands.len(), 4, "{name}");
        let line = &bands[2..];
        assert_eq!(
            [line[0].from, line[0].to, line[1].to],
            g.power_ramp(),
            "{name} line"
        );
        assert_eq!(
            bands[0].from,
            with_alpha(g.power_low, FILL_ALPHA * g.gpu_fill.a)
        );
        seen.push(g.power_ramp());
    }
    assert_ne!(seen[0], seen[1], "dark and light have their own ramps");
}

/// Charts that are not power draw and have no value ramp keep their flat colours: a percent
/// chart of a metric without a ramp, and a battery's charge and discharge chart in watts.
#[gpui_kit::test]
fn percent_and_battery_charts_keep_flat_colours(cx: &mut TestAppContext) {
    let (chart, vcx) = draw_chart(
        cx,
        |g| {
            spec(
                YScale::Fixed {
                    min: 0.0,
                    max: 100.0,
                },
                false,
                g,
            )
        },
        ValueFormat::Percent,
        smallvec![Some(step(PKG, 20.0, 80.0))],
    );
    let g = vcx.update(|_, cx| current(cx).graph);
    assert!(ramps(&chart, vcx).is_empty());
    assert_eq!(solid(&chart, vcx), vec![g.gpu_fill, g.gpu_line]);

    chart.update(vcx, |v, cx| {
        v.set_labels(labels(ValueFormat::Watts), cx);
        v.set_spec(spec(YScale::Symmetric, false, &g), cx);
        v.set_series_for_test(smallvec![Some(step(PKG, -20.0, 30.0))], cx);
    });
    redraw(&chart, vcx);
    assert!(
        ramps(&chart, vcx).is_empty(),
        "battery power keeps its colour"
    );
    assert!(solid(&chart, vcx).contains(&g.gpu_line));
}
