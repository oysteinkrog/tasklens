//! ChartView construction test with `TestAppContext` (E19, ADR-013). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use gpui_kit::{AppContext, TestAppContext};
use smallvec::smallvec;
use tl_ui::chart::view::ChartView;
use tl_ui::chart::{ChartLabels, ChartSeries, ChartSpec, ValueAlign, ValueFormat};

fn labels() -> ChartLabels {
    ChartLabels {
        heading: Some("CPU".into()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: "Utilization",
        legend: smallvec![],
        show_axis_max: true,
    }
}

#[gpui_kit::test]
fn chart_view_constructs(cx: &mut TestAppContext) {
    let spec = ChartSpec {
        series: smallvec![ChartSeries::default()],
        ..Default::default()
    };
    let view = cx.new(|cx| ChartView::new(spec, labels(), cx));
    view.read_with(cx, |v, _| {
        assert_eq!(v.render_count.get(), 0);
        assert_eq!(v.cached_path_count(), 0);
        assert!(v.cache_key_is_default());
        assert_eq!(v.series_ids().count(), 1);
    });
}
