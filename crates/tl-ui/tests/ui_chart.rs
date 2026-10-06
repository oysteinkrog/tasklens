//! ChartView construction test with `TestAppContext` (E19, ADR-013). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{App, AppContext, Entity, TestAppContext};
use smallvec::smallvec;
use tl_model::client::EngineClient;
use tl_ui::chart::view::ChartView;
use tl_ui::chart::{ChartLabels, ChartSeries, ChartSpec, ValueAlign, ValueFormat};
use tl_ui::hub::SnapshotHub;
use tl_ui::testing::fake_engine::FakeEngine;

/// A hub over `engine`; installs the engine's latest snapshot.
fn hub_for(engine: &Arc<FakeEngine>, cx: &mut App) -> Entity<SnapshotHub> {
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.new(|cx| SnapshotHub::new(client, cx))
}

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
    let hub = cx.update(|cx| hub_for(&FakeEngine::new(), cx));
    let view = cx.new(|cx| ChartView::new(spec, labels(), &hub, cx));
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
    use tl_ui::testing::fake_engine::FakeEngine;

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
            let hub = super::hub_for(&FakeEngine::new(), cx);
            let chart = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));
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

    /// Toggling `performance.curved_lines` rebuilds the geometry once (tasklens-yrf.18).
    #[gpui_kit::test]
    fn curved_toggle_rebuilds_once(cx: &mut TestAppContext) {
        use tl_model::client::EngineClient;
        use tl_model::settings::{Settings, UiState};
        use tl_ui::app::{BootSettings, EngineGlobal};
        use tl_ui::settings_global::TlSettings;

        cx.update(|cx| {
            let client: Arc<dyn EngineClient> = FakeEngine::new();
            cx.set_global(EngineGlobal(client));
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let mut r = rig(cx, spec(false, true));
        r.set(view_of(600, now_boot_ns(), 100 * MS, 1), ChartState::Live);
        r.draw();
        assert_eq!(r.builds(), 1);

        let toggle = |r: &mut Rig<'_>, on: bool| {
            r.vcx.update(|_, cx| {
                TlSettings::update(cx, |d| d.settings.performance.curved_lines = on);
            });
            r.chart.update(r.vcx, |_, cx| cx.notify());
            r.draw();
            r.chart.update(r.vcx, |_, cx| cx.notify());
            r.draw();
        };
        toggle(&mut r, true);
        assert_eq!(r.builds(), 2, "rebuilt once after turning curves on");
        toggle(&mut r, false);
        assert_eq!(r.builds(), 3, "rebuilt once after turning curves off");
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

// ChartView data binding: hub subscription, series lookup, re-render rules (E19, tasklens-yrf.10).

mod binding {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, px,
    };
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::snapshot::{SeriesSet, Snapshot};
    use tl_ui::chart::view::{ChartView, search_series};
    use tl_ui::chart::{ChartSeries, ChartSpec, ChartState};
    use tl_ui::testing::fake_engine::FakeEngine;

    const MS: u64 = 1_000_000;
    const S: u64 = 1_000_000_000;

    fn sid(device: u16, metric: MetricId) -> SeriesId {
        SeriesId {
            device: DeviceIdx(device),
            metric,
        }
    }

    const A: SeriesId = SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::CPU_TOTAL,
    };
    const B: SeriesId = SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::MEM_USED,
    };

    /// `n` samples 100 ms apart, the newest at `newest`.
    fn series(id: SeriesId, version: u64, n: u64, newest: u64) -> Arc<SeriesView> {
        let samples: Vec<Sample> = (0..n)
            .map(|i| Sample::valid(newest - (n - 1 - i) * 100 * MS, 50.0))
            .collect();
        Arc::new(SeriesView {
            id,
            version,
            period_ns: 100 * MS,
            samples: samples.into(),
        })
    }

    fn set_series(snap: &mut Snapshot, views: Vec<Arc<SeriesView>>) {
        snap.series = Arc::new(SeriesSet {
            header: snap.series.header,
            series: Arc::from(views),
            slow: Arc::from([]),
        });
    }

    fn spec_for(ids: &[SeriesId]) -> ChartSpec {
        ChartSpec {
            series: ids
                .iter()
                .map(|&id| ChartSeries {
                    id,
                    ..ChartSeries::default()
                })
                .collect(),
            ..ChartSpec::default()
        }
    }

    struct Root {
        chart: Entity<ChartView>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(302.)).h(px(102.)).child(self.chart.clone())
        }
    }

    /// A window holding one chart for `ids`, bound to a hub over `engine`.
    fn chart_in_window<'a>(
        cx: &'a mut TestAppContext,
        engine: &Arc<FakeEngine>,
        ids: &[SeriesId],
    ) -> (Entity<ChartView>, &'a mut VisualTestContext) {
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
        let spec = spec_for(ids);
        let (root, vcx) = cx.add_window_view(|_, cx| {
            let hub = super::hub_for(engine, cx);
            let chart = cx.new(|cx| ChartView::new(spec, super::labels(), &hub, cx));
            Root { chart }
        });
        vcx.run_until_parked();
        let chart = vcx.read_entity(&root, |r, _| r.chart.clone());
        (chart, vcx)
    }

    fn render_count(chart: &Entity<ChartView>, vcx: &mut VisualTestContext) -> u32 {
        vcx.read_entity(chart, |v, _| v.render_count.get())
    }

    fn resolved(chart: &Entity<ChartView>, vcx: &mut VisualTestContext) -> Arc<SeriesView> {
        vcx.read_entity(chart, |v, _| {
            v.resolved()[0].clone().expect("series resolved")
        })
    }

    #[gpui_kit::test]
    fn chart_not_rerendered_on_unrelated_series(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let now = 10 * S;
        engine.publish_with(|s| set_series(s, vec![series(A, 1, 5, now), series(B, 1, 5, now)]));
        let (chart, vcx) = chart_in_window(cx, &engine, &[B]);
        assert_eq!(render_count(&chart, vcx), 1);
        let before = resolved(&chart, vcx);

        engine.publish_with(|s| {
            let b = s.series.series[1].clone();
            set_series(s, vec![series(A, 2, 5, now + 100 * MS), b]);
        });
        vcx.run_until_parked();
        assert_eq!(render_count(&chart, vcx), 1, "series A changed, not B");
        assert!(Arc::ptr_eq(&before, &resolved(&chart, vcx)));
    }

    #[gpui_kit::test]
    fn chart_rerendered_on_own_series(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let now = 10 * S;
        engine.publish_with(|s| set_series(s, vec![series(A, 1, 5, now), series(B, 1, 5, now)]));
        let (chart, vcx) = chart_in_window(cx, &engine, &[B]);
        assert_eq!(render_count(&chart, vcx), 1);
        let before = resolved(&chart, vcx);

        let next = series(B, 2, 5, now + 100 * MS);
        engine.publish_with(|s| {
            let a = s.series.series[0].clone();
            set_series(s, vec![a, next.clone()]);
        });
        vcx.run_until_parked();
        assert_eq!(render_count(&chart, vcx), 2);
        let after = resolved(&chart, vcx);
        assert!(!Arc::ptr_eq(&before, &after), "a new Arc was stored");
        assert!(Arc::ptr_eq(&after, &next), "the snapshot's Arc, not a copy");
    }

    #[gpui_kit::test]
    fn chart_rerendered_when_device_appears(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let gpu = sid(1, MetricId::GPU_UTIL);
        let (chart, vcx) = chart_in_window(cx, &engine, &[gpu]);
        assert_eq!(render_count(&chart, vcx), 1);

        engine.publish_with(|s| Arc::make_mut(&mut s.devices).version += 1);
        vcx.run_until_parked();
        assert_eq!(render_count(&chart, vcx), 2, "the series was absent");

        engine.publish_with(|s| set_series(s, vec![series(gpu, 1, 5, 10 * S)]));
        vcx.run_until_parked();
        assert_eq!(render_count(&chart, vcx), 3);

        engine.publish_with(|s| Arc::make_mut(&mut s.devices).version += 1);
        vcx.run_until_parked();
        assert_eq!(render_count(&chart, vcx), 3, "every series present");
    }

    #[gpui_kit::test]
    fn chart_resolves_by_binary_search(cx: &mut TestAppContext) {
        let ids: Vec<SeriesId> = (0..80u16)
            .map(|i| sid(i / 4, MetricId(i % 4 * 10)))
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, ids, "built in SeriesId order");
        let views: Vec<Arc<SeriesView>> = ids.iter().map(|&id| series(id, 1, 1, S)).collect();
        let first = ids[3];
        let last = ids[77];

        // 2 lookups, at most ⌈log₂ 80⌉ = 7 comparisons each.
        let mut comparisons = 0u32;
        for target in [first, last] {
            let found = search_series(&views, |v| {
                comparisons += 1;
                v.id.cmp(&target)
            });
            assert_eq!(found.map(|v| v.id), Some(target));
        }
        assert!(comparisons <= 2 * 7, "{comparisons} comparisons");
        for &target in &ids {
            let mut n = 0u32;
            let found = search_series(&views, |v| {
                n += 1;
                v.id.cmp(&target)
            });
            assert!(Arc::ptr_eq(
                found.unwrap(),
                &views[ids.iter().position(|&i| i == target).unwrap()]
            ));
            assert!(n <= 7, "{n} comparisons for {target:?}");
        }
        assert!(search_series(&views, |v| v.id.cmp(&sid(50, MetricId::CPU_TOTAL))).is_none());

        let mut snap = tl_ui::testing::fake_engine::empty_snapshot();
        set_series(&mut snap, views);
        let hub = cx.update(|cx| super::hub_for(&FakeEngine::new(), cx));
        let chart =
            cx.new(|cx| ChartView::new(spec_for(&[first, last]), super::labels(), &hub, cx));
        let counts = |snap: &Snapshot| [3, 77].map(|i| Arc::strong_count(&snap.series.series[i]));
        let before = counts(&snap);
        let resolved = chart.update(cx, |v, _| {
            v.resolve(&snap);
            v.resolved().to_vec()
        });
        // One Arc clone per series is held by the chart, one by `resolved`; nothing is copied.
        assert_eq!(counts(&snap), before.map(|n| n + 2));
        assert!(Arc::ptr_eq(
            resolved[0].as_ref().unwrap(),
            &snap.series.series[3]
        ));
        assert!(Arc::ptr_eq(
            resolved[1].as_ref().unwrap(),
            &snap.series.series[77]
        ));
    }

    #[gpui_kit::test]
    fn chart_state_warming_up_then_live_then_stale(cx: &mut TestAppContext) {
        let hub = cx.update(|cx| super::hub_for(&FakeEngine::new(), cx));
        let chart = cx.new(|cx| ChartView::new(spec_for(&[B]), super::labels(), &hub, cx));
        let mut snap = tl_ui::testing::fake_engine::empty_snapshot();
        let newest = 100 * S;

        chart.update(cx, |v, _| {
            assert_eq!(v.derived_state(newest), ChartState::WarmingUp, "absent");

            set_series(&mut snap, vec![series(B, 1, 0, newest)]);
            v.resolve(&snap);
            assert_eq!(v.derived_state(newest), ChartState::WarmingUp, "no samples");

            set_series(&mut snap, vec![series(B, 2, 10, newest)]);
            v.resolve(&snap);
            assert_eq!(v.derived_state(newest + 50 * MS), ChartState::Live);
            assert_eq!(
                v.derived_state(newest + 2 * S),
                ChartState::Stale { age_ns: 2 * S }
            );
        });
    }

    #[gpui_kit::test]
    fn owner_state_wins(cx: &mut TestAppContext) {
        let hub = cx.update(|cx| super::hub_for(&FakeEngine::new(), cx));
        let chart = cx.new(|cx| ChartView::new(spec_for(&[B]), super::labels(), &hub, cx));
        let mut snap = tl_ui::testing::fake_engine::empty_snapshot();
        let newest = 100 * S;
        chart.update(cx, |v, cx| {
            v.set_state(ChartState::Restricted, cx);
            assert_eq!(v.derived_state(newest), ChartState::Restricted, "absent");
            set_series(&mut snap, vec![series(B, 1, 10, newest)]);
            v.resolve(&snap);
            assert_eq!(v.derived_state(newest), ChartState::Restricted, "live data");
            assert_eq!(v.derived_state(newest + 5 * S), ChartState::Restricted);

            v.set_state(ChartState::Failed("permission denied".into()), cx);
            assert_eq!(
                v.derived_state(newest),
                ChartState::Failed("permission denied".into())
            );
            v.set_state(ChartState::Live, cx);
            assert_eq!(v.derived_state(newest), ChartState::Live, "derived again");
        });
    }
}

