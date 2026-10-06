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

// ChartElement paint plan (E19, tasklens-yrf.9).

mod element {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext, Context, Entity, Hsla, IntoElement, ParentElement, Pixels, Render, Styled,
        TestAppContext, VisualTestContext, Window, div, px,
    };
    use smallvec::{SmallVec, smallvec};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::time::now_boot_ns;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::chart::view::ChartView;
    use tl_ui::chart::{ChartSeries, ChartSpec, ChartState};

    const MS: u64 = 1_000_000;
    const S: u64 = 1_000_000_000;

    /// Root view that owns the window's driver and lays the chart out at `width × 202`.
    struct Root {
        driver: Entity<AnimationDriver>,
        chart: Entity<ChartView>,
        width: Pixels,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(self.width).h(px(202.)).child(self.chart.clone())
        }
    }

    struct Rig<'a> {
        root: Entity<Root>,
        chart: Entity<ChartView>,
        driver: Entity<AnimationDriver>,
        vcx: &'a mut VisualTestContext,
    }

    fn line() -> Hsla {
        Hsla {
            h: 0.5,
            s: 0.6,
            l: 0.5,
            a: 0.8,
        }
    }

    fn series_spec(fill: bool) -> ChartSeries {
        ChartSeries {
            fill: fill.then(|| line().alpha(0.2)),
            line: line(),
            ..ChartSeries::default()
        }
    }

    /// `n` samples `period` apart, the newest at `newest`.
    fn view_of(n: u64, newest: u64, period: u64, version: u64) -> Arc<SeriesView> {
        let samples: Vec<Sample> = (0..n)
            .map(|i| Sample::valid(newest - (n - 1 - i) * period, (i % 100) as f32))
            .collect();
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(0),
                metric: MetricId::CPU_TOTAL,
            },
            version,
            period_ns: period,
            samples: samples.into(),
        })
    }

    /// A chart 600 physical px wide (602 logical with the 1 px inset at scale 1).
    fn rig(cx: &mut TestAppContext, spec: ChartSpec) -> Rig<'_> {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        });
        let labels = super::labels();
        let (root, vcx) = cx.add_window_view(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            let chart = cx.new(|cx| ChartView::new(spec, labels, cx));
            Root {
                driver,
                chart,
                width: px(602.),
            }
        });
        vcx.simulate_scale_factor_change(1.0);
        let (chart, driver) = vcx.read_entity(&root, |r, _| (r.chart.clone(), r.driver.clone()));
        Rig {
            root,
            chart,
            driver,
            vcx,
        }
    }

    impl Rig<'_> {
        fn set(&mut self, series: Arc<SeriesView>, state: ChartState) {
            self.chart.update(self.vcx, |v, cx| {
                v.set_series_for_test(smallvec![Some(series)], cx);
                v.set_state(state, cx);
            });
        }

        fn draw(&mut self) {
            self.vcx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
        }

        fn builds(&mut self) -> u32 {
            self.vcx
                .read_entity(&self.chart, |v, _| v.build_count.get())
        }

        fn path_paints(&mut self) -> u32 {
            self.vcx
                .read_entity(&self.chart, |v, _| v.path_paints.get())
        }

        fn armed(&mut self) -> bool {
            self.vcx.read_entity(&self.driver, |d, _| d.is_armed())
        }
    }

    fn spec(smooth: bool, fill: bool) -> ChartSpec {
        ChartSpec {
            series: smallvec![series_spec(fill)],
            smooth,
            ..ChartSpec::default()
        }
    }

    #[gpui_kit::test]
    fn geometry_built_once_for_static_data(cx: &mut TestAppContext) {
        let mut r = rig(cx, spec(false, true));
        r.set(view_of(600, now_boot_ns(), 100 * MS, 1), ChartState::Live);
        r.vcx.run_until_parked();
        let (builds0, paints0) = (r.builds(), r.path_paints());
        r.draw();
        r.draw();
        let sets = r.vcx.read_entity(&r.chart, |v, _| v.cached_path_count());
        assert_eq!(sets, 2, "one fill and one stroke");
        assert_eq!(r.builds(), 1, "built once, then reused");
        assert!(builds0 <= 1);
        assert_eq!(
            r.path_paints() - paints0,
            2 * 2,
            "2 × (fills + strokes) over two paints"
        );
    }

    #[gpui_kit::test]
    fn geometry_rebuilt_on_version_bump(cx: &mut TestAppContext) {
        let mut r = rig(cx, spec(false, true));
        let now = now_boot_ns();
        r.set(view_of(600, now, 100 * MS, 1), ChartState::Live);
        r.draw();
        assert_eq!(r.builds(), 1);
        r.set(view_of(600, now + 100 * MS, 100 * MS, 2), ChartState::Live);
        r.draw();
        assert_eq!(r.builds(), 2);
    }

    #[gpui_kit::test]
    fn geometry_rebuilt_on_resize(cx: &mut TestAppContext) {
        let mut r = rig(cx, spec(false, true));
        r.set(view_of(600, now_boot_ns(), 100 * MS, 1), ChartState::Live);
        r.draw();
        assert_eq!(r.builds(), 1);
        r.root.update(r.vcx, |root, cx| {
            root.width = px(402.);
            cx.notify();
        });
        r.draw();
        assert_eq!(r.builds(), 2);
    }

    #[gpui_kit::test]
    fn moving_chart_requests_frame(cx: &mut TestAppContext) {
        let mut r = rig(cx, spec(true, false));
        r.set(
            view_of(600, now_boot_ns() - 50 * MS, 100 * MS, 1),
            ChartState::Live,
        );
        r.draw();
        assert!(r.armed(), "the driver was asked for a frame");
        // The frame notifies the chart, which paints again.
        let before = r.vcx.read_entity(&r.chart, |v, _| v.render_count.get());
        r.vcx.update(|window, cx| window.simulate_next_frame(cx));
        r.vcx.run_until_parked();
        r.draw();
        let after = r.vcx.read_entity(&r.chart, |v, _| v.render_count.get());
        assert!(after > before);
    }

    #[gpui_kit::test]
    fn idle_chart_requests_no_frame(cx: &mut TestAppContext) {
        let mut r = rig(cx, spec(true, false));
        r.set(
            view_of(600, now_boot_ns() - 3 * S, 100 * MS, 1),
            ChartState::Live,
        );
        r.draw();
        assert!(!r.armed());
        let requested = r.vcx.read_entity(&r.driver, |d, _| d.stats.requested);
        assert_eq!(requested, 0);
    }

    #[gpui_kit::test]
    fn reduce_motion_disables_sliding(cx: &mut TestAppContext) {
        let mut r = rig(cx, spec(true, false));
        r.vcx.update(|_, cx| cx.set_reduce_motion(true));
        let newest = now_boot_ns() - 50 * MS;
        r.set(view_of(600, newest, 100 * MS, 1), ChartState::Live);
        r.draw();
        assert!(!r.armed());
        let edge = r
            .vcx
            .read_entity(&r.chart, |v, _| v.last_right_edge_ns.get());
        assert_eq!(edge, newest);
    }

    #[gpui_kit::test]
    fn chart_stale_tail_half_alpha(cx: &mut TestAppContext) {
        let mut r = rig(cx, spec(false, false));
        let now = now_boot_ns();
        r.set(
            view_of(50, now - 5 * S, S, 1),
            ChartState::Stale { age_ns: 5 * S },
        );
        r.draw();
        let (colors, edge): (SmallVec<[Hsla; 8]>, u64) = r.vcx.read_entity(&r.chart, |v, _| {
            (v.path_colors.borrow().clone(), v.last_right_edge_ns.get())
        });
        assert!(
            edge >= now,
            "the right edge moves with the clock while stale"
        );
        assert_eq!(
            colors.len(),
            2,
            "live stroke, then the stale tail: {colors:?}"
        );
        assert_eq!(colors[0].a, line().a);
        assert_eq!(colors[1].a, line().a * 0.5);
        assert_eq!(
            (colors[1].h, colors[1].s, colors[1].l),
            (line().h, line().s, line().l)
        );
    }

    #[gpui_kit::test]
    fn paint_log_records_newest_sample(cx: &mut TestAppContext) {
        let mut r = rig(cx, spec(false, false));
        r.set(view_of(1, 1_000, 100 * MS, 1), ChartState::Live);
        r.vcx.run_until_parked();
        r.driver.update(r.vcx, |d, _| d.install_paint_log());
        r.draw();
        let log = r
            .driver
            .update(r.vcx, |d, _| d.take_paint_log())
            .expect("installed");
        let id = r.chart.entity_id();
        assert_eq!(log.records.len(), 1);
        let rec = log.records[0];
        assert_eq!(rec.view, id);
        assert_eq!(rec.newest_ns, 1_000);
        assert!(rec.painted_at_ns >= 1_000);
    }
}
