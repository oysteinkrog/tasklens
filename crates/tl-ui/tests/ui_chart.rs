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

// Grid, baseline and dotted gap quads (E19, tasklens-yrf.8).

mod grid {
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{
        App, Bounds, Context, IntoElement, PaintQuad, Pixels, Quad, Render, TestAppContext, Window,
        canvas, point, px, size,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_ui::chart::GridStyle;
    use tl_ui::chart::geometry::TimeMap;
    use tl_ui::chart::grid::{GridPaint, grid_k_range, paint_baseline, paint_gaps, paint_grid};
    use tl_ui::theme::{TlTheme, current};

    const S: u64 = 1_000_000_000;

    fn theme(cx: &mut TestAppContext) -> Arc<TlTheme> {
        cx.update(|cx: &mut App| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            current(cx).clone()
        })
    }

    /// 60 s range, right edge at 123.4 s, anchored at the right edge.
    fn map(plot_width: f32) -> TimeMap {
        TimeMap {
            anchor_ns: 123_400_000_000,
            right_edge_ns: 123_400_000_000,
            range_ns: 60 * S,
            plot_width,
        }
    }

    fn style() -> GridStyle {
        GridStyle {
            lines: true,
            baseline: true,
            outline: true,
            rounded: false,
        }
    }

    fn grid_paint<'a>(
        plot: Bounds<Pixels>,
        map: &'a TimeMap,
        dx: f32,
        scale_factor: f32,
        theme: &'a TlTheme,
    ) -> GridPaint<'a> {
        GridPaint {
            plot,
            box_bounds: plot,
            map,
            dx: px(dx),
            scale_factor,
            theme,
            style: style(),
            outline_color: theme.accent,
            radius: px(0.),
        }
    }

    struct Painter(Rc<dyn Fn(&mut Window)>);

    impl Render for Painter {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let paint = self.0.clone();
            canvas(|_, _, _| (), move |_, _, window, _| paint(window))
        }
    }

    /// Paints `f` in a headless window at `scale` and returns the frame's quads.
    fn painted(
        cx: &mut TestAppContext,
        scale: f32,
        f: impl Fn(&mut Window) + 'static,
    ) -> Vec<Quad> {
        let (_, cx) = cx.add_window_view(|_, _| Painter(Rc::new(f)));
        cx.simulate_scale_factor_change(scale);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.painted_quads()
        })
    }

    #[test]
    fn grid_k_range_covers_window() {
        let (first, last) = grid_k_range(&map(600.));
        assert_eq!((first, last), (11, 20));
        assert_eq!(first * 6 * S, 66 * S);
        assert_eq!(last * 6 * S, 120 * S);
    }

    #[gpui_kit::test]
    fn grid_quads_at_quarters(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let grid = theme.graph.grid;
        let quads = painted(cx, 1.0, move |window| {
            let m = map(600.);
            let plot = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(300.)));
            paint_grid(window, &grid_paint(plot, &m, m.dx(600.), 1.0, &theme));
        });
        let mut rows: Vec<f32> = quads
            .iter()
            .filter(|q| q.background == grid.into())
            .filter(|q| q.bounds.size.height.0 == 1.0 && q.bounds.size.width.0 == 600.0)
            .map(|q| q.bounds.origin.y.0)
            .collect();
        rows.sort_by(f32::total_cmp);
        assert_eq!(rows, [75.0, 150.0, 225.0]);
        let columns = quads
            .iter()
            .filter(|q| q.bounds.size.width.0 == 1.0 && q.bounds.size.height.0 == 300.0)
            .count();
        assert!((10..=11).contains(&columns), "{columns} vertical lines");
        assert_eq!(quads.len(), 3 + columns);
    }

    #[gpui_kit::test]
    fn no_grid_under_40px(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let m = map(600.);
        let plot = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(32.)));
        let mut quads: Vec<PaintQuad> = Vec::new();
        paint_grid(&mut quads, &grid_paint(plot, &m, m.dx(600.), 1.0, &theme));
        assert!(quads.is_empty());

        let card = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(56.)));
        paint_grid(&mut quads, &grid_paint(card, &m, m.dx(600.), 1.0, &theme));
        assert!(!quads.is_empty(), "sidebar cards keep the grid");
    }

    #[gpui_kit::test]
    fn gap_quads_every_3px(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let color = theme.accent.alpha(0.40);
        let quads = painted(cx, 1.0, move |window| {
            let m = map(600.);
            let plot = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(300.)));
            paint_gaps(
                window,
                &grid_paint(plot, &m, 0., 1.0, &theme),
                &[100.0..130.0],
            );
        });
        let xs: Vec<f32> = quads.iter().map(|q| q.bounds.origin.x.0).collect();
        let expected: Vec<f32> = (0..10).map(|i| 100.0 + 3.0 * i as f32).collect();
        assert_eq!(xs, expected);
        for q in &quads {
            assert_eq!((q.bounds.size.width.0, q.bounds.size.height.0), (1.0, 1.0));
            assert_eq!(q.bounds.origin.y.0, 299.0);
            assert_eq!(q.background, color.into());
        }
    }

    #[gpui_kit::test]
    fn hairline_is_one_physical_px(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let m = map(660.);
        let plot = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(300.)));
        let mut quads: Vec<PaintQuad> = Vec::new();
        paint_baseline(&mut quads, &grid_paint(plot, &m, 0., 1.1, &theme));
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].bounds.size.height, px(1.0 / 1.1));
        assert_eq!(quads[0].bounds.origin.y, px(300.) - px(1.0 / 1.1));

        let painted = painted(cx, 1.1, move |window| {
            let m = map(660.);
            paint_baseline(window, &grid_paint(plot, &m, 0., 1.1, &theme));
        });
        assert_eq!(painted.len(), 1);
        assert_eq!(painted[0].bounds.size.height.0, 1.0, "1 physical px");
    }
}