// Chart element anchor: state painting, scene shape, render counts, frame requests, value_now
// (E19-A1, tasklens-yrf.1).

mod anchor {
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, ParentElement, Pixels, Render, StyleRefinement,
        Styled, TestAppContext, VisualTestContext, Window, div, px,
    };
    use smallvec::{SmallVec, smallvec};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::snapshot::{SeriesSet, Snapshot};
    use tl_model::time::now_boot_ns;
    use tl_model::validity::Validity;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::chart::geometry::y_of;
    use tl_ui::chart::{ChartSeries, ChartSpec, ChartState, ChartView, GridStyle};
    use tl_ui::testing::fake_engine::FakeEngine;

    const MS: u64 = 1_000_000;
    const S: u64 = 1_000_000_000;

    const A: SeriesId = SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::CPU_TOTAL,
    };
    const B: SeriesId = SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::MEM_USED,
    };

    fn init(cx: &mut TestAppContext) {
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
    }

    fn full_grid() -> GridStyle {
        GridStyle {
            lines: true,
            baseline: true,
            outline: true,
            rounded: false,
        }
    }

    fn spec(id: SeriesId, smooth: bool) -> ChartSpec {
        ChartSpec {
            series: smallvec![ChartSeries {
                id,
                fill: Some(gpui_kit::Hsla {
                    h: 0.5,
                    s: 0.6,
                    l: 0.5,
                    a: 0.2,
                }),
                line: gpui_kit::Hsla {
                    h: 0.5,
                    s: 0.6,
                    l: 0.5,
                    a: 1.0,
                },
                ..ChartSeries::default()
            }],
            grid: full_grid(),
            smooth,
            ..ChartSpec::default()
        }
    }

    fn view_from(
        id: SeriesId,
        version: u64,
        period_ns: u64,
        samples: Vec<Sample>,
    ) -> Arc<SeriesView> {
        Arc::new(SeriesView {
            id,
            version,
            period_ns,
            samples: samples.into(),
        })
    }

    /// `n` valid samples `period` apart at value 50, the newest at `newest`.
    fn flat(id: SeriesId, version: u64, n: u64, newest: u64, period: u64) -> Arc<SeriesView> {
        let samples = (0..n)
            .map(|i| Sample::valid(newest - (n - 1 - i) * period, 50.0))
            .collect();
        view_from(id, version, period, samples)
    }

    /// Root view that owns the window's driver and stacks `charts`, each `w × h` logical px and
    /// embedded cached, as owners do, so a chart re-renders only when it is notified.
    struct Root {
        driver: Entity<AnimationDriver>,
        charts: Vec<Entity<ChartView>>,
        w: Pixels,
        h: Pixels,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .children(self.charts.iter().map(|c| {
                    div()
                        .w(self.w)
                        .h(self.h)
                        .flex_none()
                        .child(c.clone().cached(StyleRefinement::default().size_full()))
                }))
        }
    }

    /// A window with `specs.len()` charts of `w × h`, bound to a hub over `engine`. The test
    /// window starts at scale 2; tests that check physical px call `at_scale_one`.
    fn window_with<'a>(
        cx: &'a mut TestAppContext,
        engine: &Arc<FakeEngine>,
        specs: Vec<ChartSpec>,
        w: f32,
        h: f32,
    ) -> (Entity<Root>, &'a mut VisualTestContext) {
        init(cx);
        let (root, vcx) = cx.add_window_view(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            let hub = super::hub_for(engine, cx);
            let charts = specs
                .into_iter()
                .map(|s| cx.new(|cx| ChartView::new(s, super::labels(), &hub, cx)))
                .collect();
            Root {
                driver,
                charts,
                w: px(w),
                h: px(h),
            }
        });
        vcx.run_until_parked();
        (root, vcx)
    }

    /// Scale 1, so physical px equal logical px. Refreshes the window, rendering every chart.
    fn at_scale_one(vcx: &mut VisualTestContext) {
        vcx.simulate_scale_factor_change(1.0);
        vcx.run_until_parked();
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    fn chart(root: &Entity<Root>, vcx: &mut VisualTestContext, i: usize) -> Entity<ChartView> {
        vcx.read_entity(root, |r, _| r.charts[i].clone())
    }

    fn set_series(snap: &mut Snapshot, views: Vec<Arc<SeriesView>>) {
        snap.series = Arc::new(SeriesSet {
            header: snap.series.header,
            series: Arc::from(views),
            slow: Arc::from([]),
        });
    }

    #[gpui_kit::test]
    fn warming_up_paints_no_paths(cx: &mut TestAppContext) {
        let (root, vcx) = window_with(cx, &FakeEngine::new(), vec![spec(A, false)], 602., 202.);
        at_scale_one(vcx);
        let c = chart(&root, vcx, 0);
        draw(vcx);
        vcx.read_entity(&c, |v, _| {
            assert_eq!(v.derived_state(now_boot_ns()), ChartState::WarmingUp);
            assert_eq!(v.path_paints.get(), 0, "no paint_path call");
            // 10 or 11 vertical hairlines plus 3 horizontal ones.
            let quads = v.last_grid_quads.get();
            assert!((13..=14).contains(&quads), "{quads} grid quads");
            assert!(v.last_gap_spans.borrow().is_empty());
            assert_eq!(v.value_now(), Some((0.0, Validity::WarmingUp)));
        });
    }

    #[gpui_kit::test]
    fn failed_state_dotted_baseline_only(cx: &mut TestAppContext) {
        let (root, vcx) = window_with(cx, &FakeEngine::new(), vec![spec(A, true)], 602., 202.);
        at_scale_one(vcx);
        let c = chart(&root, vcx, 0);
        c.update(vcx, |v, cx| {
            // Live, moving data: the owner state still wins.
            v.set_series_for_test(
                smallvec![Some(flat(A, 1, 600, now_boot_ns() - 50 * MS, 100 * MS))],
                cx,
            );
            v.set_state(ChartState::Failed("EACCES".into()), cx);
        });
        draw(vcx);
        vcx.read_entity(&c, |v, _| {
            assert_eq!(v.last_grid_quads.get(), 0, "no grid lines");
            assert_eq!(v.path_paints.get(), 0, "no paths");
            assert_eq!(&v.last_gap_spans.borrow()[..], &[0.0..600.0], "whole plot");
            assert!(matches!(v.value_now(), Some((_, Validity::Failed))));
        });
        let (armed, requested) = vcx.read_entity(&root, |r, cx| {
            let d = r.driver.read(cx);
            (d.is_armed(), d.stats.requested)
        });
        assert!(!armed, "a failed chart does not animate");
        assert_eq!(requested, 0);
    }

    #[gpui_kit::test]
    fn gap_and_spike_scene_shape(cx: &mut TestAppContext) {
        let (root, vcx) = window_with(cx, &FakeEngine::new(), vec![spec(A, false)], 400., 200.);
        at_scale_one(vcx);
        let c = chart(&root, vcx, 0);
        // 70 samples 1 s apart over the 60 s window: value 20, one gap at 40, a spike to 100
        // at 50.
        let newest = now_boot_ns();
        let samples = (0..70u64)
            .map(|i| {
                let t = newest - (69 - i) * S;
                match i {
                    40 => Sample::gap(t),
                    50 => Sample::valid(t, 100.0),
                    _ => Sample::valid(t, 20.0),
                }
            })
            .collect();
        c.update(vcx, |v, cx| {
            v.set_series_for_test(smallvec![Some(view_from(A, 1, S, samples))], cx);
        });
        draw(vcx);
        vcx.read_entity(&c, |v, _| {
            assert_eq!(v.derived_state(newest), ChartState::Live);
            assert_eq!(v.cached_fill_stroke_counts(), (1, 1));
            assert_eq!(v.cached_path_count(), 2, "no stale sets");
            assert_eq!(
                v.path_colors.borrow().len(),
                2,
                "1 fill and 1 stroke painted"
            );
            assert_eq!(v.last_gap_spans.borrow().len(), 1, "1 dotted span");
            // The stroke's top vertex sits on the spike row, y_of(100) = 0 (the plot top), within
            // the 1 px line width.
            let spike_row = y_of(100.0, 0.0, 100.0, 198.0);
            assert_eq!(spike_row, 0.0);
            let top = v.stroke_top_y().expect("stroke vertices");
            assert!((top - spike_row).abs() <= 1.0, "stroke top at {top}");
        });
    }

    #[gpui_kit::test]
    fn render_counter_ten_snapshots(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let t0 = 10 * S;
        engine.publish_with(|s| {
            set_series(
                s,
                vec![flat(A, 1, 5, t0, 100 * MS), flat(B, 1, 5, t0, 100 * MS)],
            )
        });
        let (root, vcx) = window_with(
            cx,
            &engine,
            vec![spec(A, false), spec(B, false)],
            302.,
            102.,
        );
        let (a, b) = (chart(&root, vcx, 0), chart(&root, vcx, 1));
        for i in 1..=10u64 {
            vcx.executor().advance_clock(Duration::from_millis(100));
            engine.publish_with(|s| {
                let b = s.series.series[1].clone();
                set_series(s, vec![flat(A, 1 + i, 5, t0 + i * 100 * MS, 100 * MS), b]);
            });
            vcx.run_until_parked();
        }
        let count = |c: &Entity<ChartView>, vcx: &mut VisualTestContext| {
            vcx.read_entity(c, |v, _| v.render_count.get())
        };
        assert_eq!(count(&a, vcx), 11);
        assert_eq!(count(&b, vcx), 1);
    }

    #[gpui_kit::test]
    fn thirty_two_charts_one_frame_request(cx: &mut TestAppContext) {
        let specs = (0..32).map(|_| spec(A, true)).collect();
        let (root, vcx) = window_with(cx, &FakeEngine::new(), specs, 202., 12.);
        let now = now_boot_ns();
        for i in 0..32 {
            let c = chart(&root, vcx, i);
            c.update(vcx, |v, cx| {
                v.set_series_for_test(
                    smallvec![Some(flat(A, 1, 600, now - 50 * MS, 100 * MS))],
                    cx,
                );
            });
        }
        draw(vcx);
        let driver = vcx.read_entity(&root, |r, _| r.driver.clone());
        let requested =
            |vcx: &mut VisualTestContext| vcx.read_entity(&driver, |d, _| d.stats.requested);
        assert_eq!(requested(vcx), 1, "32 moving charts, one frame request");

        // The next frame notifies all 32; their paints arm one more request.
        vcx.update(|window, cx| window.simulate_next_frame(cx));
        vcx.run_until_parked();
        draw(vcx);
        assert_eq!(requested(vcx), 2);
    }

    #[gpui_kit::test]
    fn value_now_per_state(cx: &mut TestAppContext) {
        let hub = cx.update(|cx| super::hub_for(&FakeEngine::new(), cx));
        let chart = cx.new(|cx| ChartView::new(spec(A, false), super::labels(), &hub, cx));
        let t = 100 * S;
        let with_gap: SmallVec<[Option<Arc<SeriesView>>; 2]> = smallvec![Some(view_from(
            A,
            2,
            100 * MS,
            vec![Sample::valid(t - 100 * MS, 42.0), Sample::gap(t)],
        ))];
        chart.update(cx, |v, cx| {
            assert_eq!(v.value_at(t), Some((0.0, Validity::WarmingUp)), "absent");

            v.set_series_for_test(smallvec![Some(flat(A, 1, 10, t, 100 * MS))], cx);
            assert_eq!(v.derived_state(t + 50 * MS), ChartState::Live);
            assert_eq!(v.value_at(t + 50 * MS), Some((50.0, Validity::Valid)));
            assert_eq!(v.value_at(t + 5 * S), Some((50.0, Validity::Stale)));

            for (state, validity) in [
                (ChartState::Failed("EACCES".into()), Validity::Failed),
                (ChartState::Restricted, Validity::Restricted),
                (ChartState::Unsupported, Validity::Unsupported),
            ] {
                v.set_state(state, cx);
                assert_eq!(v.value_at(t), Some((50.0, validity)));
                assert_eq!(v.value_now().map(|(_, s)| s), Some(validity));
            }
            v.set_state(ChartState::Live, cx);

            // Newest sample is a gap: no current value, the last valid one is kept.
            v.set_series_for_test(with_gap.clone(), cx);
            assert_eq!(v.value_at(t + 50 * MS), Some((42.0, Validity::WarmingUp)));

            v.set_spec(ChartSpec::default(), cx);
            assert_eq!(v.value_at(t), None, "no series");
        });
    }
}

