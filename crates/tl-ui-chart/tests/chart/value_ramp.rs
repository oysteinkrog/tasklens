//! One-series charts on a fixed scale fill and draw series 0 with their device's value ramp,
//! and `appearance.color_charts_by_value` turns every ramp off, power included
//! (tasklens-j477q).

use std::sync::Arc;

use gpui_kit::TestAppContext;
use smallvec::smallvec;
use tl_model::client::EngineClient;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::theme::{GraphPalette, ValueRampKind, current};
use tl_ui_chart::chart::ramp::FILL_ALPHA;
use tl_ui_chart::chart::{ChartSeries, ChartSpec, ValueFormat, YScale};
use tl_ui_settings::settings::{Settings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

use crate::power_ramp::{
    PKG, color_at, dist, draw_chart, ramps, redraw, row_of, solid, spec, step, with_alpha,
};

/// Installs the settings global with `appearance.color_charts_by_value = on`. Without the
/// global the ramps are on, as in a fresh config.
pub(crate) fn value_colors(cx: &mut TestAppContext, on: bool) {
    cx.update(|cx| {
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
    });
}

const PERCENT: YScale = YScale::Fixed {
    min: 0.0,
    max: 100.0,
};

fn id(metric: MetricId) -> SeriesId {
    SeriesId {
        device: DeviceIdx(0),
        metric,
    }
}

/// A one-series chart of `metric` on `y`: filled series 0 in the CPU colours, plus the solid
/// kernel line when `kernel`, as the CPU total charts draw it.
fn one_series(metric: MetricId, y: YScale, kernel: bool, g: &GraphPalette) -> ChartSpec {
    let mut s = spec(y, false, g);
    s.series[0] = ChartSeries {
        id: id(metric),
        fill: Some(g.cpu_fill),
        line: g.cpu_line,
        ..ChartSeries::default()
    };
    if kernel {
        s.series.push(ChartSeries {
            id: id(MetricId::CPU_KERNEL),
            fill: None,
            line: g.kernel_line,
            ..ChartSeries::default()
        });
    }
    s
}

/// The acceptance case: a CPU chart at 95 % has the CPU hot colour at the top of its fill. The
/// fill and the line each paint once per plot half; the kernel line keeps its colour.
#[gpui_kit::test]
fn cpu_chart_at_95_pct_is_hot_at_the_top(cx: &mut TestAppContext) {
    let (chart, vcx) = draw_chart(
        cx,
        |g| one_series(MetricId::CPU_TOTAL, PERCENT, true, g),
        ValueFormat::Percent,
        smallvec![
            Some(step(id(MetricId::CPU_TOTAL), 10.0, 95.0)),
            Some(step(id(MetricId::CPU_KERNEL), 5.0, 30.0)),
        ],
    );
    let g = vcx.update(|_, cx| current(cx).graph);
    let [low, mid, high] = g.value_ramp(ValueRampKind::Cpu);
    let bands = ramps(&chart, vcx);
    assert_eq!(
        bands.len(),
        4,
        "fill and line, each in two halves: {bands:?}"
    );
    assert_eq!(
        solid(&chart, vcx),
        vec![g.kernel_line],
        "the kernel line is flat"
    );

    let fa = FILL_ALPHA * g.cpu_fill.a;
    let (fill, line) = (&bands[..2], &bands[2..]);
    assert_eq!(fill[0].from, with_alpha(low, fa));
    assert_eq!(fill[0].to, with_alpha(mid, fa));
    assert_eq!(
        fill[1].to,
        with_alpha(high, fa),
        "the top of the fill is hot"
    );
    assert_eq!([line[0].from, line[0].to, line[1].to], [low, mid, high]);

    // At 95 % the fill is next to the hot colour and far from the calm one.
    let top = color_at(fill, row_of(95.0, 100.0));
    assert!(dist(top, high) < 0.1, "fill at 95 %: {top:?}, hot {high:?}");
    assert!(dist(top, low) > 0.3, "fill at 95 % is not calm: {top:?}");
    let bottom = color_at(fill, row_of(5.0, 100.0));
    assert!(dist(bottom, low) < 0.1, "fill at 5 %: {bottom:?}");
}

/// GPU utilisation and engines, disk active time and memory in use each take their own ramp;
/// memory's axis is its total, not 100 %.
#[gpui_kit::test]
fn each_kind_uses_its_own_ramp(cx: &mut TestAppContext) {
    let gib = (1u64 << 30) as f32;
    let cases = [
        (MetricId::GPU_UTIL, ValueRampKind::Gpu, PERCENT, 90.0),
        (
            MetricId::GPU_ENGINE_VIDEO,
            ValueRampKind::Gpu,
            PERCENT,
            90.0,
        ),
        (MetricId::DISK_ACTIVE, ValueRampKind::Disk, PERCENT, 90.0),
        (
            MetricId::MEM_USED,
            ValueRampKind::Memory,
            YScale::FixedTotal(16.0 * gib),
            14.0 * gib,
        ),
    ];
    let (chart, vcx) = draw_chart(
        cx,
        |g| one_series(MetricId::GPU_UTIL, PERCENT, false, g),
        ValueFormat::Percent,
        smallvec![Some(step(id(MetricId::GPU_UTIL), 10.0, 90.0))],
    );
    let g = vcx.update(|_, cx| current(cx).graph);
    let mut seen = Vec::new();
    for (metric, kind, y, high_value) in cases {
        chart.update(vcx, |v, cx| {
            v.set_spec(one_series(metric, y, false, &g), cx);
            v.set_series_for_test(smallvec![Some(step(id(metric), 0.0, high_value))], cx);
        });
        redraw(&chart, vcx);
        let bands = ramps(&chart, vcx);
        assert_eq!(bands.len(), 4, "{metric:?}: {bands:?}");
        let line = &bands[2..];
        assert_eq!(
            [line[0].from, line[0].to, line[1].to],
            g.value_ramp(kind),
            "{metric:?}"
        );
        assert!(solid(&chart, vcx).is_empty(), "{metric:?}");
        seen.push(g.value_ramp(kind)[0]);
    }
    seen.dedup();
    assert_eq!(seen.len(), 3, "GPU, disk and memory start in their own hue");
}

/// Charts without a single value series on a fixed scale stay flat: all cores overlaid (two
/// series of one metric), a stacked chart, a rate chart on a moving scale and a percent chart
/// of a metric without a ramp.
#[gpui_kit::test]
fn overlaid_stacked_and_moving_charts_stay_flat(cx: &mut TestAppContext) {
    let (chart, vcx) = draw_chart(
        cx,
        |g| one_series(MetricId::CPU_TOTAL, PERCENT, false, g),
        ValueFormat::Percent,
        smallvec![Some(step(id(MetricId::CPU_TOTAL), 10.0, 90.0))],
    );
    let g = vcx.update(|_, cx| current(cx).graph);
    assert_eq!(ramps(&chart, vcx).len(), 4, "one CPU series ramps");

    let core = |i: u16| SeriesId {
        device: DeviceIdx(i),
        metric: MetricId::CPU_TOTAL,
    };
    let mut overlaid = one_series(MetricId::CPU_TOTAL, PERCENT, false, &g);
    overlaid.series[0].id = core(1);
    overlaid.series.push(ChartSeries {
        id: core(2),
        fill: None,
        line: g.gpu_line,
        ..ChartSeries::default()
    });
    let mut stacked = one_series(MetricId::CPU_TOTAL, YScale::Stacked(1), false, &g);
    stacked.stack = true;
    let rate = one_series(
        MetricId::DISK_READ,
        YScale::AutoRate { iec: true },
        false,
        &g,
    );
    let freq = one_series(MetricId::CPU_FREQ, PERCENT, false, &g);
    for (name, spec) in [
        ("overlaid", overlaid),
        ("stacked", stacked),
        ("rate", rate),
        ("no ramp", freq),
    ] {
        let views = spec
            .series
            .iter()
            .map(|s| Some(step(s.id, 10.0, 90.0)))
            .collect();
        chart.update(vcx, |v, cx| {
            v.set_spec(spec, cx);
            v.set_series_for_test(views, cx);
        });
        redraw(&chart, vcx);
        assert!(ramps(&chart, vcx).is_empty(), "{name} is flat");
        assert!(!solid(&chart, vcx).is_empty(), "{name} paints flat paths");
    }
}

/// With the setting off, a CPU chart and a power chart draw their flat colours, as before the
/// ramps. Turning it on colours them at the next paint, with no new chart.
#[gpui_kit::test]
fn setting_off_draws_flat_colours(cx: &mut TestAppContext) {
    value_colors(cx, false);
    let (chart, vcx) = draw_chart(
        cx,
        |g| one_series(MetricId::CPU_TOTAL, PERCENT, false, g),
        ValueFormat::Percent,
        smallvec![Some(step(id(MetricId::CPU_TOTAL), 10.0, 95.0))],
    );
    let g = vcx.update(|_, cx| current(cx).graph);
    assert!(ramps(&chart, vcx).is_empty());
    assert_eq!(solid(&chart, vcx), vec![g.cpu_fill, g.cpu_line]);

    // A power chart follows the same setting.
    chart.update(vcx, |v, cx| {
        v.set_labels(crate::power_ramp::labels(ValueFormat::Watts), cx);
        v.set_spec(
            spec(
                YScale::StickyUp {
                    hw_max: Some(200.0),
                },
                false,
                &g,
            ),
            cx,
        );
        v.set_series_for_test(smallvec![Some(step(PKG, 20.0, 180.0))], cx);
    });
    redraw(&chart, vcx);
    assert!(ramps(&chart, vcx).is_empty(), "power is flat too");
    assert_eq!(solid(&chart, vcx), vec![g.gpu_fill, g.gpu_line]);

    vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.appearance.color_charts_by_value = true)
    });
    redraw(&chart, vcx);
    assert_eq!(ramps(&chart, vcx).len(), 4, "on again: the power ramp");
    assert!(solid(&chart, vcx).is_empty());
}