// Chart labels: cached shaped lines, visibility rules, value glyphs and formats (E19,
// tasklens-yrf.11).

mod labels {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{
        Bounds, Context, Hsla, IntoElement, Quad, Render, Styled, TestAppContext, Window, canvas,
        point, px, size,
    };
    use smallvec::{SmallVec, smallvec};
    use tl_model::settings::{AppearanceSettings, NetworkUnit, ThemeSettings};
    use tl_model::validity::Validity;
    use tl_ui::chart::labels::{
        LabelCache, LabelKind, LabelPaint, format_value, paint, range_label,
    };
    use tl_ui::chart::{
        ChartLabels, ChartSeries, ChartSpec, ChartState, LegendStyle, ValueAlign, ValueFormat,
    };
    use tl_ui::theme::{TlTheme, UiMetrics, current};

    const S: u64 = 1_000_000_000;

    /// What one paint draws; the box sits at the window origin.
    #[derive(Clone)]
    struct Input {
        w: f32,
        h: f32,
        labels: ChartLabels,
        spec: ChartSpec,
        state: ChartState,
        value: Option<(f32, Validity)>,
        axis_text: Option<&'static str>,
    }

    fn input(w: f32, h: f32) -> Input {
        Input {
            w,
            h,
            labels: ChartLabels {
                heading: None,
                value_fmt: ValueFormat::Percent,
                value_align: ValueAlign::Right,
                range_word: "Utilization",
                legend: smallvec![],
                show_axis_max: false,
            },
            spec: ChartSpec::default(),
            state: ChartState::Live,
            value: None,
            axis_text: None,
        }
    }

    fn legend() -> SmallVec<[(gpui_kit::SharedString, LegendStyle); 2]> {
        smallvec![
            ("Total".into(), LegendStyle::Solid),
            ("Kernel".into(), LegendStyle::Dashed),
        ]
    }

    struct Painter {
        cache: Rc<RefCell<LabelCache>>,
        input: Rc<RefCell<Input>>,
    }

    impl Render for Painter {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let cache = self.cache.clone();
            let input = self.input.clone();
            canvas(
                |_, _, _| (),
                move |_, _, window, cx| {
                    let i = input.borrow().clone();
                    let metrics = *cx.global::<UiMetrics>();
                    let theme: Arc<TlTheme> = current(cx).clone();
                    let box_bounds = Bounds::new(point(px(0.), px(0.)), size(px(i.w), px(i.h)));
                    let p = LabelPaint {
                        box_bounds,
                        plot: box_bounds.inset(px(1.)),
                        labels: &i.labels,
                        spec: &i.spec,
                        state: &i.state,
                        value: i.value,
                        axis_text: i.axis_text,
                        metrics: &metrics,
                        theme: &theme,
                        network_unit: NetworkUnit::Bits,
                    };
                    let mut scratch = String::new();
                    paint(window, cx, &mut cache.borrow_mut(), &p, &mut scratch);
                },
            )
            .size_full()
        }
    }

    struct Rig<'a> {
        cache: Rc<RefCell<LabelCache>>,
        input: Rc<RefCell<Input>>,
        vcx: &'a mut gpui_kit::VisualTestContext,
        theme: Arc<TlTheme>,
    }

    impl Rig<'_> {
        /// Paints the current input and returns the frame's quads and scale factor.
        fn draw(&mut self) -> (Vec<Quad>, f32) {
            self.vcx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
                (window.painted_quads(), window.scale_factor())
            })
        }

        fn set(&self, f: impl FnOnce(&mut Input)) {
            f(&mut self.input.borrow_mut());
        }
    }

    fn rig(cx: &mut TestAppContext, i: Input) -> Rig<'_> {
        let theme = cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            current(cx).clone()
        });
        let cache = Rc::new(RefCell::new(LabelCache::default()));
        let input = Rc::new(RefCell::new(i));
        let (c, inp) = (cache.clone(), input.clone());
        let (_, vcx) = cx.add_window_view(move |_, _| Painter {
            cache: c,
            input: inp,
        });
        Rig {
            cache,
            input,
            vcx,
            theme,
        }
    }

    /// Quads of `w × h` logical px.
    fn quads_of(quads: &[Quad], sf: f32, w: f32, h: f32) -> Vec<Quad> {
        quads
            .iter()
            .filter(|q| {
                (q.bounds.size.width.0 / sf - w).abs() < 0.01
                    && (q.bounds.size.height.0 / sf - h).abs() < 0.01
            })
            .cloned()
            .collect()
    }

    #[gpui_kit::test]
    fn chart_value_label_reshapes_only_on_text_change(cx: &mut TestAppContext) {
        // Small box: no range label; no heading, legend or axis max.
        let mut i = input(116., 56.);
        i.value = Some((12.04, Validity::Valid));
        let mut r = rig(cx, i);
        r.draw();
        assert_eq!(r.cache.borrow().shape_count, 1);
        assert_eq!(r.cache.borrow().text(LabelKind::Value), Some("12.0%"));

        r.set(|i| i.value = Some((11.96, Validity::Valid)));
        r.draw();
        assert_eq!(r.cache.borrow().shape_count, 1, "same text, no re-shape");

        r.set(|i| i.value = Some((13.0, Validity::Valid)));
        r.draw();
        assert_eq!(r.cache.borrow().shape_count, 2);
        assert_eq!(r.cache.borrow().text(LabelKind::Value), Some("13.0%"));

        // Same text, other colour: re-shaped.
        r.set(|i| i.value = Some((13.0, Validity::Stale)));
        r.draw();
        assert_eq!(r.cache.borrow().shape_count, 3);
    }

    #[test]
    fn range_label_text() {
        for (word, ns, want) in [
            ("Utilization", 60 * S, "Utilization over 60 seconds"),
            ("Throughput", 300 * S, "Throughput over 5 minutes"),
            ("Memory usage", 30 * S, "Memory usage over 30 seconds"),
            ("Power draw", 600 * S, "Power draw over 10 minutes"),
        ] {
            let mut out = String::new();
            range_label(word, ns, &mut out);
            assert_eq!(out, want);
        }
    }

    #[gpui_kit::test]
    fn small_box_hides_range_and_legend(cx: &mut TestAppContext) {
        let mut i = input(116., 56.);
        i.labels.legend = legend();
        i.labels.heading = Some("CPU".into());
        let mut r = rig(cx, i);
        let (quads, _) = r.draw();
        let c = r.cache.borrow();
        assert_eq!(c.shape_count, 1, "only the heading");
        assert!(c.text(LabelKind::Range).is_none());
        assert!(c.text(LabelKind::Legend(0)).is_none());
        assert!(c.painted(LabelKind::Range).is_none());
        assert!(quads.is_empty(), "no swatches");

        // The same chart large shows both.
        drop(c);
        r.set(|i| {
            i.w = 600.;
            i.h = 300.;
        });
        r.draw();
        let c = r.cache.borrow();
        assert_eq!(
            c.text(LabelKind::Range),
            Some("Utilization over 60 seconds")
        );
        assert_eq!(c.text(LabelKind::Legend(1)), Some("Kernel"));
    }

    fn two_series() -> ChartSpec {
        let line = |h: f32| Hsla {
            h,
            s: 0.5,
            l: 0.5,
            a: 1.0,
        };
        ChartSpec {
            series: smallvec![
                ChartSeries {
                    line: line(0.1),
                    ..ChartSeries::default()
                },
                ChartSeries {
                    line: line(0.6),
                    dashed: true,
                    ..ChartSeries::default()
                },
            ],
            ..ChartSpec::default()
        }
    }

    #[gpui_kit::test]
    fn legend_horizontal_wide_vertical_narrow(cx: &mut TestAppContext) {
        let mut i = input(600., 300.);
        i.labels.legend = legend();
        i.spec = two_series();
        let mut r = rig(cx, i);
        let (quads, sf) = r.draw();
        {
            let c = r.cache.borrow();
            assert_eq!(c.text(LabelKind::Legend(0)), Some("Total"));
            assert_eq!(c.text(LabelKind::Legend(1)), Some("Kernel"));
            let a = c.painted(LabelKind::Legend(0)).expect("first item");
            let b = c.painted(LabelKind::Legend(1)).expect("second item");
            assert!(b.origin.x > a.origin.x, "side by side");
            assert_eq!(a.origin.y, b.origin.y);
        }
        let solid = quads_of(&quads, sf, 12., 2.);
        assert_eq!(solid.len(), 1, "one solid 12 × 2 swatch");
        let dashes = quads_of(&quads, sf, 3., 2.);
        let tail = quads_of(&quads, sf, 2., 2.);
        assert_eq!(
            dashes.len() + tail.len(),
            3,
            "the dashed swatch is three dashes"
        );
        let x0 = dashes[0].bounds.origin.x.0 / sf;
        let last = tail.first().or(dashes.last()).expect("dash");
        let x_end = (last.bounds.origin.x.0 + last.bounds.size.width.0) / sf;
        assert!((x_end - x0 - 12.).abs() < 0.01, "dashed swatch spans 12 px");
        assert!(x0 > solid[0].bounds.origin.x.0 / sf);
        let line1 = two_series().series[1].line;
        let line0 = two_series().series[0].line;
        assert_eq!(solid[0].background, line0.into());
        assert!(
            dashes
                .iter()
                .chain(&tail)
                .all(|q| q.background == line1.into())
        );

        r.set(|i| {
            i.w = 180.;
            i.h = 150.;
        });
        r.draw();
        let c = r.cache.borrow();
        let a = c.painted(LabelKind::Legend(0)).expect("first item");
        let b = c.painted(LabelKind::Legend(1)).expect("second item");
        assert!(b.origin.y > a.origin.y, "stacked");
        assert_eq!(a.origin.x, b.origin.x);
    }

    #[gpui_kit::test]
    fn axis_max_visibility_rule(cx: &mut TestAppContext) {
        // Plot height = box height − 2.
        let mut i = input(600., 92.);
        i.labels.show_axis_max = true;
        i.axis_text = Some("100 Mbps");
        let mut r = rig(cx, i);
        r.draw();
        assert!(r.cache.borrow().text(LabelKind::AxisMax).is_none());
        assert_eq!(r.cache.borrow().shape_count, 0);

        r.set(|i| i.h = 152.);
        r.draw();
        let c = r.cache.borrow();
        assert_eq!(c.text(LabelKind::AxisMax), Some("100 Mbps"));
        let a = c.painted(LabelKind::AxisMax).expect("axis max");
        let pad = 6.0;
        let right = f32::from(a.origin.x + a.width);
        assert!(
            (right - (600. - pad)).abs() < 0.01,
            "right-aligned at {right}"
        );
        assert!(f32::from(a.origin.x) > 300.);
        drop(c);

        // No axis text, or the flag off: hidden.
        r.set(|i| i.labels.show_axis_max = false);
        r.draw();
        assert!(r.cache.borrow().painted(LabelKind::AxisMax).is_none());
    }

    #[gpui_kit::test]
    fn value_glyph_per_validity(cx: &mut TestAppContext) {
        let mut r = rig(cx, input(300., 100.));
        let t = r.theme.clone();
        for (validity, text, color) in [
            (Validity::Valid, Some("42.0%"), t.text_primary),
            (Validity::Stale, Some("42.0%"), t.text_secondary),
            (Validity::WarmingUp, Some("…"), t.text_secondary),
            (Validity::Unsupported, Some("n/a"), t.text_secondary),
            (Validity::Missing, Some("-"), t.text_secondary),
            (Validity::Failed, Some("!"), t.status.warning),
        ] {
            r.set(|i| i.value = Some((42.0, validity)));
            r.draw();
            let c = r.cache.borrow();
            assert_eq!(c.text(LabelKind::Value), text, "{validity:?}");
            let painted = c.painted(LabelKind::Value).expect("value painted");
            assert_eq!(painted.color, color, "{validity:?}");
            assert!(c.painted(LabelKind::Lock).is_none());
        }

        r.set(|i| i.value = Some((42.0, Validity::Restricted)));
        r.draw();
        let c = r.cache.borrow();
        let lock = c.painted(LabelKind::Lock).expect("lock icon");
        assert_eq!(lock.width, px(12.));
        assert_eq!(lock.color, t.text_secondary);
        assert!(c.painted(LabelKind::Value).is_none());
        drop(c);

        r.set(|i| i.labels.value_align = ValueAlign::Hidden);
        r.draw();
        assert!(r.cache.borrow().painted.is_empty());
    }

    #[test]
    fn format_value_table_d() {
        let f = |fmt, v: f32, unit| {
            let mut out = String::new();
            format_value(fmt, v, unit, &mut out);
            out
        };
        use NetworkUnit::{Bits, Bytes};
        use ValueFormat as F;
        // Network series are bytes per second: 150 000 B/s is 1.2 Mbps.
        assert_eq!(f(F::RateNetwork, 150_000.0, Bits), "1.2 Mbps");
        assert_eq!(f(F::RateNetwork, 1.5 * 1024.0 * 1024.0, Bytes), "1.5 MiB/s");
        assert_eq!(f(F::Percent, 0.0, Bits), "0%");
        assert_eq!(f(F::Percent, 12.0, Bits), "12.0%");
        assert_eq!(f(F::Mhz, 4631.4, Bits), "4631 MHz");
        assert_eq!(f(F::Celsius, 52.4, Bits), "52 °C");
        assert_eq!(f(F::Watts, 31.94, Bits), "31.9 W");
        assert_eq!(f(F::Count, 12345.0, Bits), "12\u{2009}345");
        assert_eq!(f(F::Bytes, 1536.0, Bits), "1.50 KiB");
        assert_eq!(f(F::RateIec, 1.5 * 1024.0 * 1024.0, Bits), "1.5 MiB/s");
        assert_eq!(f(F::Rpm, 1199.6, Bits), "1200 RPM");
    }
}

// Geometry key compared in place: no key rebuild and no geometry build per frame for an
// unchanged chart, even past the inline series size (tasklens-yrf.19).
mod key_in_place {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, px,
    };
    use smallvec::SmallVec;
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::time::now_boot_ns;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::chart::view::ChartView;
    use tl_ui::chart::{ChartSeries, ChartSpec, ChartState};
    use tl_ui::testing::fake_engine::FakeEngine;

    const SERIES: usize = 16;
    const PERIOD: u64 = 1_000_000_000;

    struct Root {
        _driver: Entity<AnimationDriver>,
        chart: Entity<ChartView>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(602.)).h(px(202.)).child(self.chart.clone())
        }
    }

    fn view_of(i: usize, newest: u64, version: u64) -> Arc<SeriesView> {
        let samples: Vec<Sample> = (0..60u64)
            .map(|k| Sample::valid(newest - (59 - k) * PERIOD, ((k + i as u64) % 100) as f32))
            .collect();
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(0),
                metric: MetricId::CPU_TOTAL,
            },
            version,
            period_ns: PERIOD,
            samples: samples.into(),
        })
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    fn counts(vcx: &mut VisualTestContext, chart: &Entity<ChartView>) -> (u32, u32) {
        vcx.read_entity(chart, |v, _| (v.key_rebuilds(), v.build_count.get()))
    }

    #[gpui_kit::test]
    fn unchanged_16_series_chart_rebuilds_nothing(cx: &mut TestAppContext) {
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
        let spec = ChartSpec {
            series: (0..SERIES).map(|_| ChartSeries::default()).collect(),
            ..ChartSpec::default()
        };
        let labels = super::labels();
        let (root, vcx) = cx.add_window_view(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            let hub = super::hub_for(&FakeEngine::new(), cx);
            let chart = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));
            Root {
                _driver: driver,
                chart,
            }
        });
        vcx.simulate_scale_factor_change(1.0);
        let chart = vcx.read_entity(&root, |r, _| r.chart.clone());

        let newest = now_boot_ns();
        let mut views: SmallVec<[Option<Arc<SeriesView>>; 2]> =
            (0..SERIES).map(|i| Some(view_of(i, newest, 1))).collect();
        chart.update(vcx, |v, cx| {
            v.set_series_for_test(views.clone(), cx);
            v.set_state(ChartState::Live, cx);
        });
        draw(vcx);
        let first = counts(vcx, &chart);
        assert_eq!(first, (1, 1), "the first paint builds once");

        for _ in 0..120 {
            draw(vcx);
        }
        assert_eq!(
            counts(vcx, &chart),
            first,
            "120 unchanged paints: 0 key rebuilds, 0 geometry builds"
        );

        views[7] = Some(view_of(7, newest, 2));
        chart.update(vcx, |v, cx| v.set_series_for_test(views.clone(), cx));
        draw(vcx);
        draw(vcx);
        assert_eq!(
            counts(vcx, &chart),
            (first.0 + 1, first.1 + 1),
            "one version bump: exactly 1 key rebuild and 1 geometry build"
        );
    }
}

// ChartGridElement and ChartGridView: one element for n cells (E19, tasklens-yrf.12).

mod grid_element {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext, Context, Entity, Hsla, IntoElement, Modifiers, ParentElement, Render,
        StyleRefinement, Styled, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use smallvec::smallvec;
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::snapshot::{SeriesSet, Snapshot};
    use tl_model::time::now_boot_ns;
    use tl_ui::chart::LegendStyle;
    use tl_ui::chart::grid_element::{
        ChartGridSpec, ChartGridView, GridCellSpec, grid_height, grid_layout,
    };
    use tl_ui::testing::fake_engine::FakeEngine;

    const MS: u64 = 1_000_000;

    fn core(i: u16) -> SeriesId {
        SeriesId {
            device: DeviceIdx(i),
            metric: MetricId::CPU_TOTAL,
        }
    }

    fn kernel(i: u16) -> SeriesId {
        SeriesId {
            device: DeviceIdx(i),
            metric: MetricId::CPU_KERNEL,
        }
    }

    const MEM: SeriesId = SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::MEM_USED,
    };

    /// 20 samples 100 ms apart at `value`, the newest now.
    fn series(id: SeriesId, version: u64, value: f32) -> Arc<SeriesView> {
        let newest = now_boot_ns();
        let samples: Vec<Sample> = (0..20u64)
            .map(|i| Sample::valid(newest - (19 - i) * 100 * MS, value))
            .collect();
        Arc::new(SeriesView {
            id,
            version,
            period_ns: 100 * MS,
            samples: samples.into(),
        })
    }

    /// Publishes `n` cores (with kernel overlays when `overlay`) and the memory series, every
    /// core at `value(i)`, versions `core_v` and `mem_v`.
    fn publish(
        engine: &Arc<FakeEngine>,
        n: u16,
        overlay: bool,
        core_v: u64,
        mem_v: u64,
        value: impl Fn(u16) -> f32,
    ) {
        let mut views = vec![series(MEM, mem_v, 40.0)];
        for i in 0..n {
            views.push(series(core(i), core_v, value(i)));
            if overlay {
                views.push(series(kernel(i), core_v, value(i) / 2.0));
            }
        }
        views.sort_by_key(|v| v.id);
        engine.publish_with(|s: &mut Snapshot| {
            s.series = Arc::new(SeriesSet {
                header: s.series.header,
                series: Arc::from(views),
                slow: Arc::from([]),
            });
        });
    }

    fn spec(n: u16, overlay: bool) -> ChartGridSpec {
        let color = |h: f32, a: f32| Hsla {
            h,
            s: 0.6,
            l: 0.5,
            a,
        };
        ChartGridSpec {
            cells: (0..n)
                .map(|i| GridCellSpec {
                    heading: format!("CPU {i}").into(),
                    fill_series: core(i),
                    overlay_series: overlay.then_some(kernel(i)),
                    offline: false,
                    mark: None,
                    tooltip: None,
                })
                .collect(),
            fill: color(0.5, 0.2),
            line: color(0.5, 1.0),
            overlay_line: color(0.1, 1.0),
            ..ChartGridSpec::default()
        }
    }

    struct Root {
        grid: Entity<ChartGridView>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(1300.)).child(
                self.grid
                    .clone()
                    .cached(StyleRefinement::default().w_full()),
            )
        }
    }

    /// A window, 1300 px wide at scale 1, holding one grid for `spec` bound to a hub over
    /// `engine`.
    fn grid_in_window<'a>(
        cx: &'a mut TestAppContext,
        engine: &Arc<FakeEngine>,
        spec: ChartGridSpec,
    ) -> (Entity<ChartGridView>, &'a mut VisualTestContext) {
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
        let (root, vcx) = cx.add_window_view(|_, cx| {
            let hub = super::hub_for(engine, cx);
            let grid = cx.new(|cx| ChartGridView::new(spec, &hub, cx));
            Root { grid }
        });
        vcx.simulate_scale_factor_change(1.0);
        vcx.run_until_parked();
        let grid = vcx.read_entity(&root, |r, _| r.grid.clone());
        (grid, vcx)
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    #[test]
    fn grid_columns_for_16_is_6() {
        let l = grid_layout(16, px(1000.), px(120.), px(8.));
        assert_eq!((l.cols, l.rows), (6, 3));
        assert_eq!(l.cell_w, (px(1000.) - px(40.)) / 6.0);
    }

    #[test]
    fn grid_columns_for_32_is_8() {
        let l = grid_layout(32, px(1300.), px(120.), px(8.));
        assert_eq!((l.cols, l.rows), (8, 4));
        assert_eq!(grid_height(&l, px(112.), px(8.)), px(472.));
    }

    #[test]
    fn grid_columns_drop_when_narrow() {
        let l = grid_layout(16, px(500.), px(120.), px(8.));
        assert_eq!((l.cols, l.rows), (3, 6));
        let one = grid_layout(4, px(50.), px(120.), px(8.));
        assert_eq!(one.cols, 1, "at least one column");
    }

    #[gpui_kit::test]
    fn grid_one_fill_path_for_all_cells(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, 32, true, 1, 1, |i| 10.0 + f32::from(i));
        let (grid, vcx) = grid_in_window(cx, &engine, spec(32, true));
        draw(vcx);
        let before = vcx.read_entity(&grid, |v, _| v.path_paints.get());
        // A cached view replays its last paint unless notified.
        vcx.update(|_, cx| grid.update(cx, |_, cx| cx.notify()));
        draw(vcx);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.built_layout().map(|l| (l.cols, l.rows)), Some((8, 4)));
            assert_eq!(v.cached_fill_stroke_counts(), (1, 2));
            assert_eq!(v.path_paints.get() - before, 3, "3 path paints per frame");
            for i in 0..32 {
                assert!(v.vertices_in_cell(i) > 0, "cell {i} has geometry");
            }
        });
    }

    #[gpui_kit::test]
    fn grid_offline_cell_draws_no_path(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        // Cell 5's series has data too: the offline flag alone keeps it out.
        publish(&engine, 16, true, 1, 1, |_| 60.0);
        let mut s = spec(16, true);
        s.cells[5].offline = true;
        let (grid, vcx) = grid_in_window(cx, &engine, s);
        draw(vcx);
        draw(vcx);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.vertices_in_cell(5), 0);
            assert!(v.vertices_in_cell(4) > 0);
            assert!(v.vertices_in_cell(6) > 0);
            assert_eq!(v.offline_shapes.get(), 1, "\"Offline\" shaped once");
        });
    }

    #[gpui_kit::test]
    fn grid_rerenders_only_on_own_series(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, 4, false, 1, 1, |_| 50.0);
        let (grid, vcx) = grid_in_window(cx, &engine, spec(4, false));
        let count =
            |vcx: &mut VisualTestContext| vcx.read_entity(&grid, |v, _| v.render_count.get());
        let first = count(vcx);

        publish(&engine, 4, false, 1, 2, |_| 50.0);
        vcx.run_until_parked();
        assert_eq!(count(vcx), first, "memory changed, no core did");

        publish(&engine, 4, false, 2, 2, |_| 50.0);
        vcx.run_until_parked();
        assert_eq!(count(vcx), first + 1, "the core series changed");
    }

    #[gpui_kit::test]
    fn grid_values_reshape_per_change(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        let (grid, vcx) = grid_in_window(cx, &engine, spec(32, false));
        let shapes = |vcx: &mut VisualTestContext| {
            vcx.read_entity(&grid, |v, _| (v.value_shapes.get(), v.heading_shapes.get()))
        };
        draw(vcx);
        let (v0, _) = shapes(vcx);
        for k in 1..=10u64 {
            publish(&engine, 32, false, k, 1, |i| {
                (k * 3) as f32 + f32::from(i) / 100.0
            });
            vcx.run_until_parked();
            draw(vcx);
            draw(vcx);
        }
        let (v1, h1) = shapes(vcx);
        assert_eq!(v1 - v0, 32 * 10, "one value shape per cell per change");
        assert_eq!(h1, 32, "headings shaped once");
    }

    /// Moves the pointer to `(x, y)` and draws.
    fn hover(vcx: &mut VisualTestContext, x: f32, y: f32) {
        vcx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
        vcx.run_until_parked();
        draw(vcx);
    }

    #[gpui_kit::test]
    fn grid_no_hitboxes_when_pointer_outside(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, 16, false, 1, 1, |_| 30.0);
        let (grid, vcx) = grid_in_window(cx, &engine, spec(16, false));
        draw(vcx);
        // 16 cells, 6 columns, 3 rows: the grid ends at y = 352.
        hover(vcx, 100., 500.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hitbox_count.get(), 0, "no hitbox while outside");
            assert_eq!(v.hovered_cell(), None);
        });
    }

    #[gpui_kit::test]
    fn grid_heading_tooltip_for_hovered_cell(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, 16, false, 1, 1, |_| 30.0);
        let mut s = spec(16, false);
        for (i, c) in s.cells.iter_mut().enumerate() {
            c.tooltip = Some(format!("Core {i}, thread 0, 4200 MHz").into());
        }
        let (grid, vcx) = grid_in_window(cx, &engine, s);
        hover(vcx, 100., 500.);
        let builds = vcx.read_entity(&grid, |v, _| v.build_count.get());

        // 6 columns of (1300 - 5 × 8) / 6 = 210 px: cell 7 is column 1, row 1, at (218, 120).
        // Its heading starts `sp3` (6 px) in from that corner.
        hover(vcx, 218. + 12., 120. + 14.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hitbox_count.get(), 16, "one hitbox per heading");
            assert_eq!(v.hovered_cell(), Some(7));
            assert_eq!(
                v.hovered_tooltip().map(|t| t.as_ref()),
                Some("Core 7, thread 0, 4200 MHz")
            );
        });

        // Over cell 8's heading, then off every heading, then out of the element.
        hover(vcx, 436. + 12., 120. + 14.);
        assert_eq!(vcx.read_entity(&grid, |v, _| v.hovered_cell()), Some(8));
        hover(vcx, 436. + 100., 120. + 80.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hovered_cell(), None);
            assert_eq!(v.hitbox_count.get(), 16, "still inside");
        });
        hover(vcx, 100., 500.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hitbox_count.get(), 0);
            assert_eq!(v.build_count.get(), builds, "hover never rebuilds geometry");
        });
    }

    #[gpui_kit::test]
    fn grid_legend_in_cell_0_only(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, 16, true, 1, 1, |_| 30.0);
        let mut s = spec(16, true);
        s.legend = smallvec![
            ("Total".into(), LegendStyle::Solid),
            ("Kernel".into(), LegendStyle::Dashed),
        ];
        let (grid, vcx) = grid_in_window(cx, &engine, s);
        draw(vcx);
        vcx.update(|_, cx| grid.update(cx, |_, cx| cx.notify()));
        draw(vcx);
        assert_eq!(vcx.read_entity(&grid, |v, _| v.legend_shapes()), 2);
    }
}

/// tasklens-yrf.20: the offline text is a Fluent key read once per strings version, and every
/// cell draws the dotted baseline across its own data gaps.
mod grid_offline_and_gaps {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext, Context, Entity, Hsla, IntoElement, ParentElement, Render, StyleRefinement,
        Styled, TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SampleFlags, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::snapshot::{SeriesSet, Snapshot};
    use tl_model::time::now_boot_ns;
    use tl_ui::chart::grid_element::{ChartGridSpec, ChartGridView, GridCellSpec};
    use tl_ui::testing::fake_engine::FakeEngine;

    const MS: u64 = 1_000_000;

    fn core(i: u16) -> SeriesId {
        SeriesId {
            device: DeviceIdx(i),
            metric: MetricId::CPU_TOTAL,
        }
    }

    /// 20 samples 100 ms apart, the newest 60 s ahead of now so a slow test run never turns the
    /// series stale. With `gap`, samples 8 to 11 are not valid, so the series has two runs with
    /// 500 ms between them.
    fn series(id: SeriesId, version: u64, gap: bool) -> Arc<SeriesView> {
        let newest = now_boot_ns() + 60_000 * MS;
        let samples: Vec<Sample> = (0..20u64)
            .map(|i| {
                let s = Sample::valid(newest - (19 - i) * 100 * MS, 30.0);
                if gap && (8..12).contains(&i) {
                    Sample {
                        flags: SampleFlags::empty(),
                        ..s
                    }
                } else {
                    s
                }
            })
            .collect();
        Arc::new(SeriesView {
            id,
            version,
            period_ns: 100 * MS,
            samples: samples.into(),
        })
    }

    /// Publishes `n` cores; core `gap_cell` has the gap.
    fn publish(engine: &Arc<FakeEngine>, n: u16, version: u64, gap_cell: Option<u16>) {
        let views: Vec<_> = (0..n)
            .map(|i| series(core(i), version, gap_cell == Some(i)))
            .collect();
        engine.publish_with(|s: &mut Snapshot| {
            s.series = Arc::new(SeriesSet {
                header: s.series.header,
                series: Arc::from(views),
                slow: Arc::from([]),
            });
        });
    }

    fn spec(n: u16) -> ChartGridSpec {
        let color = |a: f32| Hsla {
            h: 0.5,
            s: 0.6,
            l: 0.5,
            a,
        };
        ChartGridSpec {
            cells: (0..n)
                .map(|i| GridCellSpec {
                    heading: format!("CPU {i}").into(),
                    fill_series: core(i),
                    overlay_series: None,
                    offline: false,
                    mark: None,
                    tooltip: None,
                })
                .collect(),
            fill: color(0.2),
            line: color(1.0),
            overlay_line: color(1.0),
            ..ChartGridSpec::default()
        }
    }

    struct Root {
        grid: Entity<ChartGridView>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(1300.)).child(
                self.grid
                    .clone()
                    .cached(StyleRefinement::default().w_full()),
            )
        }
    }

    /// A 1300 px window at scale 1 with English strings, holding one grid for `spec`.
    fn grid_in_window<'a>(
        cx: &'a mut TestAppContext,
        engine: &Arc<FakeEngine>,
        spec: ChartGridSpec,
    ) -> (Entity<ChartGridView>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            tl_ui::i18n::init_strings(cx, "en-US");
        });
        let (root, vcx) = cx.add_window_view(|_, cx| {
            let hub = super::hub_for(engine, cx);
            let grid = cx.new(|cx| ChartGridView::new(spec, &hub, cx));
            Root { grid }
        });
        vcx.simulate_scale_factor_change(1.0);
        vcx.run_until_parked();
        let grid = vcx.read_entity(&root, |r, _| r.grid.clone());
        (grid, vcx)
    }

    /// Notifies the grid (a cached view replays its last paint otherwise) and draws a frame.
    fn frame(vcx: &mut VisualTestContext, grid: &Entity<ChartGridView>) {
        vcx.update(|_, cx| grid.update(cx, |_, cx| cx.notify()));
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    #[gpui_kit::test]
    fn grid_offline_text_from_fluent_key(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, 4, 1, None);
        let mut s = spec(4);
        s.cells[2].offline = true;
        let (grid, vcx) = grid_in_window(cx, &engine, s);
        for _ in 0..5 {
            frame(vcx, &grid);
        }
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.offline_text().map(|t| t.as_ref()), Some("Offline"));
            assert_eq!(v.offline_reads.get(), 1, "read once per strings version");
            assert_eq!(v.offline_shapes.get(), 1);
        });

        // A language change bumps the strings version: one more read, same English text.
        vcx.update(|_, cx| tl_ui::i18n::set_language(cx, "en-US"));
        frame(vcx, &grid);
        frame(vcx, &grid);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.offline_reads.get(), 2);
            assert_eq!(v.offline_text().map(|t| t.as_ref()), Some("Offline"));
        });
    }

    #[gpui_kit::test]
    fn grid_cell_gap_paints_dotted_span(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, 4, 1, Some(1));
        let (grid, vcx) = grid_in_window(cx, &engine, spec(4));
        frame(vcx, &grid);
        let paints0 = vcx.read_entity(&grid, |v, _| v.path_paints.get());
        frame(vcx, &grid);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(
                v.path_paints.get() - paints0,
                2,
                "one fill and one line path: gaps add none"
            );
            let spans = v.last_gap_spans.borrow();
            let of = |cell: usize| -> Vec<std::ops::Range<f32>> {
                spans
                    .iter()
                    .filter(|(c, _)| *c == cell)
                    .map(|(_, r)| r.clone())
                    .collect()
            };
            // 20 samples cover 1.9 s of a 60 s window, so every cell has the left-edge span.
            for cell in [0, 2, 3] {
                assert_eq!(of(cell).len(), 1, "cell {cell}: left-edge span only");
            }
            let gap_cell = of(1);
            assert_eq!(gap_cell.len(), 2, "left-edge span and the middle gap");
            // The middle gap runs from sample 7 (1.2 s before the anchor) to sample 12 (0.7 s
            // before it): chart-local x is negative and the span is 5/7 of its end's distance.
            let mid = &gap_cell[1];
            assert!(mid.start < mid.end && mid.end < 0.0, "{mid:?}");
            let ratio = (mid.end - mid.start) / -mid.end;
            assert!((ratio - 5.0 / 7.0).abs() < 0.01, "ratio {ratio}");
        });
    }

    #[gpui_kit::test]
    fn grid_unchanged_frames_build_nothing(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, 8, 1, Some(3));
        let mut s = spec(8);
        s.cells[5].offline = true;
        let (grid, vcx) = grid_in_window(cx, &engine, s);
        frame(vcx, &grid);
        let (builds0, reads0) =
            vcx.read_entity(&grid, |v, _| (v.build_count.get(), v.offline_reads.get()));
        for _ in 0..120 {
            frame(vcx, &grid);
        }
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(
                v.build_count.get(),
                builds0,
                "120 unchanged frames build nothing"
            );
            assert_eq!(v.offline_reads.get(), reads0, "no string reads either");
            assert!(v.last_gap_spans.borrow().iter().any(|(c, _)| *c == 3));
        });
    }
}

// 5 and 10 minute ranges read the 1 Hz tier (E19, tasklens-yrf.15).

mod tier_1hz {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, px,
    };
    use smallvec::smallvec;
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::snapshot::{SeriesSet, Snapshot};
    use tl_model::time::now_boot_ns;
    use tl_ui::chart::grid_element::{ChartGridSpec, ChartGridView, GridCellSpec};
    use tl_ui::chart::view::{ChangedRuns, ChartView, search_series};
    use tl_ui::chart::{ChartSeries, ChartSpec};
    use tl_ui::testing::fake_engine::FakeEngine;

    const MS: u64 = 1_000_000;
    const S: u64 = 1_000_000_000;

    const A: SeriesId = SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::CPU_TOTAL,
    };

    fn slow(id: SeriesId) -> SeriesId {
        SeriesId {
            metric: id.metric.with_tier_1hz(),
            ..id
        }
    }

    /// `n` samples `period` apart, the newest now.
    fn view(id: SeriesId, n: u64, period: u64) -> Arc<SeriesView> {
        let newest = now_boot_ns();
        let samples: Vec<Sample> = (0..n)
            .map(|i| Sample::valid(newest - (n - 1 - i) * period, (i % 100) as f32))
            .collect();
        Arc::new(SeriesView {
            id,
            version: 1,
            period_ns: period,
            samples: samples.into(),
        })
    }

    /// Publishes `A`'s fast view (600 samples at 100 ms) and, when `with_1hz`, its 1 Hz view
    /// (600 samples at 1 s).
    fn publish(engine: &Arc<FakeEngine>, with_1hz: bool) {
        let fast = view(A, 600, 100 * MS);
        let slow_views: Vec<Arc<SeriesView>> = if with_1hz {
            vec![view(slow(A), 600, S)]
        } else {
            Vec::new()
        };
        engine.publish_with(|s: &mut Snapshot| {
            s.series = Arc::new(SeriesSet {
                header: s.series.header,
                series: Arc::from([fast]),
                slow: Arc::from(slow_views),
            });
        });
    }

    fn spec(range_ns: u64) -> ChartSpec {
        ChartSpec {
            series: smallvec![ChartSeries {
                id: A,
                ..ChartSeries::default()
            }],
            range_ns,
            ..ChartSpec::default()
        }
    }

    struct Root {
        chart: Entity<ChartView>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(602.)).h(px(202.)).child(self.chart.clone())
        }
    }

    fn init(cx: &mut TestAppContext) {
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
    }

    fn chart_in_window<'a>(
        cx: &'a mut TestAppContext,
        engine: &Arc<FakeEngine>,
        range_ns: u64,
    ) -> (Entity<ChartView>, &'a mut VisualTestContext) {
        init(cx);
        let (root, vcx) = cx.add_window_view(|_, cx| {
            let hub = super::hub_for(engine, cx);
            let chart = cx.new(|cx| ChartView::new(spec(range_ns), super::labels(), &hub, cx));
            Root { chart }
        });
        vcx.simulate_scale_factor_change(1.0);
        vcx.run_until_parked();
        let chart = vcx.read_entity(&root, |r, _| r.chart.clone());
        (chart, vcx)
    }

    /// (period_ns, sample count, tier_fallback) of the resolved series.
    fn resolved(chart: &Entity<ChartView>, vcx: &mut VisualTestContext) -> (u64, usize, bool) {
        vcx.read_entity(chart, |v, _| {
            let s = v.resolved()[0].clone().expect("series resolved");
            (s.period_ns, s.samples.len(), v.tier_fallback)
        })
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    #[gpui_kit::test]
    fn range_300s_reads_1hz_tier(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, true);
        let (chart, vcx) = chart_in_window(cx, &engine, 300 * S);
        assert_eq!(resolved(&chart, vcx), (S, 600, false));
        vcx.read_entity(&chart, |v, _| {
            assert_eq!(v.resolved()[0].as_ref().unwrap().id, slow(A));
        });
    }

    #[gpui_kit::test]
    fn range_60s_reads_fast_tier(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, true);
        let (chart, vcx) = chart_in_window(cx, &engine, 60 * S);
        assert_eq!(resolved(&chart, vcx), (100 * MS, 600, false));
    }

    #[gpui_kit::test]
    fn missing_1hz_tier_falls_back(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, false);
        let (chart, vcx) = chart_in_window(cx, &engine, 600 * S);
        assert_eq!(resolved(&chart, vcx), (100 * MS, 600, true));

        // The 1 Hz view arriving later is picked up and clears the fallback.
        publish(&engine, true);
        vcx.run_until_parked();
        assert_eq!(resolved(&chart, vcx), (S, 600, false));
    }

    #[gpui_kit::test]
    fn range_change_rebuilds_once(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        publish(&engine, true);
        let (chart, vcx) = chart_in_window(cx, &engine, 60 * S);
        draw(vcx);
        let builds =
            |vcx: &mut VisualTestContext| vcx.read_entity(&chart, |v, _| v.build_count.get());
        let before = builds(vcx);
        assert!(before >= 1, "the 60 s chart was built");
        for _ in 0..10 {
            draw(vcx);
        }
        assert_eq!(
            builds(vcx),
            before,
            "an unchanged 60 s chart builds nothing"
        );

        chart.update(vcx, |v, cx| v.set_spec(spec(300 * S), cx));
        for _ in 0..10 {
            draw(vcx);
        }
        assert_eq!(
            builds(vcx),
            before + 1,
            "60 s to 300 s: exactly 1 geometry build"
        );
        assert_eq!(resolved(&chart, vcx).0, S);
    }

    fn grid_spec(range_ns: u64) -> ChartGridSpec {
        ChartGridSpec {
            cells: vec![GridCellSpec {
                heading: "CPU 0".into(),
                fill_series: A,
                overlay_series: None,
                offline: false,
                mark: None,
                tooltip: None,
            }],
            range_ns,
            ..ChartGridSpec::default()
        }
    }

    #[gpui_kit::test]
    fn grid_follows_the_tier_rule(cx: &mut TestAppContext) {
        init(cx);
        let engine = FakeEngine::new();
        publish(&engine, false);
        let hub = cx.update(|cx| super::hub_for(&engine, cx));
        let grid = cx.new(|cx| ChartGridView::new(grid_spec(600 * S), &hub, cx));
        grid.read_with(cx, |v, _| {
            assert!(v.tier_fallback, "no 1 Hz view: fallback")
        });

        grid.update(cx, |v, cx| v.set_spec(grid_spec(60 * S), cx));
        grid.read_with(cx, |v, _| {
            assert!(!v.tier_fallback, "60 s reads the fast tier")
        });

        publish(&engine, true);
        cx.run_until_parked();
        grid.update(cx, |v, cx| v.set_spec(grid_spec(300 * S), cx));
        grid.read_with(cx, |v, _| {
            assert!(!v.tier_fallback, "300 s finds the 1 Hz view")
        });
    }

    #[test]
    fn changed_runs_search_each_tier_in_log_n() {
        // As `Snapshot::changed_since` lists them: 40 fast ids, then 24 1 Hz ids.
        let fast: Vec<SeriesId> = (0..40u16)
            .map(|i| SeriesId {
                device: DeviceIdx(i / 4),
                metric: MetricId(i % 4 * 10),
            })
            .collect();
        let slow_ids: Vec<SeriesId> = fast
            .iter()
            .step_by(2)
            .take(24)
            .map(|&id| slow(id))
            .collect();
        let changed: Vec<SeriesId> = fast.iter().chain(&slow_ids).copied().collect();
        assert!(!changed.is_sorted(), "two runs, not one sorted list");
        let bound = changed.len().ilog2() + 2;

        let runs = ChangedRuns::new(&changed);
        assert_eq!((runs.fast, runs.slow), (&fast[..], &slow_ids[..]));
        for &target in &changed {
            let mut n = 0u32;
            let found = search_series(runs.run_for(target), |c| {
                n += 1;
                c.cmp(&target)
            });
            assert_eq!(found, Some(&target));
            assert!(n <= bound, "{n} comparisons for {target:?}, bound {bound}");
            assert!(runs.contains(target));
        }

        // A 1 Hz id whose fast id is absent, and the reverse.
        let only_slow = slow_ids[3];
        let base = SeriesId {
            metric: only_slow.metric.base(),
            ..only_slow
        };
        let fast_only = ChangedRuns::new(&fast[..1]);
        assert!(!fast_only.contains(slow(fast[0])));
        let slow_only = ChangedRuns::new(&slow_ids);
        assert!(!slow_only.contains(base));
        assert!(
            slow_only.tier_changed(base, 300 * S),
            "300 s reads the 1 Hz id"
        );
        assert!(
            !slow_only.tier_changed(base, 60 * S),
            "60 s reads only the fast id"
        );
        assert!(!runs.contains(SeriesId {
            device: DeviceIdx(50),
            metric: MetricId::CPU_TOTAL,
        }));
    }
}

/// tasklens-yrf.16: hover readout. A hitbox only when `spec.hover`, the nearest valid sample,
/// the offset text, the box flip at the right edge, and no re-shaping while the index holds.
mod hover {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, Modifiers, MouseButton, ParentElement, Render,
        Styled, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use smallvec::smallvec;
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SampleFlags, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::time::now_boot_ns;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::chart::geometry::TimeMap;
    use tl_ui::chart::hover::{nearest_index, offset_text, pointer_time, window_stats};
    use tl_ui::chart::view::ChartView;
    use tl_ui::chart::{ChartSeries, ChartSpec};
    use tl_ui::testing::fake_engine::FakeEngine;

    const MS: u64 = 1_000_000;
    const S: u64 = 1_000_000_000;

    /// Lays the chart out at 602 × 202, so the plot is 1..601 by 1..201.
    struct Root {
        _driver: Entity<AnimationDriver>,
        chart: Entity<ChartView>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(602.)).h(px(202.)).child(self.chart.clone())
        }
    }

    fn spec(hover: bool) -> ChartSpec {
        ChartSpec {
            series: smallvec![ChartSeries::default()],
            hover,
            ..ChartSpec::default()
        }
    }

    /// 600 valid samples 100 ms apart, the newest at `newest`, value `i % 100`.
    fn view_of(newest: u64) -> Arc<SeriesView> {
        let samples: Vec<Sample> = (0..600u64)
            .map(|i| Sample::valid(newest - (599 - i) * 100 * MS, (i % 100) as f32))
            .collect();
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(0),
                metric: MetricId::CPU_TOTAL,
            },
            version: 1,
            period_ns: 100 * MS,
            samples: samples.into(),
        })
    }

    fn chart_in_window(
        cx: &mut TestAppContext,
        spec: ChartSpec,
        data: bool,
    ) -> (Entity<ChartView>, &mut VisualTestContext) {
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
        let (root, vcx) = cx.add_window_view(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            let hub = super::hub_for(&FakeEngine::new(), cx);
            let chart = cx.new(|cx| ChartView::new(spec, super::labels(), &hub, cx));
            Root {
                _driver: driver,
                chart,
            }
        });
        vcx.simulate_scale_factor_change(1.0);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let chart = vcx.read_entity(&root, |r, _| r.chart.clone());
        if data {
            chart.update(vcx, |v, cx| {
                v.set_series_for_test(smallvec![Some(view_of(now_boot_ns()))], cx)
            });
        }
        draw(vcx);
        (chart, vcx)
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    /// Moves the pointer to `(x, y)` and draws.
    fn hover(vcx: &mut VisualTestContext, x: f32, y: f32) {
        vcx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
        vcx.run_until_parked();
        draw(vcx);
    }

    fn sample(t_ms: u64, valid: bool) -> Sample {
        Sample {
            t_ns: t_ms * MS,
            value: 1.0,
            flags: if valid {
                SampleFlags::VALID
            } else {
                SampleFlags::empty()
            },
        }
    }

    #[gpui_kit::test]
    fn hover_no_hitbox_when_disabled(cx: &mut TestAppContext) {
        let (chart, vcx) = chart_in_window(cx, spec(false), true);
        hover(vcx, 300., 100.);
        vcx.read_entity(&chart, |v, _| {
            let h = v.hover_state();
            assert!(!h.hitbox_inserted, "no hitbox without spec.hover");
            assert_eq!(h.pinned_t_ns, None);
            assert_eq!(h.last_box, None);
        });

        chart.update(vcx, |v, cx| v.set_spec(spec(true), cx));
        chart.update(vcx, |v, cx| {
            v.set_series_for_test(smallvec![Some(view_of(now_boot_ns()))], cx)
        });
        draw(vcx);
        hover(vcx, 300., 101.);
        vcx.read_entity(&chart, |v, _| {
            assert!(v.hover_state().hitbox_inserted);
            assert!(v.hover_state().last_box.is_some());
        });
    }

    #[test]
    fn hover_picks_nearest_sample() {
        let s = [sample(0, true), sample(100, true), sample(200, true)];
        assert_eq!(nearest_index(&s, 140 * MS), Some(1));
        assert_eq!(nearest_index(&s, 160 * MS), Some(2));
        assert_eq!(nearest_index(&s, 500 * MS), Some(2), "past the newest");
        let gap = [sample(0, true), sample(100, true), sample(200, false)];
        assert_eq!(
            nearest_index(&gap, 160 * MS),
            Some(1),
            "invalid sample skipped"
        );
        assert_eq!(nearest_index(&[sample(0, false)], 0), None);

        let map = TimeMap {
            anchor_ns: 60 * S,
            right_edge_ns: 60 * S,
            range_ns: 60 * S,
            plot_width: 600.0,
        };
        assert_eq!(pointer_time(&map, 600.0, 600.0), 60 * S);
        assert_eq!(pointer_time(&map, 600.0, 580.0), 58 * S);
        assert_eq!(pointer_time(&map, 600.0, -10.0), 0, "clamped to the plot");

        let stats = window_stats(&gap, 1);
        assert_eq!(stats.samples, 1, "valid samples only");
    }

    #[test]
    fn hover_offset_text() {
        let mut out = String::new();
        offset_text(12_400 * MS, 60 * S, &mut out);
        assert_eq!(out, "-12 s");
        out.clear();
        offset_text(252 * S, 300 * S, &mut out);
        assert_eq!(out, "-4 m 12 s");
        out.clear();
        offset_text(252 * S, 600 * S, &mut out);
        assert_eq!(out, "-4 m 12 s");
        out.clear();
        offset_text(0, 60 * S, &mut out);
        assert_eq!(out, "0 s");
    }

    #[gpui_kit::test]
    fn hover_box_flips_at_right_edge(cx: &mut TestAppContext) {
        let (chart, vcx) = chart_in_window(cx, spec(true), true);
        // 10 px per second: x = 581 is 2 s before the newest sample, 20 px from the right edge.
        hover(vcx, 581., 100.);
        vcx.read_entity(&chart, |v, _| {
            let h = v.hover_state();
            assert!(h.hitbox_inserted, "hitbox");
            assert!(h.pinned_t_ns.is_some(), "pinned");
            let b = h.last_box.expect("box painted");
            let x = h.last_marker_x.expect("marker painted");
            assert_eq!(x, px(581.));
            assert!(
                b.right() <= px(601.),
                "flipped left of the plot's right edge"
            );
            assert!(b.right() < x, "box left of the marker");
            assert_eq!(h.index, Some(579));
            assert_eq!(h.line_text(0), Some("-2 s"));
            assert_eq!(h.line_text(1), Some("CPU"), "heading when no legend");
            assert_eq!(h.line_text(4), Some("600 samples"));
        });

        hover(vcx, 100., 100.);
        vcx.read_entity(&chart, |v, _| {
            let h = v.hover_state();
            let b = h.last_box.expect("box painted");
            assert!(
                b.left() > h.last_marker_x.unwrap(),
                "box right of the marker"
            );
        });
    }

    #[gpui_kit::test]
    fn hover_hidden_without_valid_samples(cx: &mut TestAppContext) {
        let (chart, vcx) = chart_in_window(cx, spec(true), false);
        hover(vcx, 300., 100.);
        vcx.read_entity(&chart, |v, _| {
            assert!(v.hover_state().hitbox_inserted);
            assert_eq!(
                v.hover_state().last_box,
                None,
                "WarmingUp paints no readout"
            );
            assert_eq!(v.hover_state().last_marker_x, None);
        });
    }

    #[gpui_kit::test]
    fn hover_cleared_on_mouse_down(cx: &mut TestAppContext) {
        let (chart, vcx) = chart_in_window(cx, spec(true), true);
        hover(vcx, 300., 100.);
        assert!(vcx.read_entity(&chart, |v, _| v.hover_state().last_box.is_some()));
        vcx.simulate_mouse_down(
            point(px(300.), px(100.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        vcx.run_until_parked();
        draw(vcx);
        vcx.read_entity(&chart, |v, _| {
            assert_eq!(v.hover_state().pinned_t_ns, None);
            assert_eq!(v.hover_state().last_box, None, "marker gone");
        });

        // Leaving the plot hides it as well.
        hover(vcx, 300., 100.);
        assert!(vcx.read_entity(&chart, |v, _| v.hover_state().last_box.is_some()));
        hover(vcx, 300., 400.);
        assert_eq!(
            vcx.read_entity(&chart, |v, _| v.hover_state().last_box),
            None
        );

        // So does window deactivation, on the next paint.
        hover(vcx, 300., 100.);
        assert!(vcx.read_entity(&chart, |v, _| v.hover_state().last_box.is_some()));
        vcx.deactivate_window();
        draw(vcx);
        vcx.read_entity(&chart, |v, _| {
            assert_eq!(v.hover_state().pinned_t_ns, None);
            assert_eq!(v.hover_state().last_box, None);
        });
    }

    #[gpui_kit::test]
    fn hover_reshapes_only_on_index_change(cx: &mut TestAppContext) {
        let (chart, vcx) = chart_in_window(cx, spec(true), true);
        let builds = vcx.read_entity(&chart, |v, _| v.build_count.get());
        hover(vcx, 300., 100.);
        let first = vcx.read_entity(&chart, |v, _| v.hover_state().shape_count);
        assert!(first > 0);
        draw(vcx);
        draw(vcx);
        let (same, index) = vcx.read_entity(&chart, |v, _| {
            (v.hover_state().shape_count, v.hover_state().index)
        });
        assert_eq!(same, first, "same index: no shaping");
        assert!(index.is_some());

        hover(vcx, 200., 100.);
        let moved = vcx.read_entity(&chart, |v, _| v.hover_state().shape_count);
        assert!(moved > first, "new index re-shapes");
        assert_eq!(
            vcx.read_entity(&chart, |v, _| v.build_count.get()),
            builds,
            "hover never rebuilds geometry"
        );
    }
}

/// tasklens-yrf.17: the hover readout per grid cell, with plot hitboxes only while the pointer is
/// inside the grid.
mod grid_hover {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext, Context, Entity, Hsla, IntoElement, Modifiers, ParentElement, Render,
        StyleRefinement, Styled, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::time::now_boot_ns;
    use tl_ui::chart::grid_element::{ChartGridSpec, ChartGridView, GridCellSpec};
    use tl_ui::testing::fake_engine::FakeEngine;

    const MS: u64 = 1_000_000;

    fn core(i: u16) -> SeriesId {
        SeriesId {
            device: DeviceIdx(i),
            metric: MetricId::CPU_TOTAL,
        }
    }

    fn spec(n: u16, hover: bool) -> ChartGridSpec {
        let color = |a: f32| Hsla {
            h: 0.5,
            s: 0.6,
            l: 0.5,
            a,
        };
        ChartGridSpec {
            cells: (0..n)
                .map(|i| GridCellSpec {
                    heading: format!("CPU {i}").into(),
                    fill_series: core(i),
                    overlay_series: None,
                    offline: false,
                    mark: None,
                    tooltip: None,
                })
                .collect(),
            fill: color(0.2),
            line: color(1.0),
            overlay_line: color(1.0),
            hover,
            ..ChartGridSpec::default()
        }
    }

    /// 600 valid samples 100 ms apart, the newest at `newest`, at `value`: 60 s of data.
    fn view_of(i: u16, newest: u64, value: f32) -> Arc<SeriesView> {
        let samples: Vec<Sample> = (0..600u64)
            .map(|k| Sample::valid(newest - (599 - k) * 100 * MS, value))
            .collect();
        Arc::new(SeriesView {
            id: core(i),
            version: 1,
            period_ns: 100 * MS,
            samples: samples.into(),
        })
    }

    struct Root {
        grid: Entity<ChartGridView>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(1300.)).child(
                self.grid
                    .clone()
                    .cached(StyleRefinement::default().w_full()),
            )
        }
    }

    /// A window, 1300 px wide at scale 1, holding one 16-cell grid: 6 columns of 210 px with
    /// 8 px gaps, rows of 112 px. Cell `i` reads `10 + i`.
    fn grid_in_window(
        cx: &mut TestAppContext,
        spec: ChartGridSpec,
    ) -> (Entity<ChartGridView>, &mut VisualTestContext) {
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
        let n = spec.cells.len() as u16;
        let (root, vcx) = cx.add_window_view(|_, cx| {
            let hub = super::hub_for(&FakeEngine::new(), cx);
            let grid = cx.new(|cx| ChartGridView::new(spec, &hub, cx));
            Root { grid }
        });
        vcx.simulate_scale_factor_change(1.0);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let grid = vcx.read_entity(&root, |r, _| r.grid.clone());
        let newest = now_boot_ns();
        grid.update(vcx, |v, cx| {
            let series = (0..n)
                .map(|i| (Some(view_of(i, newest, 10.0 + f32::from(i))), None))
                .collect();
            v.set_series_for_test(series, cx);
        });
        draw(vcx);
        (grid, vcx)
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    /// Moves the pointer to `(x, y)` and draws.
    fn hover(vcx: &mut VisualTestContext, x: f32, y: f32) {
        vcx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
        vcx.run_until_parked();
        draw(vcx);
    }

    #[gpui_kit::test]
    fn grid_hover_marker_in_hovered_cell_only(cx: &mut TestAppContext) {
        let (grid, vcx) = grid_in_window(cx, spec(16, true));
        let builds = vcx.read_entity(&grid, |v, _| v.build_count.get());
        // Cell 9 is column 3, row 1, at (654, 120); its plot is 655..863 by 121..231.
        hover(vcx, 760., 180.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hitbox_count.get(), 32, "16 headings and 16 plots");
            assert_eq!(v.hover_cell(), Some(9));
            let h = v.hover_state();
            assert!(h.pinned_t_ns.is_some(), "pinned");
            let x = h.last_marker_x.expect("marker painted");
            assert!(
                x >= px(655.) && x <= px(863.),
                "marker in cell 9's plot: {x:?}"
            );
            assert!(
                (f32::from(x) - 760.).abs() <= 1.0,
                "marker under the pointer"
            );
            assert!(h.last_box.is_some(), "box painted");
            assert_eq!(h.line_text(1), Some("CPU 9"), "the cell heading");
        });

        // Off every plot but inside the grid (a gap), then out of the grid.
        hover(vcx, 650., 180.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hover_cell(), None);
            assert_eq!(v.hover_state().last_marker_x, None, "no marker in a gap");
        });
        hover(vcx, 100., 500.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hitbox_count.get(), 0, "no hitbox while outside");
            assert_eq!(v.hover_state().last_box, None);
            assert_eq!(v.build_count.get(), builds, "hover never rebuilds geometry");
        });
    }

    #[gpui_kit::test]
    fn grid_hover_box_clamped_to_element(cx: &mut TestAppContext) {
        let (grid, vcx) = grid_in_window(cx, spec(16, true));
        // Cell 5 is the right-most column, 1090..1300.
        hover(vcx, 1290., 60.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hover_cell(), Some(5));
            let h = v.hover_state();
            let b = h.last_box.expect("box painted");
            assert!(b.right() <= px(1300.), "inside the element: {b:?}");
            assert!(b.right() < h.last_marker_x.unwrap(), "flipped left");
        });

        // Cell 4 (872..1082) keeps the box right of the marker past the cell's edge: the clamp
        // is the element, not the cell.
        hover(vcx, 1000., 60.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hover_cell(), Some(4));
            let h = v.hover_state();
            let b = h.last_box.expect("box painted");
            assert!(b.left() > h.last_marker_x.unwrap(), "right of the marker");
            assert!(b.right() > px(1082.) && b.right() <= px(1300.), "{b:?}");
        });
    }

    #[gpui_kit::test]
    fn grid_hover_reshapes_on_cell_change(cx: &mut TestAppContext) {
        let (grid, vcx) = grid_in_window(cx, spec(16, true));
        let builds = vcx.read_entity(&grid, |v, _| v.build_count.get());
        // Cell 3 starts at x = 654, cell 4 at x = 872: the same cell-local x, the same time.
        hover(vcx, 654. + 100., 60.);
        let (first, index) = vcx.read_entity(&grid, |v, _| {
            (v.hover_state().shape_count, v.hover_state().index)
        });
        assert!(index.is_some());
        draw(vcx);
        assert_eq!(
            vcx.read_entity(&grid, |v, _| v.hover_state().shape_count),
            first,
            "same cell, same index: no shaping"
        );

        hover(vcx, 872. + 100., 60.);
        let (moved, h) = vcx.read_entity(&grid, |v, _| {
            let h = v.hover_state();
            (h.shape_count, (h.index, h.last_index, v.hover_cell()))
        });
        assert_eq!(h, (index, index, Some(4)), "same sample in the new cell");
        assert!(moved > first, "the cell change re-shapes");
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hover_state().line_text(1), Some("CPU 4"));
            assert_eq!(v.hover_state().line_text(2), Some("14.0%"));
        });
        draw(vcx);
        draw(vcx);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hover_state().shape_count, moved, "shaped once");
            assert_eq!(v.build_count.get(), builds, "hover never rebuilds geometry");
        });
    }

    #[gpui_kit::test]
    fn grid_hover_disabled_only_heading_hitboxes(cx: &mut TestAppContext) {
        let (grid, vcx) = grid_in_window(cx, spec(16, false));
        hover(vcx, 760., 180.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hitbox_count.get(), 16, "heading hitboxes only");
            assert_eq!(v.hover_cell(), None);
            assert_eq!(v.hover_state().pinned_t_ns, None);
            assert_eq!(v.hover_state().last_box, None);
        });

        // With hover on, an offline cell has no plot hitbox.
        let mut s = spec(16, true);
        s.cells[9].offline = true;
        grid.update(vcx, |v, cx| v.set_spec(s, cx));
        draw(vcx);
        hover(vcx, 761., 180.);
        vcx.read_entity(&grid, |v, _| {
            assert_eq!(v.hitbox_count.get(), 16 + 15);
            assert_eq!(v.hover_cell(), None, "offline cell 9 has no readout");
            assert_eq!(v.hover_state().last_box, None);
        });
    }
}
