//! Performance tab Overview page (E24, UI-PRF-017, D-022): the drawing parts. Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use gpui_kit::px;
use tl_ui::pages::performance::overview::parts::{
    CoreBarsModel, CoreCell, CoreCellFit, clock_bucket, core_bars_layout, core_bars_layout_marked,
    core_cell, meter_fill_w,
};

#[test]
fn meter_fill_width_rounds_to_physical_px() {
    let sf = 1.1;
    assert_eq!(
        meter_fill_w(0.5, px(101.), sf),
        px((50.5f32 * sf).round() / sf)
    );
    assert_eq!(meter_fill_w(0.0001, px(101.), sf), px(1. / sf));
    assert_eq!(meter_fill_w(1.4, px(101.), sf), px(101.));
    assert_eq!(meter_fill_w(0., px(101.), sf), px(0.));
    assert_eq!(meter_fill_w(-0.2, px(101.), sf), px(0.));
    assert_eq!(meter_fill_w(f32::NAN, px(101.), sf), px(0.));
}

/// D-029, UI-PRF-059: 18 px rows in the fewest columns that fit them, at most 4, then the rows
/// scroll.
#[test]
fn core_bars_layout_32_cores() {
    let l = core_bars_layout(32, px(776.), px(600.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (1, 32, px(18.)));
    assert!(!l.scrolls);
    // 32 rows of 18 px need 576 px: a lower part takes the fewest columns that fit.
    let l = core_bars_layout(32, px(776.), px(520.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (2, 16, px(18.)));
    assert!(!l.scrolls);
    let l = core_bars_layout(32, px(776.), px(400.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (2, 16, px(18.)));
    let l = core_bars_layout(32, px(776.), px(200.), 1.);
    assert_eq!((l.cols, l.rows), (3, 11));
    let l = core_bars_layout(32, px(776.), px(144.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (4, 8, px(18.)));
    assert!(!l.scrolls);
    // Not even 4 columns fit: 18 px rows that scroll, with 16 px kept for the scrollbar.
    let l = core_bars_layout(32, px(776.), px(143.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (4, 8, px(18.)));
    assert!(l.scrolls);
    let l = core_bars_layout(32, px(776.), px(50.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (4, 8, px(18.)));
    assert!(l.scrolls);
    assert_eq!(l.cell_w, px((760. - 24.) / 4.));
    assert_eq!(l.content_h(), px(144.));
    // No column is left empty: 9 cores in 3 rows take 3 columns, not 4.
    let l = core_bars_layout(9, px(776.), px(40.), 1.);
    assert_eq!((l.cols, l.rows), (3, 3));
    // A cell keeps its name and percent: a 150 px part holds 1 column and scrolls.
    let l = core_bars_layout(8, px(150.), px(60.), 1.);
    assert_eq!((l.cols, l.rows, l.cell_w), (1, 8, px(134.)));
    assert!(l.scrolls);
}

/// UI-PRF-059: core rows are 18 px apart at every part height, core count and scale; they
/// never shrink to share the height, and scrolling rows keep the same pitch.
#[test]
fn core_rows_always_18px_apart() {
    for scale in [1., 1.5] {
        for n in [1, 4, 8, 12, 16, 24, 32, 64, 128] {
            for h in (0..=1200).step_by(7) {
                let l = core_bars_layout(n, px(776.), px(h as f32), scale);
                assert_eq!(l.row_h, px(18. * scale), "{n} cores in {h} px at {scale}x");
                if !l.scrolls {
                    assert!(l.content_h() <= px(h as f32 + 1e-3), "{n} in {h}: {l:?}");
                }
            }
        }
    }
}

/// D-029: C0 to C15 fill the first column top to bottom, C16 to C31 the second.
#[test]
fn core_rows_fill_columns_top_to_bottom() {
    let l = core_bars_layout(32, px(438.), px(456.), 1.);
    assert_eq!((l.cols, l.rows), (2, 16));
    assert_eq!(l.cell_of(0), (0, 0));
    assert_eq!(l.cell_of(15), (0, 15));
    assert_eq!(l.cell_of(16), (1, 0));
    assert_eq!(l.cell_of(31), (1, 15));
}

/// crw.53: the CPU box at 1200 × 900 is 776 × 516. Its body is 750 × 456 and the side part
/// beside the 40% chart is 750 × 0.6 − 12 = 438 px wide: 2 columns of 215 px, 16 rows of 18 px.
#[test]
fn core_rows_fit_32_threads_at_1200x900() {
    let l = core_bars_layout(32, px(438.), px(456.), 1.);
    assert_eq!((l.cols, l.rows), (2, 16));
    assert_eq!(l.cell_w, px(215.));
    assert_eq!(l.row_h, px(18.));
    assert!(!l.scrolls);
    assert_eq!(l.fit, CoreCellFit::Full);
    assert_eq!(
        core_cell(&l, 1.),
        CoreCell {
            spark_x: px(32.),
            spark_w: px(48.),
            meter_x: px(84.),
            meter_w: px(43.),
            pct_right: px(167.),
            ghz: true,
        }
    );
}

/// D-029: a narrower cell drops the GHz label first, then shrinks the sparkline, then the
/// meter. The name and the percent stay.
#[test]
fn core_cell_shrinks_spark_then_meter() {
    let cell = |w: f32| {
        let l = core_bars_layout(32, px(w), px(144.), 1.);
        assert_eq!(l.cols, 4);
        (l.fit, core_cell(&l, 1.))
    };
    // 188 px cells hold every part with a 16 px meter.
    let (fit, c) = cell(4. * 188. + 24.);
    assert_eq!(fit, CoreCellFit::Full);
    assert_eq!((c.spark_w, c.meter_w, c.ghz), (px(48.), px(16.), true));
    // 140 px: no GHz, the percent ends at the cell's right edge.
    let (fit, c) = cell(4. * 140. + 24.);
    assert_eq!(fit, CoreCellFit::NoGhz);
    assert_eq!(
        c,
        CoreCell {
            spark_x: px(32.),
            spark_w: px(48.),
            meter_x: px(84.),
            meter_w: px(16.),
            pct_right: px(140.),
            ghz: false,
        }
    );
    // 119 px: the sparkline shrinks to 27 px, the meter keeps 16 px.
    let (fit, c) = cell(4. * 119. + 24.);
    assert_eq!(fit, CoreCellFit::SparkShrinks);
    assert_eq!(
        c,
        CoreCell {
            spark_x: px(32.),
            spark_w: px(27.),
            meter_x: px(63.),
            meter_w: px(16.),
            pct_right: px(119.),
            ghz: false,
        }
    );
    // 104 px: the sparkline is down to 12 px, the meter still 16 px.
    let (fit, c) = cell(4. * 104. + 24.);
    assert_eq!(fit, CoreCellFit::SparkShrinks);
    assert_eq!((c.spark_w, c.meter_w), (px(12.), px(16.)));
    // 100 px: the sparkline keeps 12 px, the meter shrinks to 12 px.
    let (fit, c) = cell(4. * 100. + 24.);
    assert_eq!(fit, CoreCellFit::MeterShrinks);
    assert_eq!(
        c,
        CoreCell {
            spark_x: px(32.),
            spark_w: px(12.),
            meter_x: px(48.),
            meter_w: px(12.),
            pct_right: px(100.),
            ghz: false,
        }
    );
    // 88 px: the meter is gone, the 12 px sparkline stays.
    let (fit, c) = cell(4. * 88. + 24.);
    assert_eq!(fit, CoreCellFit::MeterShrinks);
    assert_eq!((c.spark_w, c.meter_w), (px(12.), px(0.)));
    // 86 px: no room for the sparkline, the meter takes what is left.
    let (fit, c) = cell(4. * 86. + 24.);
    assert_eq!(fit, CoreCellFit::NoSpark);
    assert_eq!(
        (c.spark_w, c.meter_x, c.meter_w),
        (px(0.), px(32.), px(14.))
    );
    // 72 px: name and percent only.
    let (fit, c) = cell(4. * 72. + 24.);
    assert_eq!(fit, CoreCellFit::NoSpark);
    assert_eq!((c.meter_w, c.pct_right), (px(0.), px(72.)));
    // UI scale widens every part.
    let l = core_bars_layout(32, px(2. * (4. * 188. + 24.)), px(288.), 2.);
    assert_eq!((l.cols, l.fit), (4, CoreCellFit::Full));
}

#[test]
fn core_ghz_bucket_change_only() {
    assert_eq!(clock_bucket(3949.), 3900);
    assert_eq!(clock_bucket(3950.), 4000);
    assert_eq!(clock_bucket(-5.), 0);
    // Whole MHz under 1 GHz; 999.6 MHz rounds up to "1.0 GHz".
    assert_eq!(clock_bucket(812.4), 812);
    assert_eq!(clock_bucket(999.6), 1000);
    let mut model = CoreBarsModel::default();
    model.set_cpus((0..2).map(|i| format!("C{i}").into()));
    assert_eq!(model.clock_labels()[0].as_ref(), "…");
    assert!(model.set_freqs([Some(3900.), Some(4630.)].into_iter()));
    assert_eq!(model.clock_labels()[0].as_ref(), "3.9 GHz");
    assert_eq!(model.clock_labels()[1].as_ref(), "4.6 GHz");
    assert!(!model.set_freqs([Some(3920.), Some(4610.)].into_iter()));
    assert_eq!(model.freqs[0], Some(3920.));
    assert!(model.set_freqs([Some(3960.), Some(4610.)].into_iter()));
    assert_eq!(model.clock_labels()[0].as_ref(), "4.0 GHz");
    assert!(model.set_freqs([Some(3960.), None].into_iter()));
    assert_eq!(model.clock_labels()[1].as_ref(), "…");
    let mut text = String::new();
    model.copy_text(&mut text);
    assert_eq!(text, "C0\t…\t4.0 GHz\nC1\t…\n");

    // crw.104: MHz under 1 GHz, and the BogoMIPS mark on every clock.
    assert!(model.set_freqs([Some(3960.), Some(800.)].into_iter()));
    assert_eq!(model.clock_labels()[1].as_ref(), "800 MHz");
    assert!(model.set_clock_mark(true));
    assert!(!model.set_clock_mark(true));
    assert_eq!(model.clock_labels()[0].as_ref(), "4.0 GHz (?)");
    assert_eq!(model.clock_labels()[1].as_ref(), "800 MHz (?)");
    assert!(model.set_freqs([Some(4270.), Some(800.)].into_iter()));
    assert_eq!(model.clock_labels()[0].as_ref(), "4.3 GHz (?)");
}

/// crw.104: the clock column makes room for the BogoMIPS mark, so a marked cell needs 20 px
/// more before it shows the clock.
#[test]
fn core_clock_column_widens_for_the_mark() {
    let l = core_bars_layout(32, px(438.), px(456.), 1.);
    assert_eq!((l.fit, l.clock_w), (CoreCellFit::Full, px(44.)));
    let marked = core_bars_layout_marked(32, px(438.), px(456.), 1., true);
    assert_eq!((marked.fit, marked.clock_w), (CoreCellFit::Full, px(64.)));
    assert_eq!(core_cell(&marked, 1.).pct_right, marked.cell_w - px(68.));
    // A 200 px cell has the clock unmarked, not marked.
    let l = core_bars_layout(1, px(200.), px(18.), 1.);
    assert_eq!(l.fit, CoreCellFit::Full);
    let l = core_bars_layout_marked(1, px(200.), px(18.), 1., true);
    assert_eq!(l.fit, CoreCellFit::NoGhz);
}

/// crw.104: sorted by load the rows put the busiest CPU first, ties and CPUs without a sample
/// by number, and back by number when the sort is off.
#[test]
fn core_rows_sort_by_load() {
    let mut model = CoreBarsModel::default();
    model.set_cpus((0..5).map(|i| format!("C{i}").into()));
    assert_eq!(model.order(), [0, 1, 2, 3, 4]);
    model.set_values([Some(10.), Some(80.), None, Some(80.), Some(30.)].into_iter());
    assert_eq!(model.order(), [0, 1, 2, 3, 4], "by number by default");
    assert!(model.set_by_load(true));
    assert_eq!(model.order(), [1, 3, 4, 0, 2]);
    // A new load reorders.
    assert!(model.set_values([Some(95.), Some(80.), None, Some(80.), Some(30.)].into_iter()));
    assert_eq!(model.order(), [0, 1, 3, 4, 2]);
    assert!(model.set_by_load(false));
    assert_eq!(model.order(), [0, 1, 2, 3, 4]);
    assert!(!model.set_by_load(false));
}

#[test]
fn core_bars_bucket_change_only() {
    let mut model = CoreBarsModel::default();
    model.set_cpus((0..2).map(|i| format!("C{i}").into()));
    assert!(model.set_values([Some(12.2), Some(50.)].into_iter()));
    assert!(!model.set_values([Some(12.8), Some(50.)].into_iter()));
    assert_eq!(model.values[0], Some(12.8));
    assert!(model.set_values([Some(13.1), Some(50.)].into_iter()));
    // Valid to no sample is a change too.
    assert!(model.set_values([Some(13.1), None].into_iter()));
    assert!(!model.set_values([Some(13.1), None].into_iter()));
}

#[cfg(feature = "test-hooks")]
mod drawn {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        App, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, Window, div, hsla, px, size,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_ui::pages::performance::overview::mem_box::{METER_ROW_LABEL_ID, MeterRow, meter_row};
    use tl_ui::pages::performance::overview::parts::{
        CoreBars, CoreBarsModel, METER_H, Meter, take_meter_quads,
    };

    enum Part {
        Meter(Option<f32>),
        Cores(Rc<RefCell<CoreBarsModel>>),
        Row(MeterRow),
    }

    struct Host(Part);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // Core rows get room for every part, the GHz label included (D-029).
            let width = if matches!(self.0, Part::Cores(_)) {
                200.
            } else {
                101.
            };
            let body = match &self.0 {
                Part::Meter(fraction) => Meter {
                    fraction: *fraction,
                    color: hsla(0.5, 0.5, 0.5, 1.),
                }
                .into_any_element(),
                Part::Row(row) => {
                    let m = *tl_ui::theme::metrics(cx);
                    let theme = tl_ui::theme::current(cx).clone();
                    meter_row(row, &m, &theme)
                }
                Part::Cores(model) => CoreBars {
                    model: Rc::clone(model),
                }
                .into_any_element(),
            };
            div().size_full().child(
                div()
                    .absolute()
                    .top(px(10.))
                    .left(px(20.))
                    .w(px(width))
                    .h(px(200.))
                    .child(body),
            )
        }
    }

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    fn open(
        cx: &mut TestAppContext,
        part: Part,
    ) -> (gpui_kit::Entity<Host>, &mut VisualTestContext) {
        cx.update(setup);
        let (host, vcx) = cx.add_window_view(move |_, _| Host(part));
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(400.), px(300.)));
        (host, vcx)
    }

    #[gpui_kit::test]
    fn meter_row_label_stays_on_one_line(cx: &mut TestAppContext) {
        let row = MeterRow {
            label: "3D / Compute".into(),
            value: "7.0%".into(),
            fraction: Some(0.07),
            color: hsla(0.5, 0.5, 0.5, 1.),
        };
        let (_host, vcx) = open(cx, Part::Row(row));
        vcx.run_until_parked();
        draw(vcx);
        let label = vcx
            .debug_bounds(METER_ROW_LABEL_ID)
            .expect("the row label is drawn");
        // One 12 px line; a wrapped label is two lines tall.
        assert!(
            label.size.height < px(20.),
            "label wrapped: {:?}",
            label.size
        );
    }

    #[gpui_kit::test]
    fn meter_none_draws_track_only(cx: &mut TestAppContext) {
        let (host, vcx) = open(cx, Part::Meter(None));
        vcx.run_until_parked();
        take_meter_quads();
        draw(vcx);
        let quads = take_meter_quads();
        assert_eq!(quads.len(), 1, "track only: {quads:?}");
        assert_eq!(quads[0].origin, gpui_kit::point(px(20.), px(10.)));
        assert_eq!(quads[0].size, size(px(101.), px(METER_H)));

        // With a value the fill follows the track.
        vcx.update(|_, cx| {
            host.update(cx, |h, cx| {
                h.0 = Part::Meter(Some(0.5));
                cx.notify();
            })
        });
        // Any frame the notify brought is drawn first; the count is for one frame.
        vcx.run_until_parked();
        take_meter_quads();
        draw(vcx);
        let quads = take_meter_quads();
        assert_eq!(quads.len(), 2, "track and fill: {quads:?}");
        assert_eq!(quads[1].size.width, px(51.));
    }

    #[gpui_kit::test]
    fn core_bars_reshape_only_changed_labels(cx: &mut TestAppContext) {
        let model = Rc::new(RefCell::new(CoreBarsModel::default()));
        {
            let mut m = model.borrow_mut();
            m.set_cpus((0..4).map(|i| format!("C{i}").into()));
            m.set_values([Some(10.), Some(20.), Some(30.), Some(40.)].into_iter());
        }
        let (host, vcx) = open(cx, Part::Cores(Rc::clone(&model)));
        draw(vcx);
        let (paints, shaped) = {
            let m = model.borrow();
            (m.paint_count.get(), m.shape_count.get())
        };
        assert!(paints >= 1, "painted");
        assert_eq!(
            shaped, 12,
            "4 names, 4 percents and 4 GHz labels shaped once"
        );

        assert!(
            model
                .borrow_mut()
                .set_values([Some(10.), Some(25.), Some(30.), Some(40.)].into_iter())
        );
        vcx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        draw(vcx);
        let (paints, shaped) = {
            let m = model.borrow();
            assert!(m.paint_count.get() > paints, "painted again");
            assert_eq!(
                m.shape_count.get(),
                shaped + 1,
                "only C1's percent re-shaped"
            );
            (m.paint_count.get(), m.shape_count.get())
        };

        assert!(
            model
                .borrow_mut()
                .set_freqs([None, None, Some(3900.), None].into_iter())
        );
        vcx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        draw(vcx);
        let m = model.borrow();
        assert!(m.paint_count.get() > paints, "painted again");
        assert_eq!(m.shape_count.get(), shaped + 1, "only C2's GHz re-shaped");
    }
}

// The CPU and Memory boxes (crw.38).

#[cfg(feature = "test-hooks")]
mod boxes {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
        StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
        size,
    };
    use tl_model::client::EngineClient;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{ComponentHeader, SeriesSet, Snapshot};
    use tl_model::system::Metric;
    use tl_model::time::now_boot_ns;
    use tl_model::validity::Validity;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::chart::labels::LabelKind;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::overview::cpu_box::{CPU_BOX_ID, CpuBox};
    use tl_ui::pages::performance::overview::mem_box::{MEM_BOX_ID, MemBox};
    use tl_ui::pages::performance::overview::parts::{CoreCellFit, core_cell, core_spark_id};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;

    const GIB: u64 = 1 << 30;
    const CPUS: u16 = 4;

    struct Host {
        cpu: Entity<CpuBox>,
        mem: Entity<MemBox>,
        /// The CPU box size.
        cpu_w: f32,
        cpu_h: f32,
        /// The Memory box size.
        mem_w: f32,
        mem_h: f32,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let cached = || StyleRefinement::default().size_full();
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    div()
                        .w(px(self.cpu_w))
                        .h(px(self.cpu_h))
                        .child(self.cpu.clone().cached(cached())),
                )
                .child(
                    div()
                        .w(px(self.mem_w))
                        .h(px(self.mem_h))
                        .child(self.mem.clone().cached(cached())),
                )
        }
    }

    fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("dev"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        }
    }

    /// CPU package 0, logical CPU `id` at `100 + id`, memory 1.
    fn catalog() -> DeviceCatalog {
        catalog_with(CPUS)
    }

    /// [`catalog`] with `cpus` logical CPUs.
    fn catalog_with(cpus: u16) -> DeviceCatalog {
        let mut entries = vec![
            entry(0, DeviceIdentity::CpuPackage),
            entry(1, DeviceIdentity::Memory),
        ];
        entries.extend((0..cpus).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id })));
        DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        }
    }

    fn series(device: u16, metric: MetricId, value: f32, version: u64) -> Arc<SeriesView> {
        const PERIOD: u64 = 100_000_000;
        let newest = now_boot_ns();
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(device),
                metric,
            },
            version,
            period_ns: PERIOD,
            samples: (0..10u64)
                .map(|k| Sample::valid(newest - (9 - k) * PERIOD, value))
                .collect::<Vec<_>>()
                .into(),
        })
    }

    struct Rig<'a> {
        engine: Arc<FakeEngine>,
        host: Entity<Host>,
        vcx: &'a mut VisualTestContext,
        version: u64,
        _driver: Entity<AnimationDriver>,
    }

    fn open(cx: &mut TestAppContext) -> Rig<'_> {
        open_with_memory(cx, DeviceIdx(1))
    }

    fn open_with_memory(cx: &mut TestAppContext, memory: DeviceIdx) -> Rig<'_> {
        open_sized(cx, memory, CPUS, (800., 300.))
    }

    /// `cpus` logical CPUs and a CPU box of `cpu` (width, height).
    fn open_sized(
        cx: &mut TestAppContext,
        memory: DeviceIdx,
        cpus: u16,
        cpu: (f32, f32),
    ) -> Rig<'_> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| s.devices = Arc::new(catalog_with(cpus)));
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            init_strings(cx, "en-US");
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let (host, vcx) = cx.add_window_view(move |_, cx| Host {
            cpu: cx.new(|cx| CpuBox::new(&hub, DeviceIdx(0), cx)),
            mem: cx.new(|cx| MemBox::new(&hub, memory, cx)),
            cpu_w: cpu.0,
            cpu_h: cpu.1,
            mem_w: 800.,
            mem_h: 300.,
        });
        vcx.simulate_scale_factor_change(1.0);
        let driver = vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            driver
        });
        vcx.simulate_resize(size(px(cpu.0.max(800.)), px(cpu.1 + 300.)));
        draw(vcx);
        Rig {
            engine,
            host,
            vcx,
            version: 0,
            _driver: driver,
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..2 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
    }

    impl Rig<'_> {
        fn publish(&mut self, f: impl FnOnce(&mut Snapshot)) {
            self.engine.publish_with(f);
            self.vcx.run_until_parked();
        }

        /// `CPU_TOTAL` of each logical CPU as the series set.
        fn publish_cores(&mut self, values: [f32; CPUS as usize]) {
            self.publish_core_series(&values, &[]);
        }

        /// `CPU_TOTAL` of logical CPU `i` from `pct[i]` and its `CPU_FREQ` from `mhz[i]`, as
        /// the series set.
        fn publish_core_series(&mut self, pct: &[f32], mhz: &[f32]) {
            self.version += 1;
            let version = self.version;
            let total = pct
                .iter()
                .enumerate()
                .map(|(id, &v)| (id, MetricId::CPU_TOTAL, v));
            let freq = mhz
                .iter()
                .enumerate()
                .map(|(id, &v)| (id, MetricId::CPU_FREQ, v));
            let mut views: Vec<_> = total
                .chain(freq)
                .map(|(id, metric, v)| series(100 + id as u16, metric, v, version))
                .collect();
            views.sort_by_key(|v| v.id);
            self.publish(|s| {
                s.series = Arc::new(SeriesSet {
                    header: ComponentHeader {
                        version,
                        ..ComponentHeader::default()
                    },
                    series: views.into(),
                    slow: Arc::from([]),
                });
            });
        }

        /// `ticks` one-second `MEM_USED` samples, ending now.
        fn publish_memory_history(&mut self, ticks: u64, bytes: u64) {
            self.version += 1;
            let version = self.version;
            let newest = now_boot_ns();
            let view = Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(1),
                    metric: MetricId::MEM_USED,
                },
                version,
                period_ns: 1_000_000_000,
                samples: (0..ticks)
                    .map(|k| Sample::valid(newest - (ticks - 1 - k) * 1_000_000_000, bytes as f32))
                    .collect::<Vec<_>>()
                    .into(),
            });
            self.publish(|s| {
                s.series = Arc::new(SeriesSet {
                    header: ComponentHeader {
                        version,
                        ..ComponentHeader::default()
                    },
                    series: Arc::from([view]),
                    slow: Arc::from([]),
                });
            });
        }

        fn system(&mut self, f: impl FnOnce(&mut tl_model::snapshot::SystemSnapshot)) {
            self.publish(|s| {
                let sys = Arc::make_mut(&mut s.system);
                sys.header.version += 1;
                f(sys);
            });
        }

        fn cpu(&self) -> Entity<CpuBox> {
            self.vcx.read_entity(&self.host, |h, _| h.cpu.clone())
        }

        fn mem(&self) -> Entity<MemBox> {
            self.vcx.read_entity(&self.host, |h, _| h.mem.clone())
        }

        /// (render, notify) of the CPU box, then of the Memory box.
        fn counts(&mut self) -> [u32; 4] {
            let (cpu, mem) = (self.cpu(), self.mem());
            self.vcx.update(|_, cx| {
                let (c, m) = (cpu.read(cx), mem.read(cx));
                [
                    c.render_count.get(),
                    c.notify_count.get(),
                    m.render_count.get(),
                    m.notify_count.get(),
                ]
            })
        }
    }

    #[gpui_kit::test]
    fn cpu_box_bars_follow_logical_series(cx: &mut TestAppContext) {
        let mut r = open(cx);
        let before = r.counts();
        r.publish_cores([10., 20., 30., 40.]);
        let cpu = r.cpu();
        let values = r.vcx.update(|_, cx| cpu.read(cx).bars().values.clone());
        assert_eq!(values, [Some(10.), Some(20.), Some(30.), Some(40.)]);
        assert_eq!(r.counts()[1], before[1] + 1, "one notify");

        // The same buckets again: no notify.
        r.publish_cores([10.4, 20.2, 30.9, 40.]);
        assert_eq!(r.counts()[1], before[1] + 1, "same buckets, no notify");

        let mut text = String::new();
        r.vcx.update(|_, cx| cpu.read(cx).copy_text(&mut text));
        assert!(
            text.contains("C0\t10%\n") && text.contains("C3\t40%\n"),
            "{text}"
        );
    }

    #[gpui_kit::test]
    fn cpu_box_freq_temp_optional(cx: &mut TestAppContext) {
        let mut r = open(cx);
        r.system(|s| {
            s.cpu.util_pct = Metric::valid(12.5);
            s.cpu.freq_mhz = Metric::valid(4631);
            s.cpu.temp_c = Metric::with(Validity::Unsupported);
        });
        let cpu = r.cpu();
        let text = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let c = cpu.read(cx);
                let (_, value, line) = c.text();
                (value.to_string(), line.to_string())
            })
        };
        assert_eq!(text(&mut r), ("12.5%".into(), "4631 MHz".into()));
        r.system(|s| s.cpu.temp_c = Metric::valid(52.));
        assert_eq!(text(&mut r).1, "4631 MHz · 52 °C");

        // A failed source shows the glyph, not a number.
        r.system(|s| s.cpu.util_pct = Metric::with(Validity::Failed));
        assert_eq!(text(&mut r).0, "!");

        // An unrelated system change formats nothing and does not notify.
        let before = r.counts();
        r.system(|s| s.memory.dirty += 1);
        assert_eq!(r.counts()[1], before[1]);
    }

    #[gpui_kit::test]
    fn cpu_mem_boxes_idle_snapshot_nothing(cx: &mut TestAppContext) {
        let mut r = open(cx);
        r.publish_cores([10., 20., 30., 40.]);
        r.system(|s| {
            s.memory.validity = Validity::Valid;
            s.memory.total = 16 * GIB;
            s.memory.used = 4 * GIB;
        });
        draw(r.vcx);
        let before = r.counts();
        r.publish(|s| Arc::make_mut(&mut s.processes).header.version += 1);
        draw(r.vcx);
        assert_eq!(r.counts(), before, "processes only: nothing renders");

        // Pointer movement over both boxes notifies and renders nothing.
        for (x, y) in [(100., 100.), (500., 150.), (300., 400.), (700., 550.)] {
            r.vcx
                .simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
        }
        draw(r.vcx);
        assert_eq!(r.counts(), before, "pointer moves: nothing renders");
    }

    #[gpui_kit::test]
    fn mem_box_meters_and_no_swap(cx: &mut TestAppContext) {
        let mut r = open(cx);
        r.system(|s| {
            s.memory.validity = Validity::Valid;
            s.memory.total = 64 * GIB;
            s.memory.used = 32 * GIB;
            s.memory.available = 32 * GIB;
            s.memory.cached = 16 * GIB;
            s.memory.swap_total = 0;
            s.memory.swap_free = 0;
            s.memory.commit_limit = 40 * GIB;
            s.memory.committed = 98 * GIB;
        });
        let mem = r.mem();
        let (rows, pcts, value) = r.vcx.update(|_, cx| {
            let m = mem.read(cx);
            (
                m.rows().clone(),
                m.percents().clone(),
                m.text().1.to_string(),
            )
        });
        assert_eq!(value, "50.0%");
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_ref()).collect();
        assert_eq!(labels, ["In use", "Cached", "Available", "Swap", "Commit"]);
        assert_eq!(rows[0].fraction, Some(0.5));
        assert_eq!(rows[0].value.as_ref(), "32.00 GiB");
        assert_eq!(pcts[0].as_ref(), "50.0%");
        assert_eq!(rows[1].fraction, Some(0.25));
        assert_eq!(rows[2].fraction, Some(0.5));
        assert_eq!(rows[3].value.as_ref(), "No swap");
        assert_eq!(rows[3].fraction, None);
        assert_eq!(pcts[3].as_ref(), "");
        // Commit passes 100%: the meter stops at full, the percent shows the real ratio.
        assert_eq!(rows[4].fraction, Some(1.0));
        assert_eq!(pcts[4].as_ref(), "245.0%");

        r.system(|s| {
            s.memory.swap_total = 8 * GIB;
            s.memory.swap_free = 6 * GIB;
        });
        let swap = r.vcx.update(|_, cx| mem.read(cx).rows()[3].clone());
        assert_eq!(swap.fraction, Some(0.25));
        assert_eq!(swap.value.as_ref(), "2.00 GiB");
    }

    #[gpui_kit::test]
    fn mem_box_rows_keep_all_parts_and_fill_body(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::mem_box::{
            MEM_BODY_ID, meter_id, row_id, spark_id, value_id,
        };
        let mut r = open(cx);
        r.system(|s| {
            s.memory.validity = Validity::Valid;
            s.memory.total = 64 * GIB;
            s.memory.used = 32 * GIB;
            s.memory.available = 32 * GIB;
            s.memory.cached = 16 * GIB;
            s.memory.swap_total = 8 * GIB;
            s.memory.swap_free = 6 * GIB;
            s.memory.commit_limit = 40 * GIB;
            s.memory.committed = 20 * GIB;
        });
        // Room for the CPU box (300 px) and the tallest Memory box.
        r.vcx.simulate_resize(size(px(800.), px(900.)));
        let host = r.host.clone();
        let leak = |id: String| -> &'static str { Box::leak(id.into_boxed_str()) };
        // (box width, box height, sparkline width): one-line rows at 800 px, the 1200 x 900
        // default (384 x 516, a 358 px body) and a 720 px page (224 x 288, a 198 px body, 128 px text part and the 8 px splitter).
        // The 720 px page box is 280 px plus the 8 px above the title (UI-PRF-059), so its
        // body stays 202 px high and the five two-line rows (196 px) fit.
        for (w, h, spark_w) in [(800., 300., 96.), (384., 516., 96.), (224., 288., 58.)] {
            r.vcx.update(|_, cx| {
                host.update(cx, |hst, cx| {
                    hst.mem_w = w;
                    hst.mem_h = h;
                    cx.notify();
                })
            });
            draw(r.vcx);
            let frame = r.vcx.debug_bounds(MEM_BOX_ID).expect("Memory box drawn");
            let body = r.vcx.debug_bounds(MEM_BODY_ID).expect("body drawn");
            for i in 0..5 {
                let value = r.vcx.debug_bounds(leak(value_id(i)));
                let value = value.unwrap_or_else(|| panic!("{w} px: row {i} draws its value"));
                assert!(
                    f32::from(value.size.width) > 0. && value.right() <= frame.right(),
                    "{w} px: value {i} is shown inside the box, got {value:?}"
                );
                let meter = r.vcx.debug_bounds(leak(meter_id(i)));
                let meter = meter.unwrap_or_else(|| panic!("{w} px: row {i} draws its meter"));
                assert!(
                    f32::from(meter.size.width) >= 32.,
                    "{w} px: meter {i} keeps its width, got {:?}",
                    meter.size.width
                );
                let spark = r.vcx.debug_bounds(leak(spark_id(i)));
                let spark = spark.unwrap_or_else(|| panic!("{w} px: row {i} draws its sparkline"));
                assert!(
                    spark.right() <= frame.right(),
                    "{w} px: sparkline {i} stays inside the box"
                );
                assert!(
                    (f32::from(spark.size.width) - spark_w).abs() < 1.,
                    "{w} px: sparkline {i} is {spark_w} px, got {:?}",
                    spark.size.width
                );
            }
            // The rows fill the body: no empty space above the first or below the last.
            let first = r.vcx.debug_bounds(leak(row_id(0))).expect("row 0 drawn");
            let last = r.vcx.debug_bounds(leak(row_id(4))).expect("row 4 drawn");
            assert!(
                (f32::from(first.top() - body.top())).abs() < 1.,
                "{w} x {h}: first row at the body top, row {first:?}, body {body:?}"
            );
            assert!(
                (f32::from(body.bottom() - last.bottom())).abs() < 1.,
                "{w} x {h}: last row at the body bottom, row {last:?}, body {body:?}"
            );
        }
    }

    #[gpui_kit::test]
    fn mem_box_draws_used_history(cx: &mut TestAppContext) {
        // The Overview can be constructed before the memory source adds its device. The first
        // device update must retarget the chart from its missing sentinel to the memory device.
        let mut r = open_with_memory(cx, DeviceIdx(u16::MAX));
        r.system(|s| {
            s.memory.validity = Validity::Valid;
            s.memory.total = 64 * GIB;
            s.memory.used = 32 * GIB;
        });
        r.publish(|s| {
            let mut devices = catalog();
            devices.version = 2;
            s.devices = Arc::new(devices);
        });
        r.publish_memory_history(8, 32 * GIB);
        draw(r.vcx);

        let mem = r.mem();
        r.vcx.update(|_, cx| {
            let chart = mem.read(cx).sparks()[0].read(cx);
            let used = chart.resolved()[0].as_ref().expect("MEM_USED resolved");
            assert_eq!(used.samples.len(), 8, "fixture history reached the chart");
            assert_ne!(
                chart.cached_fill_stroke_counts(),
                (0, 0),
                "used history built fill and stroke paths"
            );
        });
    }

    #[gpui_kit::test]
    fn mem_box_ignores_cpu_tick(cx: &mut TestAppContext) {
        let mut r = open(cx);
        r.system(|s| {
            s.memory.validity = Validity::Valid;
            s.memory.total = 16 * GIB;
            s.memory.used = 4 * GIB;
        });
        draw(r.vcx);
        let mem = r.mem();
        let chart = r.vcx.update(|_, cx| mem.read(cx).sparks()[0].clone());
        let builds = |r: &mut Rig| r.vcx.update(|_, cx| chart.read(cx).build_count.get());
        let (before, built) = (r.counts(), builds(&mut r));
        r.publish_cores([10., 20., 30., 40.]);
        draw(r.vcx);
        let after = r.counts();
        assert!(after[1] > before[1], "the CPU box took the tick");
        assert_eq!(after[2..], before[2..], "the Memory box did not");
        assert_eq!(builds(&mut r), built, "nor did its chart");
    }

    #[gpui_kit::test]
    fn scale_label_shapes_once(cx: &mut TestAppContext) {
        // The CPU chart keeps its scale label; the Memory sparklines draw none (crw.54).
        let r = open(cx);
        draw(r.vcx);
        let (cpu, mem) = (r.cpu(), r.mem());
        let cpu_chart = r.vcx.update(|_, cx| cpu.read(cx).total_chart().clone());
        let sparks = r.vcx.update(|_, cx| mem.read(cx).sparks().clone());
        r.vcx.update(|_, cx| {
            let c = cpu_chart.read(cx).label_cache();
            assert!(
                c.painted(LabelKind::AxisMax).is_some(),
                "scale label painted"
            );
            assert_eq!(c.text(LabelKind::AxisMax), Some("100.0%"));
            for spark in &sparks {
                let c = spark.read(cx).label_cache();
                assert!(c.painted(LabelKind::AxisMax).is_none(), "no scale label");
            }
        });
    }

    #[gpui_kit::test]
    fn cpu_box_stacks_below_560(cx: &mut TestAppContext) {
        let mut r = open(cx);
        let cpu = r.cpu();
        let wide = |r: &mut Rig| r.vcx.update(|_, cx| cpu.read(cx).side_by_side());
        assert!(wide(&mut r), "800 px: bars beside the chart");
        let host = r.host.clone();
        for (w, side) in [(500., false), (540., false), (600., true)] {
            r.vcx.update(|_, cx| {
                host.update(cx, |h, cx| {
                    h.cpu_w = w;
                    cx.notify();
                })
            });
            draw(r.vcx);
            assert_eq!(wide(&mut r), side, "{w} px");
        }
    }

    /// crw.53: the clock column follows each core's `CPU_FREQ` series, and the box notifies only
    /// when a shown clock changes: 0.1 GHz steps, whole MHz under 1 GHz (crw.104).
    #[gpui_kit::test]
    fn cpu_box_ghz_follows_freq_series(cx: &mut TestAppContext) {
        let mut r = open(cx);
        let pct = [10., 20., 30., 40.];
        r.publish_core_series(&pct, &[3900., 4000., 4630., 800.]);
        let cpu = r.cpu();
        let ghz = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let c = cpu.read(cx);
                let bars = c.bars();
                bars.clock_labels()
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(ghz(&mut r), ["3.9 GHz", "4.0 GHz", "4.6 GHz", "800 MHz"]);
        let before = r.counts();

        // Inside every shown clock: no notify.
        r.publish_core_series(&pct, &[3920., 3980., 4610., 800.3]);
        assert_eq!(r.counts()[1], before[1], "same buckets, no notify");

        // One clock crosses a bucket: one notify.
        r.publish_core_series(&pct, &[3920., 3980., 4610., 1200.]);
        assert_eq!(r.counts()[1], before[1] + 1, "one notify");
        assert_eq!(ghz(&mut r)[3], "1.2 GHz");

        let mut text = String::new();
        r.vcx.update(|_, cx| cpu.read(cx).copy_text(&mut text));
        assert!(text.contains("C3\t40%\t1.2 GHz\n"), "{text}");

        // No clock samples: the warming glyph.
        r.publish_core_series(&pct, &[]);
        assert_eq!(ghz(&mut r), ["…"; 4]);
    }

    /// crw.53 AC: on a 32-thread fixture every core row shows in the CPU box at 1200 × 900
    /// (776 × 516), in 2 columns of 18 px rows beside the 40% chart, each with its sparkline.
    /// The column count follows the height (D-029), so a wider box keeps 2 columns.
    #[gpui_kit::test]
    fn cpu_box_fits_32_threads_at_1200x900(cx: &mut TestAppContext) {
        const N: u16 = 32;
        let mut r = open_sized(cx, DeviceIdx(1), N, (776., 516.));
        let pct: Vec<f32> = (0..N).map(|i| f32::from(i) * 3.).collect();
        let mhz: Vec<f32> = (0..N).map(|i| 3000. + f32::from(i) * 25.).collect();
        r.publish_core_series(&pct, &mhz);
        draw(r.vcx);
        let cpu = r.cpu();
        let (layout, sparks) = r.vcx.update(|_, cx| {
            let c = cpu.read(cx);
            (c.core_layout(), c.core_sparks().to_vec())
        });
        assert!(r.vcx.update(|_, cx| cpu.read(cx).side_by_side()));
        assert_eq!((layout.cols, layout.rows), (2, 16));
        assert_eq!(layout.row_h, px(18.));
        assert!(!layout.scrolls);
        assert_eq!(layout.fit, CoreCellFit::Full);
        assert_eq!(sparks.len(), usize::from(N));

        let frame = r.vcx.debug_bounds(CPU_BOX_ID).expect("CPU box drawn");
        for i in 0..usize::from(N) {
            let id: &'static str = Box::leak(core_spark_id(i).into_boxed_str());
            let b = r
                .vcx
                .debug_bounds(id)
                .unwrap_or_else(|| panic!("core {i} sparkline drawn"));
            assert_eq!(b.size, size(px(48.), px(14.)), "core {i} sparkline size");
            assert!(
                b.left() >= frame.left()
                    && b.right() <= frame.right()
                    && b.bottom() <= frame.bottom(),
                "core {i} inside the box: {b:?} in {frame:?}"
            );
            let (col, row) = (i / 16, i % 16);
            if col == 1 {
                let left = r
                    .vcx
                    .debug_bounds(Box::leak(core_spark_id(i - 16).into_boxed_str()));
                let left = left.expect("left neighbour");
                assert_eq!(b.top(), left.top(), "core {i} beside core {}", i - 16);
                assert!(b.left() > left.right(), "core {i} in the right column");
            } else if row > 0 {
                let above = r
                    .vcx
                    .debug_bounds(Box::leak(core_spark_id(i - 1).into_boxed_str()));
                let above = above.expect("row above");
                assert_eq!(b.top() - above.top(), px(18.), "core {i} one row lower");
            }
        }
        // The sparklines are bound to their core's series.
        r.vcx.update(|_, cx| {
            let chart = sparks[31].read(cx);
            assert!(chart.resolved()[0].is_some(), "C31 CPU_TOTAL resolved");
        });

        // A wider box keeps 2 columns: the rows still need them at its height.
        r.vcx.simulate_resize(size(px(1200.), px(816.)));
        let host = r.host.clone();
        r.vcx.update(|_, cx| {
            host.update(cx, |h, cx| {
                h.cpu_w = 1000.;
                cx.notify();
            })
        });
        draw(r.vcx);
        let layout = r.vcx.update(|_, cx| cpu.read(cx).core_layout());
        assert_eq!((layout.cols, layout.rows), (2, 16));
    }

    /// crw.74: stacked below 560 px, the last core meter ends inside the box body, above the
    /// bottom padding (tasklens-o55.54: the box has no "Now" label under it).
    #[gpui_kit::test]
    fn overview_stacked_core_meters_keep_gap_above_now(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::parts::{body_id, take_meter_quads};
        const N: usize = 8;
        let mut r = open_sized(cx, DeviceIdx(1), N as u16, (400., 516.));
        take_meter_quads();
        let pct: Vec<f32> = (0..N).map(|i| 10. + i as f32 * 5.).collect();
        r.publish_core_series(&pct, &[]);
        draw(r.vcx);
        let frame = r.vcx.debug_bounds(CPU_BOX_ID).expect("CPU box drawn");
        let id: &'static str = Box::leak(body_id(CPU_BOX_ID).into_boxed_str());
        let body = r.vcx.debug_bounds(id).expect("body");
        let last = take_meter_quads()
            .into_iter()
            .filter(|q| frame.contains(&q.origin))
            .map(|q| q.bottom())
            .fold(px(0.), |a, b| a.max(b));
        assert!(last > px(0.), "core meters drawn");
        assert!(
            last <= body.bottom(),
            "last meter {last:?} inside the body {body:?}"
        );
        assert!(
            frame.bottom() - last >= px(4.),
            "gap above the bottom border: {:?}",
            frame.bottom() - last
        );
    }

    /// D-029: every core row shows its name, sparkline, bar and percent in that order. Rows are
    /// 18 px apart (UI-PRF-059); 32 threads that do not fit one column flow top to bottom into 2
    /// (C0 to C15, then C16 to C31). A box too short for 4 columns keeps 18 px rows, with the
    /// name and percent, and scrolls.
    #[gpui_kit::test]
    fn overview_core_rows_name_spark_columns(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::parts::{
            CORE_ROWS_SCROLL_ID, take_core_labels, take_meter_quads,
        };
        const N: usize = 32;
        // The last painted labels of each core.
        let last_labels = || {
            let mut by_core = std::collections::BTreeMap::new();
            for l in take_core_labels() {
                by_core.insert(l.core, l);
            }
            by_core.into_values().collect::<Vec<_>>()
        };
        let mut r = open_sized(cx, DeviceIdx(1), N as u16, (776., 516.));
        take_core_labels();
        take_meter_quads();
        let pct: Vec<f32> = (0..N).map(|i| 1. + i as f32 * 3.).collect();
        r.publish_core_series(&pct, &[]);
        draw(r.vcx);
        let cpu = r.cpu();
        let layout = r.vcx.update(|_, cx| cpu.read(cx).core_layout());
        assert_eq!((layout.cols, layout.rows, layout.row_h), (2, 16, px(18.)));
        let parts = core_cell(&layout, 1.);
        let frame = r.vcx.debug_bounds(CPU_BOX_ID).expect("CPU box drawn");

        // One name and percent per core, the columns filled top to bottom.
        let labels = last_labels();
        assert_eq!(labels.len(), N, "{labels:?}");
        let first = labels[0];
        for l in &labels {
            let (col, row) = (l.core / 16, l.core % 16);
            assert!(l.name.x > frame.left() && l.pct_right <= frame.right());
            assert!(
                l.name.y + px(18.) <= frame.bottom(),
                "C{} inside the box",
                l.core
            );
            let x = first.name.x + (layout.cell_w + px(8.)) * col as f32;
            assert_eq!(l.name.x, x, "C{} in column {col}", l.core);
            assert_eq!(l.name.y, first.name.y + px(18.) * row as f32, "C{}", l.core);
            assert_eq!(l.pct_right, l.name.x + parts.pct_right);
        }

        // Name, sparkline, bar, percent from left to right in every row.
        let quads = take_meter_quads();
        for l in &labels {
            let id: &'static str = Box::leak(core_spark_id(l.core).into_boxed_str());
            let spark = r.vcx.debug_bounds(id).expect("sparkline drawn");
            assert!(spark.top() >= l.name.y && spark.bottom() <= l.name.y + px(18.));
            assert_eq!(spark.left(), l.name.x + parts.spark_x, "C{}", l.core);
            let track = quads
                .iter()
                .find(|q| q.left() == l.name.x + parts.meter_x && q.top() > l.name.y)
                .filter(|q| q.bottom() < l.name.y + px(18.))
                .unwrap_or_else(|| panic!("C{} bar", l.core));
            assert!(spark.right() < track.left(), "C{} spark before bar", l.core);
            assert!(
                track.right() < l.pct_right,
                "C{} bar before percent",
                l.core
            );
        }

        // Too short for 4 columns of 18 px rows: the rows scroll, name and percent stay.
        // The update draws the window itself, so the old labels go first.
        take_core_labels();
        r.vcx.simulate_resize(size(px(800.), px(440.)));
        let host = r.host.clone();
        r.vcx.update(|_, cx| {
            host.update(cx, |h, cx| {
                h.cpu_h = 140.;
                cx.notify();
            })
        });
        draw(r.vcx);
        let layout = r.vcx.update(|_, cx| cpu.read(cx).core_layout());
        assert!(layout.scrolls, "{layout:?}");
        assert_eq!((layout.cols, layout.rows, layout.row_h), (4, 8, px(18.)));
        let view = r
            .vcx
            .debug_bounds(CORE_ROWS_SCROLL_ID)
            .expect("scroll view");
        assert!(view.size.height < layout.content_h(), "{view:?}");
        let labels = last_labels();
        assert_eq!(labels.len(), N);
        assert_eq!(labels[1].name.y - labels[0].name.y, px(18.));
        assert_eq!(
            labels[8].name.y, labels[0].name.y,
            "C8 tops the second column"
        );
    }

    /// `CPU_TOTAL` of logical CPU 0 as `values` at 10 Hz ending now.
    fn publish_train(r: &mut Rig, values: &[f32]) {
        const PERIOD: u64 = 100_000_000;
        let newest = now_boot_ns();
        let n = values.len() as u64;
        let samples: Vec<Sample> = values
            .iter()
            .enumerate()
            .map(|(k, &v)| Sample::valid(newest - (n - 1 - k as u64) * PERIOD, v))
            .collect();
        r.version += 1;
        let version = r.version;
        r.publish(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: vec![Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(100),
                        metric: MetricId::CPU_TOTAL,
                    },
                    version,
                    period_ns: PERIOD,
                    samples: samples.into(),
                })]
                .into(),
                slow: Arc::from([]),
            });
        });
    }

    fn core0(r: &mut Rig) -> Option<f32> {
        let cpu = r.cpu();
        r.vcx.update(|_, cx| cpu.read(cx).bars().values[0])
    }

    /// 0, 100, 0, 100 at 10 Hz for 3 s.
    fn flicker() -> Vec<f32> {
        (0..31)
            .map(|i| if i % 2 == 0 { 0. } else { 100. })
            .collect()
    }

    #[gpui_kit::test]
    fn overview_core_rows_use_smoothed_value(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_train(&mut r, &flicker());
        let v = core0(&mut r).expect("a smoothed value");
        assert!((40. ..=60.).contains(&v), "smoothed {v}");
        // The total chart and the title value stay raw.
        let cpu = r.cpu();
        let spec_total = r
            .vcx
            .update(|_, cx| cpu.read(cx).total_chart().read(cx).spec.clone());
        assert!(spec_total.series.iter().all(|s| s.mean_ns == 0));
    }

    #[gpui_kit::test]
    fn overview_core_smoothing_off_shows_raw(cx: &mut TestAppContext) {
        let mut r = open(cx);
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.performance.overview_core_smoothing =
                    tl_model::settings::CoreSmoothing::Off;
            });
        });
        publish_train(&mut r, &flicker());
        assert_eq!(core0(&mut r), Some(0.), "the newest raw sample");
    }

    #[gpui_kit::test]
    fn overview_core_invalid_sample_resets_average(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_train(&mut r, &[100.; 10]);
        assert_eq!(core0(&mut r), Some(100.));
        let newest = now_boot_ns() + 10_000_000_000;
        r.version += 1;
        let version = r.version;
        r.publish(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: vec![Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(100),
                        metric: MetricId::CPU_TOTAL,
                    },
                    version,
                    period_ns: 100_000_000,
                    samples: vec![
                        Sample::valid(newest, 100.),
                        Sample::gap(newest + 100_000_000),
                        Sample::valid(newest + 200_000_000, 20.),
                    ]
                    .into(),
                })]
                .into(),
                slow: Arc::from([]),
            });
        });
        assert_eq!(
            core0(&mut r),
            Some(20.),
            "restarts from the next valid sample"
        );
    }

    /// `CPU_TOTAL` of every logical CPU at 10 Hz for the last `secs` seconds.
    fn publish_core_history(r: &mut Rig, secs: u64) {
        const PERIOD: u64 = 100_000_000;
        let newest = now_boot_ns();
        let n = secs * 10;
        r.version += 1;
        let version = r.version;
        let views: Vec<_> = (0..CPUS)
            .map(|id| {
                Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(100 + id),
                        metric: MetricId::CPU_TOTAL,
                    },
                    version,
                    period_ns: PERIOD,
                    samples: (0..n)
                        .map(|k| Sample::valid(newest - (n - 1 - k) * PERIOD, (k % 100) as f32))
                        .collect::<Vec<_>>()
                        .into(),
                })
            })
            .collect();
        r.publish(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
        });
    }

    /// Each core sparkline's stroke width over its drawn width. The stroke x runs from minus
    /// the plot width to 0 at the right edge, so the width is `right - left`.
    fn spark_spans(r: &mut Rig) -> Vec<f32> {
        draw(r.vcx);
        let cpu = r.cpu();
        let strokes: Vec<f32> = r.vcx.update(|_, cx| {
            cpu.read(cx)
                .core_sparks()
                .iter()
                .map(|s| {
                    let s = s.read(cx);
                    let left = s.stroke_left_x().expect("a stroke");
                    let right = s.stroke_right_x().expect("a stroke");
                    right - left
                })
                .collect()
        });
        strokes
            .into_iter()
            .enumerate()
            .map(|(i, stroke)| {
                let id: &'static str = Box::leak(core_spark_id(i).into_boxed_str());
                let b = r.vcx.debug_bounds(id).expect("sparkline drawn");
                let w = f32::from(b.size.width);
                assert!(w > 20., "core {i} sparkline is {w} px wide");
                stroke / w
            })
            .collect()
    }

    /// crw.83: after 60 s of 10 Hz samples every core sparkline plots the whole 60 s range, and
    /// device catalog changes (a new version, a device coming) neither rebuild nor shorten it.
    #[gpui_kit::test]
    fn overview_core_sparkline_spans_full_range(cx: &mut TestAppContext) {
        let mut r = open(cx);
        let cpu = r.cpu();
        let sparks = |r: &mut Rig| r.vcx.update(|_, cx| cpu.read(cx).core_sparks().to_vec());
        let before = sparks(&mut r);
        assert_eq!(before.len(), usize::from(CPUS));

        let check = |spans: Vec<f32>, when: &str| {
            assert_eq!(spans.len(), usize::from(CPUS), "{when}");
            for (i, share) in spans.into_iter().enumerate() {
                assert!(
                    share >= 0.95,
                    "{when}: core {i} plots {share} of its width, not the whole range"
                );
            }
        };

        // Only 6 s of history covers only the right tenth.
        publish_core_history(&mut r, 6);
        let short = spark_spans(&mut r);
        assert!(short.iter().all(|&share| share < 0.2), "{short:?}");

        publish_core_history(&mut r, 61);
        check(spark_spans(&mut r), "steady catalog");

        // A new catalog version with the same CPUs.
        r.publish(|s| {
            let mut cat = catalog();
            cat.version = 2;
            s.devices = Arc::new(cat);
        });
        publish_core_history(&mut r, 61);
        check(spark_spans(&mut r), "catalog version 2");

        // A device comes (a disk at 50): the CPUs keep their charts.
        r.publish(|s| {
            let cat = catalog();
            let mut entries = cat.entries.to_vec();
            entries.push(entry(
                50,
                DeviceIdentity::Disk {
                    major: 259,
                    minor: 0,
                    name: Arc::from("nvme0n1"),
                    wwn_or_serial: None,
                },
            ));
            s.devices = Arc::new(DeviceCatalog {
                version: 3,
                entries: entries.into(),
            });
        });
        publish_core_history(&mut r, 61);
        check(spark_spans(&mut r), "catalog version 3");

        let after = sparks(&mut r);
        assert!(
            before
                .iter()
                .zip(&after)
                .all(|(a, b)| a.entity_id() == b.entity_id()),
            "the sparklines are the same entities"
        );
    }

    #[gpui_kit::test]
    fn overview_core_sparkline_plots_second_means(cx: &mut TestAppContext) {
        let r = open(cx);
        let cpu = r.cpu();
        let means = r.vcx.update(|_, cx| {
            let c = cpu.read(cx);
            (
                c.core_sparks()[0].read(cx).spec.series[0].mean_ns,
                c.total_chart().read(cx).spec.series[0].mean_ns,
            )
        });
        assert_eq!(means, (1_000_000_000, 0));
    }
}

// The GPU, Disk and Network boxes (crw.39).

#[cfg(feature = "test-hooks")]
mod bottom {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
        StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{
        ComponentHeader, DeviceDetailsSnapshot, DiskDetails, GraphicsAdapter, InvField, SeriesSet,
        Snapshot, SystemSnapshot,
    };
    use tl_model::system::{DiskTotals, GpuEngine, GpuTotals, LinkState, Metric, NetTotals};
    use tl_model::table::Total;
    use tl_model::time::now_boot_ns;
    use tl_model::units;
    use tl_model::validity::Validity;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::DeviceKey;
    use tl_ui::pages::performance::overview::disk_box::{
        DISK_ROW_ID_PREFIX, DiskBox, DiskRow, io_row_id,
    };
    use tl_ui::pages::performance::overview::gpu_box::{
        GPU_BOX_ID, GpuBox, SIDE_ROWS_MIN_W, chart_id, engine_cell_id, engine_row_id, grid_id,
        rows_id, vram_id,
    };
    use tl_ui::pages::performance::overview::layout::Arrange;
    use tl_ui::pages::performance::overview::net_box::NetBox;
    use tl_ui::pages::performance::overview::parts::{BOX_CHROME, GridFit, body_id, fill_grid};
    use tl_ui::settings_global::{SettingKey, SettingValue, TlSettings};
    use tl_ui::testing::fake_engine::FakeEngine;

    const GIB: u64 = 1 << 30;
    const MIB: f64 = 1024. * 1024.;
    const DISK_AGG: u16 = 50;
    const NET_AGG: u16 = 51;

    type Devices = Vec<(DeviceKey, DeviceIdx, SharedString)>;

    struct Host {
        gpu: Entity<GpuBox>,
        disk: Entity<DiskBox>,
        net: Entity<NetBox>,
        /// GPU and Disk box heights, and the GPU box width.
        gpu_h: f32,
        disk_h: f32,
        gpu_w: f32,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let cached = || StyleRefinement::default().size_full();
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    div()
                        .w(px(self.gpu_w))
                        .h(px(self.gpu_h))
                        .child(self.gpu.clone().cached(cached())),
                )
                .child(
                    div()
                        .w(px(500.))
                        .h(px(self.disk_h))
                        .child(self.disk.clone().cached(cached())),
                )
                .child(
                    div()
                        .w(px(500.))
                        .h(px(200.))
                        .child(self.net.clone().cached(cached())),
                )
        }
    }

    fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("dev"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind,
            hot_c: None,
        }
    }

    fn gpu_id(n: u8) -> DeviceIdentity {
        DeviceIdentity::Gpu {
            uuid: [n; 16],
            pci_bus_id: Arc::from(format!("0000:0{n}:00.0")),
        }
    }

    fn disk_id(minor: u32, name: &str) -> DeviceIdentity {
        DeviceIdentity::Disk {
            major: 8,
            minor,
            name: Arc::from(name),
            wwn_or_serial: None,
        }
    }

    fn net_id(ifindex: u32, name: &str) -> DeviceIdentity {
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex,
            name: Arc::from(name),
        }
    }

    /// GPUs at 10..13, disks sda, sdb and sdc at 20, 21 and 22, aggregates, eth0 (wired) at 30,
    /// docker0 (virtual) at 31, lo at 32 and wlan0 (wireless) at 33.
    fn catalog() -> DeviceCatalog {
        let mut entries: Vec<_> = (0..3)
            .map(|n| entry(10 + n, gpu_id(n as u8), None))
            .collect();
        entries.push(entry(20, disk_id(0, "sda"), None));
        entries.push(entry(21, disk_id(16, "sdb"), None));
        entries.push(entry(22, disk_id(32, "sdc"), None));
        entries.push(entry(
            DISK_AGG,
            DeviceIdentity::Aggregate(AggregateKind::Disks),
            None,
        ));
        entries.push(entry(
            NET_AGG,
            DeviceIdentity::Aggregate(AggregateKind::Nets),
            None,
        ));
        entries.push(entry(30, net_id(2, "eth0"), Some(NetKind::Wired)));
        entries.push(entry(31, net_id(3, "docker0"), Some(NetKind::Virtual)));
        entries.push(entry(32, net_id(1, "lo"), Some(NetKind::Other)));
        entries.push(entry(33, net_id(4, "wlan0"), Some(NetKind::Wireless)));
        DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        }
    }

    fn devices(kind: &str, idx: &[u16]) -> Devices {
        idx.iter()
            .enumerate()
            .map(|(n, i)| {
                (
                    DeviceKey(Arc::from(format!("{kind}{n}"))),
                    DeviceIdx(*i),
                    SharedString::from(format!("{kind} {n}")),
                )
            })
            .collect()
    }

    fn gpu_totals(device: u16) -> GpuTotals {
        GpuTotals {
            device: DeviceIdx(device),
            util_pct: Metric::valid(25.),
            mem_util_pct: Metric::default(),
            mem_used: Metric::valid(4 * GIB),
            mem_total: Metric::valid(16 * GIB),
            shared_used: Metric::default(),
            shared_total: Metric::default(),
            mem_reserved: Metric::default(),
            enc_pct: Metric::default(),
            dec_pct: Metric::default(),
            enc_sessions: Metric::default(),
            enc_dec_shared: false,
            temp_c: Metric::with(Validity::Unsupported),
            temp_edge_c: Metric::default(),
            temp_junction_c: Metric::default(),
            temp_mem_c: Metric::default(),
            power_w: Metric::valid(120.),
            power_limit_w: Metric::default(),
            clock_gfx_mhz: Metric::default(),
            clock_mem_mhz: Metric::default(),
            fan_pct: Metric::default(),
            pcie_rx_bps: Metric::default(),
            pcie_tx_bps: Metric::default(),
            engines: Default::default(),
            throttling: Default::default(),
            fdinfo_partial: false,
            energy_since_boot: Metric::default(),
            clock_video_mhz: Metric::default(),
            pstate: Metric::default(),
            clock_event_reasons: Metric::default(),
            fan_rpm: Metric::default(),
            pcie_gen: Metric::default(),
            pcie_width: Metric::default(),
            info: Arc::default(),
        }
    }

    fn disk_totals(device: u16, read_bps: f64) -> DiskTotals {
        DiskTotals {
            device: DeviceIdx(device),
            active_pct: Metric::valid(40.),
            read_bps: Metric::valid(read_bps),
            write_bps: Metric::valid(0.),
            response_ms: Metric::default(),
            read_total: 0,
            written_total: 0,
            capacity: 0,
            temp_c: Metric::default(),
        }
    }

    fn net_totals(device: u16, rx_bps: f64, rx_total: u64) -> NetTotals {
        net_totals_tx(device, rx_bps, rx_total, 0., 0)
    }

    fn net_totals_tx(
        device: u16,
        rx_bps: f64,
        rx_total: u64,
        tx_bps: f64,
        tx_total: u64,
    ) -> NetTotals {
        NetTotals {
            device: DeviceIdx(device),
            rx_bps: Metric::valid(rx_bps),
            tx_bps: Metric::valid(tx_bps),
            rx_total,
            tx_total,
            link_speed_mbps: Metric::default(),
            state: LinkState::Up,
        }
    }

    struct Rig<'a> {
        engine: Arc<FakeEngine>,
        host: Entity<Host>,
        vcx: &'a mut VisualTestContext,
        version: u64,
        _driver: Entity<AnimationDriver>,
    }

    fn open(
        cx: &mut TestAppContext,
        gpus: Devices,
        disks: Devices,
        heights: (f32, f32),
    ) -> Rig<'_> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| s.devices = Arc::new(catalog()));
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            init_strings(cx, "en-US");
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let (host, vcx) = cx.add_window_view(move |_, cx| Host {
            // These tests cover the UI-PRF-017 sections, `chart_engines` since D-026.
            gpu: cx.new(|cx| {
                let mut gpu = GpuBox::new(&hub, &gpus, cx);
                gpu.set_mode("chart_engines", cx);
                gpu
            }),
            // These tests cover the UI-PRF-017 sections, `rows` since D-026.
            disk: cx.new(|cx| {
                let mut disk = DiskBox::new(&hub, &disks, cx);
                disk.set_mode("rows", cx);
                disk
            }),
            net: cx.new(|cx| NetBox::new(&hub, cx)),
            gpu_h: heights.0,
            disk_h: heights.1,
            gpu_w: 500.,
        });
        vcx.simulate_scale_factor_change(1.0);
        let driver = vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            driver
        });
        vcx.simulate_resize(size(px(500.), px(1600.)));
        draw(vcx);
        Rig {
            engine,
            host,
            vcx,
            version: 0,
            _driver: driver,
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
    }

    impl Rig<'_> {
        fn publish(&mut self, f: impl FnOnce(&mut Snapshot)) {
            self.engine.publish_with(f);
            self.vcx.run_until_parked();
        }

        fn system(&mut self, f: impl FnOnce(&mut SystemSnapshot)) {
            self.publish(|s| {
                let sys = Arc::make_mut(&mut s.system);
                sys.header.version += 1;
                f(sys);
            });
        }

        fn boxes(&self) -> (Entity<GpuBox>, Entity<DiskBox>, Entity<NetBox>) {
            self.vcx.read_entity(&self.host, |h, _| {
                (h.gpu.clone(), h.disk.clone(), h.net.clone())
            })
        }

        fn heights(&mut self, gpu_h: f32, disk_h: f32) {
            let host = self.host.clone();
            self.vcx.update(|_, cx| {
                host.update(cx, |h, cx| {
                    h.gpu_h = gpu_h;
                    h.disk_h = disk_h;
                    cx.notify();
                })
            });
            draw(self.vcx);
        }

        fn gpu_width(&mut self, w: f32) {
            let host = self.host.clone();
            self.vcx.update(|_, cx| {
                host.update(cx, |h, cx| {
                    h.gpu_w = w;
                    cx.notify();
                })
            });
            draw(self.vcx);
        }

        fn bounds(&mut self, id: String) -> gpui_kit::Bounds<gpui_kit::Pixels> {
            let id: &'static str = Box::leak(id.into_boxed_str());
            self.vcx.debug_bounds(id).expect(id)
        }
    }

    fn rate_iec(bps: f64) -> String {
        let mut s = String::new();
        units::rate_iec_auto(&mut s, bps);
        s
    }

    /// The box height that leaves a body of `body` px under the title line.
    fn box_h(body: f32) -> f32 {
        body + BOX_CHROME
    }

    /// The height of `n` body rows: 18 px each, 4 px apart.
    fn rows_h(n: usize) -> f32 {
        n as f32 * 18. + (n - 1) as f32 * 4.
    }

    #[gpui_kit::test]
    fn gpu_box_caps_at_two_with_more(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10, 11, 12]), Vec::new(), (450., 200.));
        let (gpu, _, _) = r.boxes();
        let (sections, more, text, shown) = r.vcx.update(|_, cx| {
            let g = gpu.read(cx);
            (
                g.sections().len(),
                g.more(),
                g.more_text().to_string(),
                g.shown(),
            )
        });
        assert_eq!((sections, more), (2, 1));
        assert_eq!(text, "+1 more");
        // 450 - 60 = 390 px body. Without totals each section has 3 engine rows and VRAM:
        // 48 + 4 × 22 = 136. The second adds a 22 px heading and the 22 px "+1 more" row, plus
        // 12 px between sections: 328.
        assert_eq!(shown, 2, "450 px fits both sections");
        let title = r.vcx.update(|_, cx| gpu.read(cx).shown_more_text());
        assert_eq!(title.as_ref(), "+1 more");
        let rows = r.bounds(rows_id(1));
        assert_eq!(
            f32::from(rows.size.height),
            rows_h(5),
            "3 engines, VRAM and the \"+1 more\" body row"
        );
        let mut copy = String::new();
        r.vcx.update(|_, cx| gpu.read(cx).copy_text(&mut copy));
        assert!(copy.ends_with("+1 more\n"), "{copy}");
    }

    #[gpui_kit::test]
    fn gpu_box_second_section_only_when_both_fit(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10, 11]), Vec::new(), (450., 200.));
        let (gpu, _, _) = r.boxes();
        let shown = |r: &mut Rig| r.vcx.update(|_, cx| gpu.read(cx).shown());
        assert_eq!(shown(&mut r), 2);
        // A 300 px body is below the 306 px two need.
        r.heights(box_h(300.), 200.);
        assert_eq!(shown(&mut r), 1, "300 px body: one section");
        r.heights(150., 200.);
        assert_eq!(shown(&mut r), 1, "150 px: one section");
        r.heights(450., 200.);
        assert_eq!(shown(&mut r), 2, "back to both");
    }

    /// Two GPUs with 3 engines, VRAM and the temperature and power row: each section is
    /// 48 + 5 × 22 = 158 px, the second adds a 22 px heading, with 12 px between: 350 px, the
    /// body of a 410 px box plus the range figures row and the space above the title
    /// (UI-PRF-059). Sections take their content height, so neither clips.
    #[gpui_kit::test]
    fn gpu_box_two_full_sections_at_410(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            devices("gpu", &[10, 11]),
            Vec::new(),
            (box_h(350.), 200.),
        );
        r.system(|s| {
            let mut t11 = gpu_engines(false);
            t11.device = DeviceIdx(11);
            s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(false)), (DeviceIdx(11), t11)]);
        });
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let state = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                (g.shown(), g.collapsed())
            })
        };
        assert_eq!(state(&mut r), (2, false), "410 px: two full sections");
        let axis = r.bounds(body_id(GPU_BOX_ID));
        for i in 0..2 {
            let rows = r.bounds(rows_id(i));
            assert_eq!(f32::from(rows.size.height), rows_h(5), "section {i}");
            assert!(
                rows.bottom() <= axis.bottom(),
                "section {i} rows end at {:?}, the body ends at {:?}",
                rows.bottom(),
                axis.bottom()
            );
        }
        let first = r.bounds(rows_id(0));
        let second = r.bounds(engine_row_id(1, "gpu"));
        assert!(first.bottom() < second.top(), "{first:?} above {second:?}");

        r.heights(box_h(349.), 200.);
        assert_eq!(
            state(&mut r),
            (1, false),
            "409 px: one full section and \"+1 more\""
        );
        let rows = r.bounds(rows_id(0));
        assert_eq!(f32::from(rows.size.height), rows_h(6));
    }

    /// The first GPU section's rows end inside the body, and are `n` rows high.
    fn assert_rows_above_axis(r: &mut Rig, n: usize, what: &str) {
        let rows = r.bounds(rows_id(0));
        let axis = r.bounds(body_id(GPU_BOX_ID));
        assert!(
            rows.bottom() <= axis.bottom(),
            "{what}: rows end at {:?}, the body ends at {:?}",
            rows.bottom(),
            axis.bottom()
        );
        assert_eq!(f32::from(rows.size.height), rows_h(n), "{what}");
    }

    /// Two GPUs with 3 engines, VRAM and the temperature and power row (UI-PRF-019 GPU fit,
    /// UI-PRF-059). The body is the box minus the 30 px chrome (`BOX_CHROME`).
    /// The first section in full with the "+1 more" row needs 48 + 6 × 22 = 180 px, with its
    /// busiest engine and "+2 more engines" 158 px, and with "+3 more engines" 136 px.
    /// - 134 px body: "+3 more engines", VRAM and "+1 more" (114 px); the
    ///   temperature and power row gives way.
    /// - 152 px body: "+3 more engines", VRAM, temperature and
    ///   power and "+1 more" (136 px).
    /// - 160 px body: the busiest engine and "+2 more engines" fit (158 px).
    /// - 100 px body, below the kind minimum: VRAM gives way too, so "+3 more engines" and
    ///   "+1 more" are 92 px.
    #[gpui_kit::test]
    fn gpu_box_min_height_shows_busiest_engine(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            devices("gpu", &[10, 11]),
            Vec::new(),
            (box_h(134.), 200.),
        );
        r.system(|s| {
            let mut t10 = gpu_engines(false);
            t10.dec_pct = Metric::valid(50.);
            let mut t11 = gpu_engines(false);
            t11.device = DeviceIdx(11);
            s.gpus = Arc::from([(DeviceIdx(10), t10), (DeviceIdx(11), t11)]);
        });
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let state = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                (
                    (g.shown(), g.collapsed(), g.busiest_shown(), g.cut_rows()),
                    g.shown_more_text().to_string(),
                    g.sections()[0].engines_more.to_string(),
                    g.sections()[0].engines_more_all.to_string(),
                )
            })
        };
        let no_engine_rows = |r: &mut Rig, what: &str| {
            for key in ["gpu", "enc", "dec"] {
                assert!(
                    r.vcx.debug_bounds(leak(engine_row_id(0, key))).is_none(),
                    "{what}: {key}"
                );
            }
        };

        let (fit, more, _, engines_more_all) = state(&mut r);
        assert_eq!(fit, (1, true, false, 1), "134 px body");
        assert_eq!(
            (more.as_str(), engines_more_all.as_str()),
            ("+1 more", "+3 more engines")
        );
        no_engine_rows(&mut r, "134 px body");
        assert_rows_above_axis(
            &mut r,
            3,
            "134 px body: \"+3 more engines\", VRAM, \"+1 more\"",
        );

        r.heights(box_h(152.), 200.);
        assert_eq!(state(&mut r).0, (1, true, false, 0), "152 px body");
        no_engine_rows(&mut r, "152 px body");
        assert_rows_above_axis(
            &mut r,
            4,
            "152 px body: \"+3 more engines\", VRAM, temperature and power, \"+1 more\"",
        );

        r.heights(box_h(160.), 200.);
        let (fit, more, engines_more, _) = state(&mut r);
        assert_eq!(fit, (1, true, true, 0), "160 px body");
        assert_eq!(
            (more.as_str(), engines_more.as_str()),
            ("+1 more", "+2 more engines")
        );
        assert!(
            r.vcx.debug_bounds(leak(engine_row_id(0, "dec"))).is_some(),
            "busiest row"
        );
        for key in ["gpu", "enc"] {
            assert!(
                r.vcx.debug_bounds(leak(engine_row_id(0, key))).is_none(),
                "{key}"
            );
        }
        assert_rows_above_axis(
            &mut r,
            5,
            "160 px body: busiest engine, \"+2 more engines\", VRAM, temperature and power, \
             \"+1 more\"",
        );

        r.heights(box_h(100.), 200.);
        assert_eq!(state(&mut r).0, (1, true, false, 2), "100 px body");
        assert_rows_above_axis(&mut r, 2, "100 px body: \"+3 more engines\", \"+1 more\"");
    }

    /// One GPU with 3 engines, VRAM and the temperature and power row: 158 px in full, 136 px
    /// with the busiest engine and "+2 more engines", 114 px with "+3 more engines".
    /// - 134 px body: "+3 more engines", VRAM, temperature and power.
    /// - 152 px body: the busiest engine and "+2 more engines".
    /// - 160 px body: every engine row.
    #[gpui_kit::test]
    fn gpu_box_one_gpu_min_height_whole_rows(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (box_h(134.), 200.));
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(false))]));
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let state = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                (g.collapsed(), g.busiest_shown(), g.cut_rows())
            })
        };
        assert_eq!(state(&mut r), (true, false, 0), "134 px body");
        assert_rows_above_axis(&mut r, 3, "134 px body");

        r.heights(box_h(152.), 200.);
        assert_eq!(state(&mut r), (true, true, 0), "152 px body");
        assert_rows_above_axis(&mut r, 4, "152 px body");

        r.heights(box_h(160.), 200.);
        assert!(!state(&mut r).0, "a 160 px body: not collapsed");
        for key in ["gpu", "enc", "dec"] {
            assert!(
                r.vcx.debug_bounds(leak(engine_row_id(0, key))).is_some(),
                "{key}"
            );
        }
        assert_rows_above_axis(&mut r, 5, "160 px body");
    }

    /// Two GPUs with 2 engines each (3D / Compute and a shared Video Encode/Decode), VRAM and
    /// the temperature and power row: a GPU with two engines collapses too (UI-PRF-019 GPU
    /// fit). The first section in full with "+1 more" needs 48 + 5 × 22 = 158 px, with
    /// "+2 more engines" 136 px.
    /// - 134 px body: "+2 more engines", VRAM and "+1 more".
    /// - 152 px body: "+2 more engines", VRAM, temperature and power and "+1 more".
    /// - 160 px body: both engine rows.
    #[gpui_kit::test]
    fn gpu_box_two_engines_collapse(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            devices("gpu", &[10, 11]),
            Vec::new(),
            (box_h(134.), 200.),
        );
        r.system(|s| {
            let mut t11 = gpu_engines(true);
            t11.device = DeviceIdx(11);
            s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(true)), (DeviceIdx(11), t11)]);
        });
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let state = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                (g.shown(), g.collapsed(), g.busiest_shown(), g.cut_rows())
            })
        };
        let (engines, more_all) = r.vcx.update(|_, cx| {
            let s = &gpu.read(cx).sections()[0];
            (s.engines.len(), s.engines_more_all.to_string())
        });
        assert_eq!((engines, more_all.as_str()), (2, "+2 more engines"));

        assert_eq!(state(&mut r), (1, true, false, 1), "134 px body");
        for key in ["gpu", "enc"] {
            assert!(
                r.vcx.debug_bounds(leak(engine_row_id(0, key))).is_none(),
                "134 px body: {key}"
            );
        }
        assert_rows_above_axis(&mut r, 3, "134 px body");

        r.heights(box_h(152.), 200.);
        assert_eq!(state(&mut r), (1, true, false, 0), "152 px body");
        assert_rows_above_axis(&mut r, 4, "152 px body");

        r.heights(box_h(160.), 200.);
        let (shown, collapsed, _, cut) = state(&mut r);
        assert_eq!((shown, collapsed, cut), (1, false, 0), "160 px body");
        for key in ["gpu", "enc"] {
            assert!(
                r.vcx.debug_bounds(leak(engine_row_id(0, key))).is_some(),
                "160 px body: {key}"
            );
        }
        assert_rows_above_axis(&mut r, 5, "160 px body");
    }

    /// Every GPU layout draws only whole rows, top first, at every body height (UI-PRF-019 GPU
    /// fit). The body steps from 0 to 300 px in 2 px steps, for 1 and 2 GPUs with 2 and 3
    /// engines, in each mode and Arrange choice. Every drawn row, chart, grid and cell ends at
    /// or above the body's bottom.
    #[gpui_kit::test]
    fn gpu_box_whole_rows_at_every_height(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10, 11]), Vec::new(), (300., 200.));
        let (gpu, _, _) = r.boxes();
        let layouts = [
            ("chart", Arrange::Auto),
            ("chart_engines", Arrange::Vertical),
            ("chart_engines", Arrange::Horizontal),
            ("engine_charts", Arrange::Auto),
            ("engine_charts", Arrange::Vertical),
            ("engine_charts", Arrange::Horizontal),
        ];
        for gpus in [1, 2] {
            for engines in [2, 3] {
                // 3D, encode and decode, or, with decode Unsupported, 3D and encode, in every
                // mode.
                let totals = |device: u16| {
                    let mut t = gpu_engines(false);
                    t.device = DeviceIdx(device);
                    if engines == 2 {
                        t.dec_pct = Metric::with(Validity::Unsupported);
                    }
                    t
                };
                let list = devices("gpu", &[10, 11][..gpus]);
                r.vcx
                    .update(|_, cx| gpu.update(cx, |g, cx| g.set_gpus(&list, cx)));
                r.system(|s| {
                    s.gpus = Arc::from([(DeviceIdx(10), totals(10)), (DeviceIdx(11), totals(11))])
                });
                for (mode, arrange) in layouts {
                    r.vcx.update(|_, cx| {
                        gpu.update(cx, |g, cx| {
                            g.set_mode(mode, cx);
                            g.set_arrange(arrange, cx);
                        })
                    });
                    let (sections, shown_engines) = r.vcx.update(|_, cx| {
                        let g = gpu.read(cx);
                        (g.sections().len(), g.sections()[0].engines.len())
                    });
                    assert_eq!(sections, gpus, "{mode}");
                    if mode != "chart" {
                        assert_eq!(shown_engines, engines, "{mode}");
                    }
                    for body in (0..=300).step_by(2) {
                        let body = body as f32;
                        let what = format!("{gpus} GPUs, {engines} engines, {mode} {arrange:?}");
                        r.heights(box_h(body), 200.);
                        let axis = r.bounds(body_id(GPU_BOX_ID));
                        assert_eq!(f32::from(axis.size.height), body, "{what}: body");
                        let mut ids = Vec::new();
                        for i in 0..2 {
                            ids.extend([rows_id(i), chart_id(i), grid_id(i), vram_id(i)]);
                            for key in ["gpu", "enc", "dec"] {
                                ids.push(engine_row_id(i, key));
                                ids.push(engine_cell_id(i, key));
                            }
                        }
                        let mut drawn = 0;
                        for id in ids {
                            let Some(b) = r.vcx.debug_bounds(leak(id.clone())) else {
                                continue;
                            };
                            // An empty container draws nothing. The grid fills what its
                            // section leaves, so only its cells count as drawn.
                            if b.size.height <= px(0.) {
                                continue;
                            }
                            if id != grid_id(0) && id != grid_id(1) {
                                drawn += 1;
                            }
                            assert!(
                                b.bottom() <= axis.bottom() + px(0.01),
                                "{what}, {body} px body: {id} ends at {:?}, the body \
                                 ends at {:?}",
                                b.bottom(),
                                axis.bottom()
                            );
                        }
                        if body < 22. {
                            assert_eq!(drawn, 0, "{what}, {body} px body: nothing drawn");
                        }
                        if body >= 22. {
                            assert!(drawn > 0, "{what}, {body} px body: a row shows");
                        }
                    }
                }
            }
        }
    }

    /// At the supported 720 px page width a GPU box is 224 px wide with a 198 px body: too
    /// narrow for label 72, meter 32, value 80 and the 96 px sparkline with their gaps (304 px).
    /// The engine rows drop the meter and keep the sparkline, and stay inside the body.
    #[gpui_kit::test]
    fn gpu_box_narrow_keeps_sparklines(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (300., 200.));
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(false))]));
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let meters = |r: &mut Rig| r.vcx.update(|_, cx| gpu.read(cx).meters());
        assert!(meters(&mut r), "500 px: meters");
        r.gpu_width(224.);
        assert!(!meters(&mut r), "224 px: no meters");
        let collapsed = r.vcx.update(|_, cx| gpu.read(cx).collapsed());
        assert!(!collapsed, "every engine row stays");
        let frame = r.bounds(GPU_BOX_ID.to_owned());
        for key in ["gpu", "enc", "dec"] {
            let row = r.bounds(engine_row_id(0, key));
            assert!(
                row.right() <= frame.right() - px(12.),
                "{key} row {row:?} inside the body of {frame:?}"
            );
            assert_eq!(f32::from(row.size.height), 18., "{key}");
        }
        r.gpu_width(330.);
        assert!(meters(&mut r), "330 px (304 px body): meters");
        r.gpu_width(329.);
        assert!(!meters(&mut r), "329 px: no meters");
    }

    /// D-027 Side by side: the engine rows get the width beside the chart, so the meters follow
    /// that width and not the body's. A 500 px box has room for meters across its body, but its
    /// rows beside the chart are narrower than the 304 px a meter row needs.
    #[gpui_kit::test]
    fn gpu_box_side_by_side_meters_follow_rows_width(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (300., 200.));
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(false))]));
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let meters = |r: &mut Rig| r.vcx.update(|_, cx| gpu.read(cx).meters());
        assert!(meters(&mut r), "500 px stacked: meters");
        r.vcx
            .update(|_, cx| gpu.update(cx, |b, cx| b.set_arrange(Arrange::Horizontal, cx)));
        draw(r.vcx);
        let row = r.bounds(engine_row_id(0, "gpu"));
        assert!(f32::from(row.size.width) < 304., "{row:?}");
        assert!(!meters(&mut r), "500 px side by side: no meters");
        r.gpu_width(700.);
        let row = r.bounds(engine_row_id(0, "gpu"));
        assert!(f32::from(row.size.width) >= 304., "{row:?}");
        assert!(meters(&mut r), "700 px side by side: meters");
        // A 330 px box (304 px body) cannot give both parts half: the rows keep room for the
        // whole VRAM value and the chart takes what is left.
        r.gpu_width(330.);
        let rows = r.bounds(rows_id(0));
        assert!(f32::from(rows.size.width) >= SIDE_ROWS_MIN_W, "{rows:?}");
        let chart = r.bounds(chart_id(0));
        assert!(f32::from(chart.size.width) >= 48., "{chart:?}");
        assert!(chart.right() < rows.left(), "{chart:?} left of {rows:?}");
        // A saved or dragged split keeps the same room for the rows: in a 320 px box (294 px
        // body) the chart gives up the 20% floor and keeps its 48 px.
        r.gpu_width(320.);
        r.vcx
            .update(|_, cx| gpu.update(cx, |b, cx| b.set_split(0.4, cx)));
        draw(r.vcx);
        let rows = r.bounds(rows_id(0));
        assert!(
            f32::from(rows.size.width) >= SIDE_ROWS_MIN_W,
            "saved: {rows:?}"
        );
        let chart = r.bounds(chart_id(0));
        assert!(f32::from(chart.size.width) >= 48., "saved: {chart:?}");
        assert!(
            chart.right() < rows.left(),
            "saved: {chart:?} left of {rows:?}"
        );
    }

    fn leak(id: String) -> &'static str {
        Box::leak(id.into_boxed_str())
    }

    #[gpui_kit::test]
    fn gpu_box_temp_power_optional(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (300., 200.));
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), gpu_totals(10))]));
        let (gpu, _, _) = r.boxes();
        let section = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                let s = &g.sections()[0];
                (
                    s.value.to_string(),
                    s.line.to_string(),
                    s.vram.value.to_string(),
                    s.vram.fraction,
                )
            })
        };
        let mut watts = String::new();
        units::watts(&mut watts, 120.);
        let (value, line, vram, fraction) = section(&mut r);
        assert_eq!(value, "25.0%");
        assert_eq!(line, watts, "temperature Unsupported: power only");
        assert_eq!(vram, "4.00 GiB / 16.00 GiB");
        assert_eq!(fraction, Some(0.25));

        r.system(|s| {
            let mut t = gpu_totals(10);
            t.temp_c = Metric::valid(62.);
            s.gpus = Arc::from([(DeviceIdx(10), t)]);
        });
        assert_eq!(section(&mut r).1, format!("62 °C · {watts}"));
    }

    /// GPU 10 with 3D, encode and decode at 25, 10 and 5 percent.
    fn gpu_engines(shared: bool) -> GpuTotals {
        let mut t = gpu_totals(10);
        t.enc_pct = Metric::valid(10.);
        t.dec_pct = Metric::valid(if shared { 10. } else { 5. });
        t.enc_dec_shared = shared;
        t
    }

    /// The first section's engine rows: (key, label, value, fraction).
    fn engine_rows(r: &mut Rig) -> Vec<(&'static str, String, String, Option<f32>)> {
        let (gpu, _, _) = r.boxes();
        r.vcx.update(|_, cx| {
            gpu.read(cx).sections()[0]
                .engines
                .iter()
                .map(|e| {
                    (
                        e.key,
                        e.meter.label.to_string(),
                        e.meter.value.to_string(),
                        e.meter.fraction,
                    )
                })
                .collect()
        })
    }

    #[gpui_kit::test]
    fn gpu_box_engine_rows_three(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (300., 200.));
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(false))]));
        let rows = engine_rows(&mut r);
        assert_eq!(
            rows,
            [
                ("gpu", "3D / Compute".into(), "25.0%".into(), Some(0.25)),
                ("enc", "Video Encode".into(), "10.0%".into(), Some(0.1)),
                ("dec", "Video Decode".into(), "5.0%".into(), Some(0.05)),
            ]
        );
        let (gpu, _, _) = r.boxes();
        let metrics: Vec<Vec<SeriesId>> = r.vcx.update(|_, cx| {
            gpu.read(cx).sections()[0]
                .engines
                .iter()
                .map(|e| e.chart.read(cx).series_ids().collect())
                .collect()
        });
        let id = |metric| SeriesId {
            device: DeviceIdx(10),
            metric,
        };
        assert_eq!(
            metrics,
            [
                vec![id(MetricId::GPU_UTIL)],
                vec![id(MetricId::GPU_ENC)],
                vec![id(MetricId::GPU_DEC)],
            ],
            "one sparkline per engine series"
        );
        let mut copy = String::new();
        r.vcx.update(|_, cx| gpu.read(cx).copy_text(&mut copy));
        assert!(
            copy.contains("3D / Compute\t25.0%\nVideo Encode\t10.0%\nVideo Decode\t5.0%\n"),
            "{copy}"
        );
    }

    #[gpui_kit::test]
    fn gpu_box_hidden_engine_not_drawn(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (300., 200.));
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(false))]));
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let charts = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                gpu.read(cx).sections()[0]
                    .engines
                    .iter()
                    .map(|e| {
                        (
                            e.key,
                            e.chart.entity_id(),
                            e.chart.read(cx).build_count.get(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
        };
        let before = charts(&mut r);
        assert_eq!(before.len(), 3);
        r.vcx.update(|_, cx| {
            TlSettings::set(
                cx,
                SettingKey::PerformanceHiddenGpuEngines,
                SettingValue::StrList(Arc::from(["dec".into()])),
            )
        });
        draw(r.vcx);
        let after = charts(&mut r);
        assert_eq!(
            after,
            before[..2],
            "the decode row is gone; the kept rows keep their charts and build nothing"
        );
        assert!(engine_rows(&mut r).iter().all(|row| row.0 != "dec"));
    }

    #[gpui_kit::test]
    fn gpu_box_shared_enc_dec_one_row(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (300., 200.));
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(true))]));
        let rows = engine_rows(&mut r);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].1, "3D / Compute");
        assert_eq!(
            rows[1],
            (
                "enc",
                "Video Encode/Decode".into(),
                "10.0%".into(),
                Some(0.1)
            )
        );
    }

    /// GPU 10 as an Intel GPU: Render, Compute, Copy, Video and Video Enhance at 25, 30, 40, 10
    /// and 5 percent, in the inventory with the `xe` module.
    fn intel_gpu(r: &mut Rig) {
        r.publish(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let mut t = gpu_totals(10);
            for (e, v) in [
                (GpuEngine::Render, 25.),
                (GpuEngine::Compute, 30.),
                (GpuEngine::Copy, 40.),
                (GpuEngine::Video, 10.),
                (GpuEngine::VideoEnhance, 5.),
            ] {
                t.engines.set(e, Metric::valid(v));
            }
            sys.gpus = Arc::from([(DeviceIdx(10), t)]);
            let inv = Arc::make_mut(&mut s.inventory);
            inv.header.version += 1;
            inv.graphics = Arc::from(
                [GraphicsAdapter {
                    device: Some(DeviceIdx(10)),
                    kernel_module: InvField {
                        value: Arc::from("xe"),
                        validity: Validity::Valid,
                    },
                    ..GraphicsAdapter::default()
                }]
                .as_slice(),
            );
        });
        draw(r.vcx);
    }

    fn set_gpu_mode(r: &mut Rig, mode: &str) {
        let (gpu, _, _) = r.boxes();
        r.vcx
            .update(|_, cx| gpu.update(cx, |g, cx| g.set_mode(mode, cx)));
        draw(r.vcx);
    }

    /// D-026: `engine_charts` draws one chart per engine of the GPU page's grid, in its order,
    /// with the engine name and percent; the other modes keep the three Overview engines.
    #[gpui_kit::test]
    fn overview_gpu_engine_charts_follow_gpu_page(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (344., 200.));
        intel_gpu(&mut r);
        set_gpu_mode(&mut r, "engine_charts");
        let rows = engine_rows(&mut r);
        assert_eq!(
            rows,
            [
                ("render", "Render/3D".into(), "25.0%".into(), Some(0.25)),
                ("compute", "Compute".into(), "30.0%".into(), Some(0.3)),
                ("copy", "Copy".into(), "40.0%".into(), Some(0.4)),
                ("video", "Video".into(), "10.0%".into(), Some(0.1)),
                (
                    "video-enhance",
                    "Video Processing".into(),
                    "5.0%".into(),
                    Some(0.05)
                ),
            ]
        );
        let (gpu, _, _) = r.boxes();
        let series: Vec<MetricId> = r.vcx.update(|_, cx| {
            gpu.read(cx).sections()[0]
                .engines
                .iter()
                .flat_map(|e| e.chart.read(cx).series_ids().map(|id| id.metric))
                .collect::<Vec<_>>()
        });
        assert_eq!(
            series,
            [
                MetricId::GPU_ENGINE_RENDER,
                MetricId::GPU_ENGINE_COMPUTE,
                MetricId::GPU_ENGINE_COPY,
                MetricId::GPU_ENGINE_VIDEO,
                MetricId::GPU_ENGINE_VIDEO_ENHANCE,
            ]
        );
        for (key, ..) in &rows {
            assert!(
                r.vcx.debug_bounds(leak(engine_cell_id(0, key))).is_some(),
                "{key}"
            );
        }
        assert!(r.vcx.debug_bounds(leak(vram_id(0))).is_some());

        // A hidden engine has no chart.
        r.vcx.update(|_, cx| {
            TlSettings::set(
                cx,
                SettingKey::PerformanceHiddenGpuEngines,
                SettingValue::StrList(Arc::from(["copy".into()])),
            )
        });
        draw(r.vcx);
        assert!(engine_rows(&mut r).iter().all(|row| row.0 != "copy"));
        assert!(
            r.vcx
                .debug_bounds(leak(engine_cell_id(0, "copy")))
                .is_none()
        );
    }

    /// The `engine_charts` grid of `n` cells: every cell the same size, at least 120 × 48 px,
    /// with the size `fill_grid` gives for the grid viewport. Returns the viewport and the cells.
    fn grid_cells(
        r: &mut Rig,
        keys: &[&str],
    ) -> (
        gpui_kit::Bounds<gpui_kit::Pixels>,
        Vec<gpui_kit::Bounds<gpui_kit::Pixels>>,
        GridFit,
    ) {
        let grid = r.bounds(grid_id(0));
        let cells: Vec<_> = keys
            .iter()
            .map(|k| r.bounds(engine_cell_id(0, k)))
            .collect();
        let fit = fill_grid(
            keys.len(),
            grid.size,
            size(px(120.), px(48.)),
            px(4.),
            px(16.),
        );
        for c in &cells {
            assert_eq!(c.size, fit.cell, "{c:?} in {grid:?}");
            assert!(
                c.size.width >= px(120.) && c.size.height >= px(48.),
                "{c:?}"
            );
        }
        (grid, cells, fit)
    }

    /// D-026: the engine grid fills the box with equal cells and the VRAM row below; when the
    /// cells do not fit at 120 × 48 px only whole cells show, the rest join "+n more engines"
    /// (UI-PRF-059), and the VRAM row stays in the body.
    #[gpui_kit::test]
    fn overview_grid_cells_fill_box(cx: &mut TestAppContext) {
        let keys = ["render", "compute", "copy", "video", "video-enhance"];
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (344., 200.));
        intel_gpu(&mut r);
        set_gpu_mode(&mut r, "engine_charts");
        // The rows under the grid: the VRAM row, with "+n more engines" above it when `rows` is 2.
        let check_vram = |r: &mut Rig, grid: gpui_kit::Bounds<gpui_kit::Pixels>, rows: f32| {
            let vram = r.bounds(vram_id(0));
            let axis = r.bounds(body_id(GPU_BOX_ID));
            assert!(grid.bottom() <= vram.top(), "{grid:?} above {vram:?}");
            assert!(vram.bottom() <= axis.bottom(), "{vram:?} above {axis:?}");
            assert_eq!(f32::from(vram.size.height), 18. * rows + 4. * (rows - 1.));
        };

        // 500 × 344: every cell inside the grid, which ends 4 px above the VRAM row.
        for (w, h) in [(500., 344.), (900., 600.), (300., 500.)] {
            r.gpu_width(w);
            r.heights(h, 200.);
            let (grid, cells, fit) = grid_cells(&mut r, &keys);
            assert!(!fit.scrolls, "{w} × {h}");
            for c in &cells {
                assert!(
                    c.left() >= grid.left()
                        && c.right() <= grid.right()
                        && c.top() >= grid.top()
                        && c.bottom() <= grid.bottom(),
                    "{w} × {h}: {c:?} in {grid:?}"
                );
            }
            // The cells fill the grid: no other column count gives larger cells.
            let area = |f: &GridFit| f32::from(f.cell.width) * f32::from(f.cell.height);
            for cols in 1..=keys.len() {
                let rows = keys.len().div_ceil(cols);
                let cw =
                    ((f32::from(grid.size.width) - 4. * (cols - 1) as f32) / cols as f32).floor();
                let ch =
                    ((f32::from(grid.size.height) - 4. * (rows - 1) as f32) / rows as f32).floor();
                if cw >= 120. && ch >= 48. {
                    assert!(cw * ch <= area(&fit), "{w} × {h}: {cols} columns");
                }
            }
            let first = cells[0];
            assert!(
                first.top() - grid.top() < px(1.) && first.left() - grid.left() < px(1.),
                "{first:?} at the grid's top left {grid:?}"
            );
            check_vram(&mut r, grid, 1.);
        }

        // 500 wide with a 94 px body: the cells do not all fit. Only whole cells show, the rest
        // join "+n more engines" and nothing scrolls (UI-PRF-059), in each Arrange choice.
        r.gpu_width(500.);
        r.heights(box_h(94.), 200.);
        let (gpu, _, _) = r.boxes();
        for arrange in [Arrange::Auto, Arrange::Vertical, Arrange::Horizontal] {
            r.vcx
                .update(|_, cx| gpu.update(cx, |g, cx| g.set_arrange(arrange, cx)));
            draw(r.vcx);
            let (drawn, more) = r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                (g.shown_cells(0), g.sections()[0].cells_more.to_string())
            });
            assert!(
                (1..keys.len()).contains(&drawn),
                "{arrange:?}: {drawn} cells"
            );
            assert_eq!(
                more,
                format!("+{} more engines", keys.len() - drawn),
                "{arrange:?}"
            );
            let grid = r.bounds(grid_id(0));
            for (i, k) in keys.iter().enumerate() {
                let cell = r.vcx.debug_bounds(leak(engine_cell_id(0, k)));
                if i >= drawn {
                    assert!(cell.is_none(), "{arrange:?}: cell {i} left out");
                    continue;
                }
                let c = cell.unwrap_or_else(|| panic!("{arrange:?}: cell {i}"));
                assert!(
                    c.left() >= grid.left()
                        && c.right() <= grid.right()
                        && c.top() >= grid.top()
                        && c.bottom() <= grid.bottom(),
                    "{arrange:?}: {c:?} whole in {grid:?}"
                );
                assert!(
                    c.size.width >= px(120.) || arrange == Arrange::Vertical,
                    "{arrange:?}: {c:?}"
                );
                assert!(
                    c.size.height >= px(48.) || arrange == Arrange::Horizontal,
                    "{arrange:?}: {c:?}"
                );
            }
            check_vram(&mut r, grid, 2.);
        }
    }

    #[test]
    fn fill_grid_takes_largest_cells() {
        let min = size(px(120.), px(48.));
        let fit = |n, w: f32, h: f32| fill_grid(n, size(px(w), px(h)), min, px(4.), px(16.));
        // 4 cells in 358 × 230: 2 columns of 177 × 113 beat 1 column of 358 × 54.
        let f = fit(4, 358., 230.);
        assert_eq!((f.cols, f.rows, f.scrolls), (2, 2, false));
        assert_eq!(f.cell, size(px(177.), px(113.)));
        // Too low for 48 px cells: as many columns as fit beside the 16 px scrollbar.
        let f = fit(5, 474., 60.);
        assert_eq!((f.cols, f.rows, f.scrolls), (3, 2, true));
        assert_eq!(f.cell, size(px(150.), px(48.)));
        // Narrower than one cell: one column the row's width.
        let f = fit(2, 100., 200.);
        assert_eq!((f.cols, f.cell.width), (1, px(100.)));
        assert_eq!(fit(0, 500., 500.), GridFit::default());
        assert_eq!(f.class(), fit(2, 100.4, 200.4).class(), "sub-pixel resize");
        assert_ne!(f.class(), fit(2, 102., 200.).class());
    }

    #[gpui_kit::test]
    fn disk_box_rows_and_rates(cx: &mut TestAppContext) {
        let mut r = open(cx, Vec::new(), devices("disk", &[20, 21]), (200., 300.));
        r.system(|s| {
            s.disks = Arc::from([
                (DeviceIdx(20), disk_totals(20, MIB)),
                (DeviceIdx(21), disk_totals(21, MIB)),
            ]);
        });
        let (_, disk, _) = r.boxes();
        let rows = r.vcx.update(|_, cx| disk.read(cx).rows().to_vec());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].read.as_ref(), rate_iec(MIB));
        assert_eq!(rows[0].write.as_ref(), rate_iec(0.));
        assert_eq!(rows[0].busy, Some(0.4));
        assert_eq!(rows[0].active.as_ref(), percent(40.));
        assert_eq!(rows[1].name.as_ref(), "disk 1");
    }

    fn percent(v: f32) -> String {
        let mut s = String::new();
        tl_ui::chart::format_value(
            tl_ui::chart::ValueFormat::Percent,
            v,
            tl_model::settings::NetworkUnit::default(),
            &mut s,
        );
        s
    }

    /// Disk totals at 1 MiB/s read with active time `pct`, for each of `idx`.
    fn active_disks(idx: &[u16], pct: &[f32]) -> Arc<[(DeviceIdx, DiskTotals)]> {
        idx.iter()
            .zip(pct)
            .map(|(d, p)| {
                (
                    DeviceIdx(*d),
                    DiskTotals {
                        active_pct: Metric::valid(*p),
                        ..disk_totals(*d, MIB)
                    },
                )
            })
            .collect::<Vec<_>>()
            .into()
    }

    /// How many sections the last render showed, and which of the rows it expanded.
    fn disk_state(r: &mut Rig) -> (usize, Vec<bool>) {
        let (_, disk, _) = r.boxes();
        r.vcx.update(|_, cx| {
            let d = disk.read(cx);
            (
                d.shown(),
                (0..d.rows().len()).map(|i| d.expanded(i)).collect(),
            )
        })
    }

    /// crw.55: three whole disks get three sections, each a header with name, model and active
    /// percent, and Read and Write sparklines on the disk's own series.
    #[gpui_kit::test]
    fn disk_box_three_disks_three_sections(cx: &mut TestAppContext) {
        let mut r = open(cx, Vec::new(), devices("disk", &[20, 21, 22]), (200., 300.));
        r.system(|s| s.disks = active_disks(&[20, 21, 22], &[40., 40., 40.]));
        r.publish(|s| {
            let mut details = DeviceDetailsSnapshot::default();
            details.header.version = 1;
            details.disks = Arc::from([(
                DeviceIdx(20),
                DiskDetails {
                    model: Total {
                        value: Arc::from("Samsung SSD 980"),
                        validity: Validity::Valid,
                    },
                    ..DiskDetails::default()
                },
            )]);
            s.device_details = Arc::new(details);
        });
        draw(r.vcx);
        assert_eq!(disk_state(&mut r), (3, vec![true, true, true]));
        let (_, disk, _) = r.boxes();
        let rows = r.vcx.update(|_, cx| disk.read(cx).rows().to_vec());
        assert_eq!(rows[0].name.as_ref(), "disk 0");
        assert_eq!(rows[0].model.as_ref(), "Samsung SSD 980");
        assert_eq!(rows[1].model.as_ref(), "", "no model: the name only");
        assert_eq!(rows[0].active.as_ref(), percent(40.));
        let id = |metric| SeriesId {
            device: DeviceIdx(20),
            metric,
        };
        let (read_ids, write_ids): (Vec<SeriesId>, Vec<SeriesId>) = r.vcx.update(|_, cx| {
            (
                rows[0].read_chart.read(cx).series_ids().collect(),
                rows[0].write_chart.read(cx).series_ids().collect(),
            )
        });
        assert_eq!(read_ids, [id(MetricId::DISK_READ)]);
        assert_eq!(write_ids, [id(MetricId::DISK_WRITE)]);
        for row in &rows {
            r.bounds(format!("{DISK_ROW_ID_PREFIX}{}", row.key.0));
            r.bounds(io_row_id(&row.key, "read"));
            r.bounds(io_row_id(&row.key, "write"));
        }

        // A catalog bump keeps every chart.
        let charts = |rows: &[DiskRow]| -> Vec<_> {
            rows.iter()
                .flat_map(|r| [r.read_chart.entity_id(), r.write_chart.entity_id()])
                .collect()
        };
        r.publish(|s| {
            let mut c = catalog();
            c.version = 2;
            s.devices = Arc::new(c);
        });
        let after = r.vcx.update(|_, cx| disk.read(cx).rows().to_vec());
        assert_eq!(charts(&after), charts(&rows), "no new charts");
    }

    /// crw.55: a partition in the disk inputs never gets a section.
    #[gpui_kit::test]
    fn disk_box_partitions_never_appear(cx: &mut TestAppContext) {
        let mut disks = devices("disk", &[20, 21, 22, 23]);
        disks[3].0 = DeviceKey(Arc::from("disk:sda1"));
        let mut r = open(cx, Vec::new(), disks, (200., 300.));
        let mut c = catalog();
        let mut entries = c.entries.to_vec();
        entries.push(DeviceEntry {
            parent: Some(DeviceIdx(20)),
            ..entry(23, disk_id(1, "sda1"), None)
        });
        c.entries = Arc::from(entries);
        c.version = 2;
        r.publish(|s| s.devices = Arc::new(c));
        draw(r.vcx);
        let (_, disk, _) = r.boxes();
        let idx: Vec<u16> = r
            .vcx
            .update(|_, cx| disk.read(cx).rows().iter().map(|r| r.idx.0).collect());
        assert_eq!(idx, [20, 21, 22]);
        assert_eq!(disk_state(&mut r).0, 3);
        assert!(
            r.vcx
                .debug_bounds(leak(format!("{DISK_ROW_ID_PREFIX}disk:sda1")))
                .is_none()
        );
    }

    /// crw.55: in a 152 px body (7 lines) 2 of 3 sections expand, the busiest first; at 300 px
    /// all 3.
    #[gpui_kit::test]
    fn disk_box_collapse_follows_activity(cx: &mut TestAppContext) {
        let idx = [20, 21, 22];
        let mut r = open(cx, Vec::new(), devices("disk", &idx), (200., box_h(152.)));
        r.system(|s| s.disks = active_disks(&idx, &[10., 80., 40.]));
        draw(r.vcx);
        assert_eq!(disk_state(&mut r), (3, vec![false, true, true]));
        let key = |n: usize| DeviceKey(Arc::from(format!("disk{n}")));
        assert!(
            r.vcx
                .debug_bounds(leak(io_row_id(&key(0), "read")))
                .is_none()
        );
        r.bounds(format!("{DISK_ROW_ID_PREFIX}disk0"));
        r.bounds(io_row_id(&key(1), "write"));

        r.system(|s| s.disks = active_disks(&idx, &[90., 5., 40.]));
        draw(r.vcx);
        assert_eq!(disk_state(&mut r), (3, vec![true, false, true]));

        r.heights(200., 300.);
        assert_eq!(disk_state(&mut r), (3, vec![true, true, true]));
    }

    /// crw.55: past 8 disks a "+n more" line; when the headers do not fit, only those that fit
    /// show, none expanded.
    #[gpui_kit::test]
    fn disk_box_rows_fit_height(cx: &mut TestAppContext) {
        let idx: Vec<u16> = (0..10).map(|i| 200 + i).collect();
        let mut r = open(cx, Vec::new(), devices("disk", &idx), (200., box_h(532.)));
        let (_, disk, _) = r.boxes();
        let state = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let d = disk.read(cx);
                (
                    d.rows().len(),
                    d.more(),
                    d.shown(),
                    (0..8).filter(|&i| d.expanded(i)).count(),
                    d.shown_more_text().to_string(),
                )
            })
        };
        // 600 px and the range figures row: a 540 px body, 24 lines: 8 headers, "+2 more" and 7
        // expanded sections.
        assert_eq!(state(&mut r), (8, 2, 8, 7, "+2 more".into()));
        // 200 px and the figures row: a 140 px body, 6 lines: 5 headers and "+5 more".
        r.heights(200., box_h(132.));
        assert_eq!(state(&mut r), (8, 2, 5, 0, "+5 more".into()));
    }

    #[gpui_kit::test]
    fn net_box_totals_follow_shown_classes(cx: &mut TestAppContext) {
        let mut r = open(cx, Vec::new(), Vec::new(), (200., 200.));
        r.system(|s| {
            s.nets = Arc::from([
                (DeviceIdx(30), net_totals(30, 1000., GIB)),
                (DeviceIdx(31), net_totals(31, 5000., 5 * GIB)),
                (DeviceIdx(32), net_totals(32, 9000., 9 * GIB)),
            ]);
        });
        let (_, _, net) = r.boxes();
        let texts = r
            .vcx
            .update(|_, cx| net.read(cx).texts().map(|t| t.to_string()));
        assert_eq!(texts[4], "1.00 GiB", "eth0 only");
        let mut down = String::new();
        tl_ui::chart::format_value(
            tl_ui::chart::ValueFormat::RateNetwork,
            1000.,
            tl_model::settings::NetworkUnit::default(),
            &mut down,
        );
        assert_eq!(texts[0], down);
        let ids: Vec<SeriesId> = r
            .vcx
            .update(|_, cx| net.read(cx).down_chart().read(cx).series_ids().collect());
        assert_eq!(
            ids,
            [SeriesId {
                device: DeviceIdx(NET_AGG),
                metric: MetricId::NET_RX
            }]
        );
    }

    /// The aggregate download and upload series: 99999 outside the 60 s window, then 8000 and
    /// 6000 B/s (download) and 7000 and 2000 B/s (upload).
    fn publish_net_series(r: &mut Rig) {
        r.version += 1;
        let version = r.version;
        let now = now_boot_ns();
        let view = |metric, values: [f32; 3]| {
            let at = [90, 30, 0];
            Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(NET_AGG),
                    metric,
                },
                version,
                period_ns: 1_000_000_000,
                samples: at
                    .iter()
                    .zip(values)
                    .map(|(s, v)| Sample::valid(now - s * 1_000_000_000, v))
                    .collect::<Vec<_>>()
                    .into(),
            })
        };
        let views = [
            view(MetricId::NET_RX, [99999., 8000., 6000.]),
            view(MetricId::NET_TX, [99999., 7000., 2000.]),
        ];
        r.publish(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: Arc::from(views),
                slow: Arc::from([]),
            });
        });
    }

    fn two_adapters(r: &mut Rig) {
        r.system(|s| {
            s.nets = Arc::from([
                (DeviceIdx(30), net_totals_tx(30, 1000., GIB, 100., GIB / 2)),
                (DeviceIdx(33), net_totals_tx(33, 5000., 2 * GIB, 400., GIB)),
            ]);
        });
        publish_net_series(r);
    }

    fn rate(unit: tl_model::settings::NetworkUnit, v: f32) -> String {
        let mut out = String::new();
        tl_ui::chart::format_value(tl_ui::chart::ValueFormat::RateNetwork, v, unit, &mut out);
        out
    }

    #[gpui_kit::test]
    fn net_box_two_adapters_sum_peak_total(cx: &mut TestAppContext) {
        let mut r = open(cx, Vec::new(), Vec::new(), (200., 200.));
        two_adapters(&mut r);
        let (_, _, net) = r.boxes();
        let unit = tl_model::settings::NetworkUnit::default();
        let (texts, copy) = r.vcx.update(|_, cx| {
            let n = net.read(cx);
            let mut copy = String::new();
            n.copy_text(&mut copy);
            (n.texts().map(|t| t.to_string()), copy)
        });
        // The rates sum the live rates; the peaks come from the series over the last 60 s.
        assert_eq!(texts[0], rate(unit, 6000.), "download rate");
        assert_eq!(texts[1], rate(unit, 500.), "upload rate");
        assert_eq!(
            texts[2],
            rate(unit, 8000.),
            "download peak, 99999 is outside 60 s"
        );
        assert_eq!(texts[3], rate(unit, 7000.), "upload peak");
        assert_eq!(texts[4], "3.00 GiB");
        assert_eq!(texts[5], "1.50 GiB");
        assert!(
            copy.starts_with("Network\twlan0\n"),
            "wlan0 is the busiest: {copy}"
        );
    }

    /// tasklens-o55.54: the Download line draws the rate, then its peak ("1.4 Mbps, peak 4.4
    /// Mbps"); there are no Peak or Total rows, and hovering the line opens a tooltip with the
    /// total since boot.
    #[gpui_kit::test]
    fn net_box_rate_line_has_peak_and_total_tooltip(cx: &mut TestAppContext) {
        use tl_ui::components::tooltip::DELAY_CONTROL;
        use tl_ui::pages::performance::overview::fit::take_fit_texts;
        use tl_ui::pages::performance::overview::net_box::DOWN_RATE_ID;
        let mut r = open(cx, Vec::new(), Vec::new(), (200., 200.));
        two_adapters(&mut r);
        r.vcx.update(|window, _| window.activate_window());
        let _ = take_fit_texts();
        draw(r.vcx);
        let unit = tl_model::settings::NetworkUnit::default();
        let peak = format!(", peak {}", rate(unit, 8000.));
        let texts = take_fit_texts();
        assert!(
            texts.iter().any(|t| t.whole == rate(unit, 6000.)),
            "the download rate: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.whole == peak.as_str()),
            "{peak:?} after it: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.whole == "3.00 GiB"),
            "no Total row: {texts:?}"
        );
        assert!(r.vcx.debug_bounds("TlTooltip-text").is_none());
        let line = r.vcx.debug_bounds(DOWN_RATE_ID).expect("the Download line");
        r.vcx
            .simulate_mouse_move(line.center(), None, gpui_kit::Modifiers::none());
        r.vcx.executor().advance_clock(DELAY_CONTROL);
        r.vcx.run_until_parked();
        draw(r.vcx);
        draw(r.vcx);
        assert!(
            r.vcx.debug_bounds("TlTooltip-text").is_some(),
            "the total since boot in a tooltip"
        );
    }

    #[gpui_kit::test]
    fn net_box_formats_like_network_page(cx: &mut TestAppContext) {
        use tl_model::settings::NetworkUnit;
        let mut r = open(cx, Vec::new(), Vec::new(), (200., 200.));
        two_adapters(&mut r);
        let (_, _, net) = r.boxes();
        for (unit, key) in [(NetworkUnit::Bytes, "bytes"), (NetworkUnit::Bits, "bits")] {
            r.vcx.update(|_, cx| {
                TlSettings::set(cx, SettingKey::UnitsNetwork, SettingValue::Str(key.into()))
            });
            draw(r.vcx);
            let texts = r
                .vcx
                .update(|_, cx| net.read(cx).texts().map(|t| t.to_string()));
            assert_eq!(texts[0], rate(unit, 6000.), "{key} rate");
            assert_eq!(texts[2], rate(unit, 8000.), "{key} peak");
            assert_eq!(texts[3], rate(unit, 7000.), "{key} upload peak");
        }
    }

    #[gpui_kit::test]
    fn net_box_ignores_unrelated_series(cx: &mut TestAppContext) {
        let mut r = open(cx, Vec::new(), Vec::new(), (200., 200.));
        two_adapters(&mut r);
        draw(r.vcx);
        let (_, _, net) = r.boxes();
        let count = |r: &mut Rig| r.vcx.update(|_, cx| net.read(cx).notify_count.get());
        let before = count(&mut r);
        r.version += 1;
        let version = r.version;
        let view = Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(100),
                metric: MetricId::CPU_TOTAL,
            },
            version,
            period_ns: 100_000_000,
            samples: vec![Sample::valid(now_boot_ns(), 10.)].into(),
        });
        r.publish(|s| {
            // The net series stay at their versions; only the CPU series is new.
            let mut series = s.series.series.to_vec();
            series.push(view);
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: Arc::from(series),
                slow: Arc::from([]),
            });
        });
        assert_eq!(
            count(&mut r),
            before,
            "a CPU-only series tick does not notify"
        );
    }

    #[gpui_kit::test]
    fn gpu_disk_net_ignore_cpu_tick(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            devices("gpu", &[10]),
            devices("disk", &[20]),
            (300., 300.),
        );
        draw(r.vcx);
        let (gpu, disk, net) = r.boxes();
        let counts = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let (g, d, n) = (gpu.read(cx), disk.read(cx), net.read(cx));
                let builds = |v: &Entity<tl_ui::chart::ChartView>| v.read(cx).build_count.get();
                let mut counts = vec![
                    g.render_count.get(),
                    g.notify_count.get(),
                    builds(&g.sections()[0].chart),
                    d.render_count.get(),
                    d.notify_count.get(),
                    builds(&d.rows()[0].read_chart),
                    builds(&d.rows()[0].write_chart),
                    n.render_count.get(),
                    n.notify_count.get(),
                    builds(n.down_chart()),
                    builds(n.up_chart()),
                ];
                counts.extend(g.sections()[0].engines.iter().map(|e| builds(&e.chart)));
                counts
            })
        };
        let before = counts(&mut r);
        assert_eq!(before.len(), 14, "three engine sparklines");
        r.version += 1;
        let version = r.version;
        let newest = now_boot_ns();
        let view = Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(100),
                metric: MetricId::CPU_TOTAL,
            },
            version,
            period_ns: 100_000_000,
            samples: vec![Sample::valid(newest, 10.)].into(),
        });
        r.publish(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: Arc::from([view]),
                slow: Arc::from([]),
            });
        });
        draw(r.vcx);
        assert_eq!(
            counts(&mut r),
            before,
            "a CPU tick touches none of the three"
        );
    }
}

// The OverviewPage shell (crw.40).

#[cfg(feature = "test-hooks")]
mod page {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::{
        AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Render,
        StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
        size,
    };
    use tl_model::client::EngineClient;
    use tl_model::demand::{CpuDemand, Demand};
    use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{
        AppearanceSettings, OverviewGridSetting, OverviewLayoutSetting, Settings, ThemeSettings,
        UiState,
    };
    use tl_model::snapshot::{ComponentHeader, SeriesSet};
    use tl_model::time::now_boot_ns;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::chart::ChartView;
    use tl_ui::components::kv_grid;
    use tl_ui::components::toast::ToastQueue;
    use tl_ui::demand::DemandAggregator;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::DeviceKey;
    use tl_ui::pages::performance::actions::{
        PerfOverviewEditLayout, PerfOverviewResetBox, PerfOverviewResetLayout,
        PerfOverviewSetBoxArrange, PerfOverviewSetBoxMode, PerfOverviewSetBoxPower,
        bind_keys_performance,
    };
    use tl_ui::pages::performance::overview::box_menu::gear_id;
    use tl_ui::pages::performance::overview::cpu_box::{
        CORE_CHARTS_ID, CPU_BOX_ID, CPU_SPLITTER_ID, CPU_SPLITTER_LINE_ID, POWER_SPLITTER_ID,
        POWER_STRIP_ID, core_chart_id,
    };
    use tl_ui::pages::performance::overview::edit::{
        GHOST_ID, TOOLBAR_ID, corner_id, hide_button_id, title_handle_id,
    };
    use tl_ui::pages::performance::overview::inner_split::hit_selector;
    use tl_ui::pages::performance::overview::layout::{
        Arrange, BoxKind, GridBox, OverviewLayout, ROW_UNIT_DEFAULT, place,
    };
    use tl_ui::pages::performance::overview::mem_box::MEM_SPLITTER_ID;
    use tl_ui::pages::performance::overview::net_box::NET_STACK_SPLITTER_ID;
    use tl_ui::pages::performance::overview::{
        BOX_ID_PREFIX, OVERVIEW_KEY, OverviewPage, RANGE_HEADER_ID,
    };
    use tl_ui::pages::performance::page::DevicePage as _;
    use tl_ui::pages::performance::sidebar::DeviceSidebar;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;

    const CPUS: u16 = 4;

    struct Host {
        page: Option<Entity<OverviewPage>>,
        sidebar: Entity<DeviceSidebar>,
        /// Esc presses that reached the global Esc rule.
        escapes: Rc<Cell<u32>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let escapes = Rc::clone(&self.escapes);
            // The sidebar draws out of the way, so its demand part carries this window as in
            // the main window.
            div()
                .key_context(tl_ui::actions::GLOBAL_CONTEXT)
                .on_action(move |_: &tl_ui::actions::EscapeChain, _, _| {
                    escapes.set(escapes.get() + 1);
                })
                .relative()
                .size_full()
                .children(
                    self.page
                        .clone()
                        .map(|p| p.cached(StyleRefinement::default().size_full())),
                )
                .child(
                    div()
                        .absolute()
                        .w(px(0.))
                        .h(px(0.))
                        .overflow_hidden()
                        .child(self.sidebar.clone()),
                )
        }
    }

    fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("dev"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind,
            hot_c: None,
        }
    }

    /// CPU package 0 with logical CPUs at 100.., memory 1, a GPU at 2 when `gpu`, disks 3 and 4,
    /// eth0 at 5, the disk and net aggregates at 50 and 51.
    fn catalog(gpu: bool) -> DeviceCatalog {
        let mut entries = vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
        ];
        if gpu {
            entries.push(entry(
                2,
                DeviceIdentity::Gpu {
                    uuid: [1; 16],
                    pci_bus_id: Arc::from("0000:01:00.0"),
                },
                None,
            ));
        }
        for (idx, name) in [(3, "sda"), (4, "sdb")] {
            entries.push(entry(
                idx,
                DeviceIdentity::Disk {
                    major: 8,
                    minor: u32::from(idx),
                    name: Arc::from(name),
                    wwn_or_serial: None,
                },
                None,
            ));
        }
        entries.push(entry(
            5,
            DeviceIdentity::Net {
                netns_ino: 1,
                ifindex: 2,
                name: Arc::from("eth0"),
            },
            Some(NetKind::Wired),
        ));
        entries.push(entry(
            50,
            DeviceIdentity::Aggregate(AggregateKind::Disks),
            None,
        ));
        entries.push(entry(
            51,
            DeviceIdentity::Aggregate(AggregateKind::Nets),
            None,
        ));
        entries
            .extend((0..CPUS).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id }, None)));
        DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        }
    }

    struct Rig<'a> {
        engine: Arc<FakeEngine>,
        host: Entity<Host>,
        page: Entity<OverviewPage>,
        demand: Entity<DemandAggregator>,
        hub: Entity<SnapshotHub>,
        vcx: &'a mut VisualTestContext,
        version: u64,
        escapes: Rc<Cell<u32>>,
        /// The toast queue of a window opened by [`open_rooted`].
        toasts: Option<Entity<ToastQueue>>,
        _driver: Entity<AnimationDriver>,
    }

    /// One window of `w × h` that is the main window, with a sidebar (drawn 0 × 0) and the
    /// Overview page filling the window. Smooth graphs are off, so a live chart draws once per
    /// sample and not on every frame.
    fn open(cx: &mut TestAppContext, gpu: bool, w: f32, h: f32) -> Rig<'_> {
        open_with(cx, gpu, w, h, false)
    }

    /// [`open`] with `smooth_graphs` set to `smooth`.
    fn open_with(cx: &mut TestAppContext, gpu: bool, w: f32, h: f32, smooth: bool) -> Rig<'_> {
        open_in(cx, gpu, w, h, smooth, false)
    }

    /// [`open`] in a window with the kit `Root` and a toast queue.
    fn open_rooted(cx: &mut TestAppContext, gpu: bool, w: f32, h: f32) -> Rig<'_> {
        open_in(cx, gpu, w, h, false, true)
    }

    fn open_in(
        cx: &mut TestAppContext,
        gpu: bool,
        w: f32,
        h: f32,
        smooth: bool,
        rooted: bool,
    ) -> Rig<'_> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| s.devices = Arc::new(catalog(gpu)));
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            init_strings(cx, "en-US");
            kv_grid::init(cx);
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            let mut settings = Settings::default();
            settings.motion.smooth_graphs = smooth;
            cx.set_global(BootSettings {
                settings,
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let (hub2, demand2) = (hub.clone(), demand.clone());
        let escapes = Rc::new(Cell::new(0));
        let host_escapes = Rc::clone(&escapes);
        let build = move |cx: &mut Context<Host>| {
            let sidebar = cx.new(|cx| DeviceSidebar::new(hub2.clone(), demand2.clone(), cx));
            let page = cx.new(|cx| {
                OverviewPage::new(
                    DeviceKey(Arc::from(OVERVIEW_KEY)),
                    hub2,
                    demand2,
                    &sidebar,
                    cx,
                )
            });
            Host {
                page: Some(page),
                sidebar,
                escapes: host_escapes,
            }
        };
        let (host, vcx, toasts) = if rooted {
            cx.update(tl_ui::components::toast::init);
            let slot: Rc<RefCell<Option<Entity<Host>>>> = Rc::default();
            let out = Rc::clone(&slot);
            let window = cx.add_window(move |window, cx| {
                let host = cx.new(build);
                *out.borrow_mut() = Some(host.clone());
                gpui_kit::base::Root::new(host, window, cx)
            });
            let vcx = VisualTestContext::from_window(window.into(), cx).into_mut();
            let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.))));
            let host = slot.borrow_mut().take().expect("host");
            (host, vcx, Some(queue))
        } else {
            let (host, vcx) = cx.add_window_view(move |_, cx| build(cx));
            (host, vcx, None)
        };
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(w), px(h)));
        let driver = vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            let handle = window.window_handle();
            demand.update(cx, |d, _| d.set_main_window(handle));
            driver
        });
        draw(vcx);
        let page = vcx.read_entity(&host, |h, _| h.page.clone().expect("page"));
        Rig {
            engine,
            host,
            page,
            demand,
            hub,
            vcx,
            version: 0,
            escapes,
            toasts,
            _driver: driver,
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..2 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
    }

    fn box_id(kind: BoxKind) -> &'static str {
        match kind {
            BoxKind::Cpu => "perf-overview-box-cpu",
            BoxKind::Memory => "perf-overview-box-memory",
            BoxKind::Gpu => "perf-overview-box-gpu",
            BoxKind::Disk => "perf-overview-box-disk",
            BoxKind::Net => "perf-overview-box-net",
            BoxKind::TopProcs => "perf-overview-box-processes",
            BoxKind::Timeline => "perf-overview-box-timeline",
        }
    }

    impl Rig<'_> {
        fn set_active(&mut self, active: bool) {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.update(cx, |p, cx| p.set_active(active, cx)));
        }

        fn merged(&mut self) -> Demand {
            let demand = self.demand.clone();
            self.vcx.update(|_, cx| demand.read(cx).merged())
        }

        /// Page render and notify counts, then each box's render and notify counts, then each
        /// chart's render and build counts (CPU, memory, GPU, disk, net down, net up).
        fn counts(&mut self) -> Vec<u32> {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let p = page.read(cx);
                let (c, m, g, d, n) = (
                    p.cpu_box().read(cx),
                    p.mem_box().read(cx),
                    p.gpu_box().read(cx),
                    p.disk_box().read(cx),
                    p.net_box().read(cx),
                );
                let mut out = vec![
                    p.render_count.get(),
                    p.notify_count.get(),
                    c.render_count.get(),
                    c.notify_count.get(),
                    m.render_count.get(),
                    m.notify_count.get(),
                    g.render_count.get(),
                    g.notify_count.get(),
                    d.render_count.get(),
                    d.notify_count.get(),
                    n.render_count.get(),
                    n.notify_count.get(),
                ];
                let mut charts: Vec<&Entity<ChartView>> = vec![c.total_chart(), &m.sparks()[0]];
                charts.extend(g.sections().iter().map(|s| &s.chart));
                if let Some(row) = d.rows().first() {
                    charts.extend([&row.read_chart, &row.write_chart]);
                }
                charts.extend([n.down_chart(), n.up_chart()]);
                for chart in charts {
                    let v = chart.read(cx);
                    out.push(v.render_count.get());
                    out.push(v.build_count.get());
                }
                out
            })
        }

        /// Publishes the static RAPL state the CPU box reads.
        fn set_rapl(&mut self, rapl: Option<tl_model::snapshot::RaplState>) {
            self.engine.publish_with(|s| {
                let d = Arc::make_mut(&mut s.device_details);
                Arc::make_mut(&mut d.cpu).rapl = rapl;
                d.header.version += 1;
            });
            draw(self.vcx);
        }

        /// One CPU tick: the package and every logical CPU's `CPU_TOTAL`, a new value each time.
        fn cpu_tick(&mut self) {
            self.version += 1;
            let version = self.version;
            let newest = now_boot_ns();
            let value = 10. * version as f32;
            let mut views: Vec<_> = [0]
                .into_iter()
                .chain((0..CPUS).map(|id| 100 + id))
                .map(|device| {
                    Arc::new(SeriesView {
                        id: SeriesId {
                            device: DeviceIdx(device),
                            metric: MetricId::CPU_TOTAL,
                        },
                        version,
                        period_ns: 100_000_000,
                        samples: (0..5u64)
                            .map(|k| Sample::valid(newest - (4 - k) * 100_000_000, value))
                            .collect::<Vec<_>>()
                            .into(),
                    })
                })
                .collect();
            views.sort_by_key(|v| v.id);
            self.engine.publish_with(|s| {
                s.series = Arc::new(SeriesSet {
                    header: ComponentHeader {
                        version,
                        ..ComponentHeader::default()
                    },
                    series: views.into(),
                    slow: Arc::from([]),
                });
            });
            draw(self.vcx);
        }
    }

    #[gpui_kit::test]
    fn overview_places_boxes_grid_default(cx: &mut TestAppContext) {
        let r = open(cx, true, 1200., 900.);
        // With the status bar on the Overview the page has no title row: the boxes start at
        // the top (tasklens-o55.54).
        assert!(
            r.vcx.debug_bounds(RANGE_HEADER_ID).is_none(),
            "no title row"
        );
        let want = place(
            &OverviewLayout::grid_default(),
            |_| true,
            size(px(1200.), px(900.)),
            1.,
            ROW_UNIT_DEFAULT,
        );
        assert_eq!(want.boxes.len(), 6, "five device boxes and the timeline");
        for p in &want.boxes {
            let got = r.vcx.debug_bounds(box_id(p.kind)).expect("box drawn");
            assert_eq!(got, p.bounds, "{:?}", p.kind);
        }
        assert!(box_id(BoxKind::Cpu).starts_with(BOX_ID_PREFIX));
        let placed = r.vcx.update(|_, cx| r.page.read(cx).placement().clone());
        assert_eq!(placed, want);
    }

    #[gpui_kit::test]
    fn overview_drops_gpu_box_without_gpu(cx: &mut TestAppContext) {
        let r = open(cx, false, 1200., 900.);
        assert!(
            r.vcx.debug_bounds(box_id(BoxKind::Gpu)).is_none(),
            "no GPU box"
        );
        // tasklens-o55.54: Disk and Net share the GPU's columns and fill the row.
        for (kind, x, w) in [(BoxKind::Disk, 16., 581.), (BoxKind::Net, 603., 581.)] {
            let b = r.vcx.debug_bounds(box_id(kind)).expect("box drawn");
            assert_eq!((b.left(), b.size.width), (px(x), px(w)), "{kind:?}");
        }
    }

    #[gpui_kit::test]
    fn overview_demand_on_off_and_release(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        let off = r.merged();
        assert!(!off.cpu_fast, "nothing sent while inactive");
        r.set_active(true);
        let on = r.merged();
        assert!(on.any_surface_visible);
        assert_eq!(on.cpu, CpuDemand::PerCore);
        assert!(
            on.cpu_fast && on.memory && on.disks && on.nets && on.gpus,
            "{on:?}"
        );
        r.set_active(false);
        assert_eq!(r.merged(), off, "inactive: part gone");

        r.set_active(true);
        assert!(r.merged().cpu_fast);
        let host = r.host.clone();
        r.vcx.update(|_, cx| {
            host.update(cx, |h, cx| {
                h.page = None;
                cx.notify();
            })
        });
        drop(r.page);
        draw(r.vcx);
        let demand = r.demand.clone();
        assert_eq!(
            r.vcx.update(|_, cx| demand.read(cx).merged()),
            off,
            "dropped: part gone"
        );
    }

    #[gpui_kit::test]
    fn overview_idle_tick_renders_nothing(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.cpu_tick();
        draw(r.vcx);
        let before = r.counts();
        for _ in 0..3 {
            r.engine
                .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
            draw(r.vcx);
        }
        assert_eq!(r.counts(), before, "processes only: nothing renders");
    }

    /// crw.46: with smooth graphs on, the Overview charts still step at the sample rate, so
    /// the animation frames between ticks render nothing. The sidebar cards keep sliding.
    #[gpui_kit::test]
    fn overview_charts_step_with_smooth_graphs_on(cx: &mut TestAppContext) {
        let mut r = open_with(cx, true, 1200., 900., true);
        r.set_active(true);
        r.cpu_tick();
        draw(r.vcx);
        let page = r.page.clone();
        let renders = |r: &mut Rig| r.vcx.read_entity(&page, |p, _| p.render_count.get());
        let before = renders(&mut r);
        for _ in 0..3 {
            r.cpu_tick();
            for _ in 0..5 {
                r.vcx.executor().advance_clock(Duration::from_millis(16));
                draw(r.vcx);
            }
        }
        let n = renders(&mut r) - before;
        assert!(n <= 3, "page renders {n}: at most one per tick");
        let host = r.host.clone();
        let (overview_smooth, card_smooth) = r.vcx.update(|_, cx| {
            let chart = page.read(cx).cpu_box().read(cx).total_chart().clone();
            let sidebar = host.read(cx).sidebar.clone();
            let card = sidebar.read(cx).cards()[0].chart.clone();
            (chart.read(cx).spec.smooth, card.read(cx).spec.smooth)
        });
        assert!(!overview_smooth, "the Overview CPU chart steps");
        assert!(card_smooth, "the sidebar card keeps sliding");
    }

    /// crw.43 (TMOG): about 200 pointer moves over every box render no page and no box, and
    /// build no chart geometry.
    #[gpui_kit::test]
    fn overview_pointer_sweep_renders_nothing(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.cpu_tick();
        draw(r.vcx);
        let before = r.counts();
        let kinds = [
            BoxKind::Cpu,
            BoxKind::Memory,
            BoxKind::Gpu,
            BoxKind::Disk,
            BoxKind::Net,
        ];
        let mut moves = 0;
        for kind in kinds {
            let b = r.vcx.debug_bounds(box_id(kind)).expect("box drawn");
            // A 8 x 5 raster over the box, inside its edges.
            for i in 0..8 {
                for j in 0..5 {
                    let x = b.left() + b.size.width * ((i as f32 + 0.5) / 8.);
                    let y = b.top() + b.size.height * ((j as f32 + 0.5) / 5.);
                    moves += 1;
                    // The Memory and Net boxes' inner splitters light up under the pointer, so
                    // those boxes render by design (UI-PRF-019); the sweep steps over them.
                    let at = gpui_kit::point(x, y);
                    if [MEM_SPLITTER_ID, NET_STACK_SPLITTER_ID]
                        .iter()
                        .any(|id| r.vcx.debug_bounds(id).is_some_and(|gap| gap.contains(&at)))
                    {
                        continue;
                    }
                    r.vcx.simulate_event(gpui_kit::MouseMoveEvent {
                        position: gpui_kit::point(x, y),
                        pressed_button: None,
                        modifiers: gpui_kit::Modifiers::none(),
                    });
                }
            }
            draw(r.vcx);
        }
        assert_eq!(moves, 200);
        let after = r.counts();
        assert_eq!(
            after[..12],
            before[..12],
            "page and box renders and notifies"
        );
        // Chart pairs from index 12: (renders, geometry builds).
        for i in (13..after.len()).step_by(2) {
            assert_eq!(after[i], before[i], "chart {} built geometry", (i - 12) / 2);
        }
    }

    #[gpui_kit::test]
    fn overview_cpu_tick_page_never_notifies(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.cpu_tick();
        draw(r.vcx);
        let before = r.counts();
        for _ in 0..3 {
            r.cpu_tick();
        }
        let after = r.counts();
        assert_eq!(after[1], before[1], "the page never notifies");
        for (i, name) in [(5, "memory"), (7, "GPU"), (9, "disk"), (11, "net")] {
            assert_eq!(after[i], before[i], "{name} box notify");
        }
        // Chart pairs from index 12: CPU, memory, GPU, disk, net down, net up.
        assert!(after[12] > before[12], "the CPU chart rendered");
        for (i, name) in [
            (15, "memory"),
            (17, "GPU"),
            (19, "disk"),
            (21, "net down"),
            (23, "net up"),
        ] {
            assert_eq!(after[i], before[i], "{name} chart builds");
        }
        let renders = after[0] - before[0];
        assert!(renders <= 3, "page renders {renders}: at most one per tick");
    }

    #[gpui_kit::test]
    fn overview_resize_relayouts_once(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        let before = r.counts();
        r.vcx.simulate_resize(size(px(700.), px(900.)));
        draw(r.vcx);
        let page = r.page.clone();
        assert!(r.vcx.update(|_, cx| page.read(cx).placement().stacked));
        let after = r.counts();
        assert_eq!(after[1] - before[1], 1, "one relayout notify");
        // The resize frame renders the page and the relayout renders it once more. A box whose
        // fit (rows, sections, side by side) changed with its new size notifies once, which
        // GPUI's ancestor marking turns into one more page render (crw.34).
        let renders = after[0] - before[0];
        assert!((2..=3).contains(&renders), "page renders {renders}");
        // Same size again: nothing.
        r.vcx.simulate_resize(size(px(700.), px(900.)));
        draw(r.vcx);
        assert_eq!(r.counts()[1], after[1]);
    }

    #[gpui_kit::test]
    fn overview_hidden_window_drops_demand(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.set_active(true);
        assert!(r.merged().cpu_fast);
        r.engine
            .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
        r.vcx.run_until_parked();
        r.vcx.executor().advance_clock(Duration::from_millis(1600));
        r.vcx.run_until_parked();
        let hub = r.hub.clone();
        assert!(r.vcx.read_entity(&hub, |h, _| h.is_hidden()));
        assert_eq!(
            r.merged(),
            Demand {
                device_history: true,
                ..Demand::default()
            }
        );
    }

    fn center(b: gpui_kit::Bounds<gpui_kit::Pixels>) -> gpui_kit::Point<gpui_kit::Pixels> {
        point(b.left() + b.size.width / 2., b.top() + b.size.height / 2.)
    }

    impl Rig<'_> {
        fn gap(&mut self, id: &'static str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
            self.vcx.debug_bounds(id).expect(id)
        }

        fn layout(&mut self) -> OverviewLayout {
            let page = self.page.clone();
            self.vcx.update(|_, cx| page.read(cx).layout().clone())
        }

        fn saved(&mut self) -> Option<tl_model::settings::OverviewLayoutSetting> {
            self.vcx
                .update(|_, cx| TlSettings::settings(cx).performance.overview.clone())
        }

        /// Sets `performance.overview_row_unit_px`: on the grid the box heights follow the row
        /// unit, not the window height (UI-PRF-056).
        fn set_row_unit(&mut self, unit: u32) {
            self.vcx.update(|_, cx| {
                TlSettings::update(cx, |d| d.settings.performance.overview_row_unit_px = unit);
            });
            draw(self.vcx);
        }

        fn press(&mut self, at: gpui_kit::Point<gpui_kit::Pixels>, click_count: usize) {
            self.vcx.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::none(),
                click_count,
                first_mouse: false,
            });
            draw(self.vcx);
        }

        fn move_to(&mut self, at: gpui_kit::Point<gpui_kit::Pixels>) {
            self.vcx
                .simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::none());
            draw(self.vcx);
        }

        fn release(&mut self, at: gpui_kit::Point<gpui_kit::Pixels>) {
            self.vcx.simulate_event(MouseUpEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
            });
            draw(self.vcx);
        }
    }

    /// (col, row, cols, rows) of `kind` in the page's layout.
    fn cells(r: &mut Rig, kind: BoxKind) -> (u16, u16, u16, u16) {
        let b = r.layout().get(kind).expect("placed");
        (b.col, b.row, b.cols, b.rows)
    }

    /// UI-PRF-056: a gap between two boxes that share a whole edge moves that edge in whole
    /// cells, clamped at both kind minimums; double-click splits the spans evenly.
    #[gpui_kit::test]
    fn overview_grid_gap_drag_moves_shared_edge(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let start = center(r.gap("perf-overview-splitter-col-cpu"));
        r.press(start, 1);
        // Under half a cell (97.8 px with the gutter) nothing moves.
        r.move_to(point(start.x - px(40.), start.y));
        assert_eq!(cells(&mut r, BoxKind::Cpu), (0, 0, 8, 7));
        r.move_to(point(start.x - px(100.), start.y));
        assert_eq!(cells(&mut r, BoxKind::Cpu), (0, 0, 7, 7));
        assert_eq!(cells(&mut r, BoxKind::Memory), (7, 0, 5, 7));
        for kind in [BoxKind::Gpu, BoxKind::Disk, BoxKind::Net] {
            assert_eq!(
                r.layout().get(kind),
                OverviewLayout::grid_default().get(kind),
                "{kind:?} stays"
            );
        }
        // The boxes follow at once, on the grid.
        let cpu = r.vcx.debug_bounds(box_id(BoxKind::Cpu)).expect("cpu");
        let mem = r.vcx.debug_bounds(box_id(BoxKind::Memory)).expect("memory");
        assert_eq!(cpu.size.width, px(679.));
        assert_eq!(mem.left() - cpu.right(), px(6.));
        // A drag past the minimum stops at it: CPU keeps 4 columns at 1200 px, Memory 3.
        r.move_to(point(px(0.), start.y));
        assert_eq!(cells(&mut r, BoxKind::Cpu), (0, 0, 4, 7));
        r.move_to(point(px(5000.), start.y));
        assert_eq!(cells(&mut r, BoxKind::Memory), (9, 0, 3, 7));
        r.release(point(px(5000.), start.y));

        // A double-click splits the 12 columns as evenly as whole cells allow, and saves at
        // once.
        let at = center(r.gap("perf-overview-splitter-col-cpu"));
        r.press(at, 2);
        r.release(at);
        assert_eq!(cells(&mut r, BoxKind::Cpu), (0, 0, 6, 7));
        assert_eq!(cells(&mut r, BoxKind::Memory), (6, 0, 6, 7));
        assert_eq!(
            OverviewLayout::from_setting(r.saved().as_ref()),
            r.layout(),
            "saved at once"
        );
        assert_eq!(r.layout().validate(), Ok(()));
        // Memory no longer shares a whole edge with Net, and CPU never did with GPU: no row
        // gaps.
        for id in [
            "perf-overview-splitter-row-memory",
            "perf-overview-splitter-row-cpu",
        ] {
            assert!(r.vcx.debug_bounds(id).is_none(), "{id}");
        }
    }

    #[gpui_kit::test]
    fn overview_layout_saved_after_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        assert_eq!(r.saved(), None);
        let start = center(r.gap("perf-overview-splitter-row-memory"));
        r.press(start, 1);
        r.move_to(point(start.x, start.y - px(30.)));
        r.move_to(point(start.x, start.y - px(46.)));
        assert_eq!(r.saved(), None, "nothing is saved during the drag");
        assert_eq!(cells(&mut r, BoxKind::Memory), (8, 0, 4, 6));
        r.release(point(start.x, start.y - px(46.)));
        let saved = r.saved().expect("saved when the drag ends");
        assert_eq!(OverviewLayout::from_setting(Some(&saved)), r.layout());
        // The grid is written, with whole cells.
        let net = saved.grid.iter().find(|e| e.kind == "net").expect("net");
        assert_eq!((net.col, net.row, net.cols, net.rows), (8, 6, 4, 7));
        assert!(saved.rows.is_empty());
        // A click that moves nothing saves nothing more: the settings stay as they are.
        let before = saved;
        let again = center(r.gap("perf-overview-splitter-row-memory"));
        r.press(again, 1);
        r.release(again);
        assert_eq!(r.saved(), Some(before));
    }

    fn cpu_split_of(setting: &Option<tl_model::settings::OverviewLayoutSetting>) -> Option<f64> {
        setting.as_ref().and_then(|s| s.grid[0].split)
    }

    /// The layout's CPU split.
    fn cpu_split(r: &mut Rig) -> f32 {
        r.layout()
            .split(BoxKind::Cpu)
            .expect("chart_cores has a split")
    }

    #[gpui_kit::test]
    fn overview_cpu_split_drag_moves_split(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let start = center(r.gap(CPU_SPLITTER_ID));
        let before = r.gap(CPU_SPLITTER_ID).left();
        r.press(start, 1);
        r.move_to(point(start.x + px(60.), start.y));
        let moved = f32::from(r.gap(CPU_SPLITTER_ID).left() - before);
        assert!((moved - 60.).abs() <= 1.5, "gap moved {moved}");
        assert_eq!(r.saved(), None, "nothing is saved during the drag");
        r.release(point(start.x + px(60.), start.y));
        let split = cpu_split(&mut r);
        assert!(split > 0.4 && split <= 0.8, "{split}");
        let saved = r.saved();
        assert_eq!(cpu_split_of(&saved), Some(f64::from(split)));
        assert_eq!(OverviewLayout::from_setting(saved.as_ref()), r.layout());
        // The boxes keep their cells.
        assert_eq!(r.layout().boxes, OverviewLayout::grid_default().boxes);

        // A drag far to the right stops where the core rows keep a 188 px column.
        let at = center(r.gap(CPU_SPLITTER_ID));
        r.press(at, 1);
        r.move_to(point(px(5000.), at.y));
        r.release(point(px(5000.), at.y));
        let frame = r.vcx.debug_bounds(CPU_BOX_ID).expect("CPU box");
        let gap = r.gap(CPU_SPLITTER_ID);
        let rows_w = f32::from(frame.right() - gap.right());
        assert!(rows_w >= 188., "core rows {rows_w}");
        assert!(cpu_split(&mut r) <= 0.8);
    }

    #[gpui_kit::test]
    fn overview_cpu_split_double_click_resets(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let start = center(r.gap(CPU_SPLITTER_ID));
        r.press(start, 1);
        r.move_to(point(start.x - px(80.), start.y));
        r.release(point(start.x - px(80.), start.y));
        assert!(cpu_split(&mut r) < 0.4);
        assert!(cpu_split_of(&r.saved()).is_some());
        let at = center(r.gap(CPU_SPLITTER_ID));
        r.press(at, 2);
        r.release(at);
        assert_eq!(cpu_split(&mut r), 0.4);
        assert_eq!(cpu_split_of(&r.saved()), None, "the default is not written");
        // The gap is back where it started.
        let back = center(r.gap(CPU_SPLITTER_ID));
        assert!((f32::from(back.x - start.x)).abs() <= 1.5);
    }

    #[gpui_kit::test]
    fn overview_cpu_split_saved_and_clamped(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let start = center(r.gap(CPU_SPLITTER_ID));
        r.press(start, 1);
        r.move_to(point(px(-5000.), start.y));
        r.release(point(px(-5000.), start.y));
        assert_eq!(cpu_split(&mut r), 0.2, "clamped at 20%");
        assert_eq!(cpu_split_of(&r.saved()), Some(0.2_f32 as f64));
        // A saved value reloads into the layout and the gap.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                layout.grid[0].split = Some(0.6);
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert_eq!(cpu_split(&mut r), 0.6_f32);
    }

    #[gpui_kit::test]
    fn overview_cpu_modes_render(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::parts::core_spark_id;
        let mut r = open_rooted(cx, true, 1200., 900.);
        let set = |r: &mut Rig, mode: &str| {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "cpu".into(),
                mode: mode.into(),
            });
        };
        let shown = |r: &mut Rig, id: &str| {
            let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
            r.vcx.debug_bounds(id).is_some()
        };
        let parts = |r: &mut Rig| {
            [
                shown(r, CPU_SPLITTER_ID),
                shown(r, &core_spark_id(0)),
                shown(r, CORE_CHARTS_ID),
            ]
        };

        // The default: total chart and core rows with the splitter.
        assert_eq!(parts(&mut r), [true, true, false]);
        assert!(r.layout().split(BoxKind::Cpu).is_some());

        // The total chart only: no rows, no splitter, no split.
        set(&mut r, "chart");
        assert_eq!(parts(&mut r), [false, false, false]);
        assert_eq!(r.layout().split(BoxKind::Cpu), None);
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());

        // Core rows only.
        set(&mut r, "cores");
        assert_eq!(parts(&mut r), [false, true, false]);
        let frame = r.bounds_of(CPU_BOX_ID.into());
        let spark = r.bounds_of(core_spark_id(0));
        assert!(spark.left() >= frame.left() && spark.right() <= frame.right());

        // One chart per logical CPU in a grid of cells of at least 120 × 60 px.
        set(&mut r, "core_charts");
        assert_eq!(parts(&mut r), [false, false, true]);
        let grid = r.bounds_of(CORE_CHARTS_ID.into());
        let first = r.bounds_of(core_chart_id(0));
        let second = r.bounds_of(core_chart_id(1));
        assert!(first.size.width >= px(120.), "{first:?}");
        assert!(first.size.height >= px(60.), "{first:?}");
        assert!(first.left() >= grid.left() && first.top() >= grid.top());
        assert_eq!(first.top(), second.top(), "cells fill a row first");
        assert!(second.left() > first.right(), "{first:?} {second:?}");

        // Back to the default mode: the default split returns.
        set(&mut r, "chart_cores");
        assert_eq!(parts(&mut r), [true, true, false]);
        assert_eq!(r.layout().split(BoxKind::Cpu), Some(0.4));
    }

    /// The CPU box's saved `power` and `power_split`.
    fn saved_power(r: &mut Rig) -> (bool, Option<f64>) {
        let b = r.saved().map(|s| s.grid[0].clone());
        b.map_or((false, None), |b| (b.power, b.power_split))
    }

    /// D-028: the power draw strip sits at the bottom of the CPU box under the parts of every
    /// mode, with a horizontal splitter above it. A drag saves `power_split`, a double-click
    /// resets it, "Reset Box" turns the strip off, and without a readable RAPL zone nothing is
    /// drawn.
    #[gpui_kit::test]
    fn overview_cpu_power_strip(cx: &mut TestAppContext) {
        use tl_model::snapshot::RaplState;
        use tl_ui::pages::performance::overview::parts::core_spark_id;
        let mut r = open_rooted(cx, true, 1200., 900.);
        // A 456 px CPU box, so the strip takes its default 25% of the body.
        r.set_row_unit(60);
        r.edit_layout();
        let power = |r: &mut Rig, on: bool| {
            r.page_action(&PerfOverviewSetBoxPower {
                kind: "cpu".into(),
                on,
            });
        };
        let mode = |r: &mut Rig, mode: &str| {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "cpu".into(),
                mode: mode.into(),
            });
        };
        let strip = |r: &mut Rig| r.vcx.debug_bounds(POWER_STRIP_ID);

        // Off by default, and nothing is written.
        r.set_rapl(Some(RaplState::NoZone));
        assert!(strip(&mut r).is_none());
        assert_eq!(saved_power(&mut r), (false, None));
        power(&mut r, true);
        assert_eq!(saved_power(&mut r), (true, None));
        assert!(strip(&mut r).is_none(), "no RAPL zone draws nothing");
        r.set_rapl(Some(RaplState::Unreadable));
        assert!(strip(&mut r).is_none(), "an unreadable zone draws nothing");
        r.set_rapl(Some(RaplState::Readable));
        let frame = r.bounds_of(CPU_BOX_ID.into());

        // Chart and cores: full width at the bottom, the splitter between it and the parts.
        let bar = strip(&mut r).expect("the strip");
        assert!(bar.size.height >= px(64.), "{bar:?}");
        assert!(bar.left() >= frame.left() && bar.right() <= frame.right());
        assert!(bar.bottom() <= frame.bottom());
        let rows = r.bounds_of(core_spark_id(0));
        let gap = r.gap(POWER_SPLITTER_ID);
        assert!(rows.bottom() <= gap.top() && gap.bottom() <= bar.top() + px(1.));
        let wide = f32::from(bar.size.width);
        assert!(wide > 500., "full width {wide}");

        // The other modes: the strip stays under their parts.
        mode(&mut r, "cores");
        let bar = strip(&mut r).expect("the strip under the cores");
        assert!(r.bounds_of(core_spark_id(0)).bottom() <= bar.top());
        mode(&mut r, "core_charts");
        let bar = strip(&mut r).expect("the strip under the core charts");
        assert!(r.bounds_of(core_chart_id(0)).bottom() <= bar.top());
        mode(&mut r, "chart");
        let bar = strip(&mut r).expect("the strip under the chart");
        assert!(bar.top() > frame.top() + px(100.));
        // A mode change keeps the strip on and its size.
        assert_eq!(saved_power(&mut r), (true, None));

        // A drag up grows the strip and saves `power_split` when it ends.
        let before = bar.size.height;
        let start = center(r.gap(POWER_SPLITTER_ID));
        r.press(start, 1);
        r.move_to(point(start.x, start.y - px(40.)));
        assert_eq!(
            saved_power(&mut r),
            (true, None),
            "nothing is saved mid-drag"
        );
        r.release(point(start.x, start.y - px(40.)));
        let grown = strip(&mut r).expect("the strip").size.height;
        assert!(grown > before + px(30.), "{before:?} -> {grown:?}");
        let (on, split) = saved_power(&mut r);
        let split = split.expect("power_split saved");
        assert!(
            on && (0.1..=0.5).contains(&split) && split > 0.25,
            "{split}"
        );
        let saved = r.saved();
        assert_eq!(OverviewLayout::from_setting(saved.as_ref()), r.layout());
        assert_eq!(r.layout().config(BoxKind::Cpu).power_split, split as f32);

        // A far drag stops at half the body; a far drag down keeps 64 px.
        let at = center(r.gap(POWER_SPLITTER_ID));
        r.press(at, 1);
        r.move_to(point(at.x, px(-5000.)));
        r.release(point(at.x, px(-5000.)));
        assert_eq!(saved_power(&mut r).1, Some(0.5));
        let at = center(r.gap(POWER_SPLITTER_ID));
        r.press(at, 1);
        r.move_to(point(at.x, px(5000.)));
        r.release(point(at.x, px(5000.)));
        assert!(strip(&mut r).expect("the strip").size.height >= px(64.));
        assert!(saved_power(&mut r).1.is_some_and(|v| v >= 0.1));

        // A double-click on the gap resets the share and writes nothing for the default.
        let at = center(r.gap(POWER_SPLITTER_ID));
        r.press(at, 2);
        r.release(at);
        assert_eq!(saved_power(&mut r), (true, None));
        assert_eq!(r.layout().config(BoxKind::Cpu).power_split, 0.25);

        // Reset Box turns the strip off.
        r.page_action(&PerfOverviewResetBox { kind: "cpu".into() });
        assert!(strip(&mut r).is_none());
        assert_eq!(saved_power(&mut r), (false, None));
        // Switching on and off again.
        power(&mut r, true);
        assert!(strip(&mut r).is_some());
        power(&mut r, false);
        assert!(strip(&mut r).is_none());
        assert_eq!(saved_power(&mut r), (false, None));
        // The strip is the CPU box's: another kind ignores the action.
        r.page_action(&PerfOverviewSetBoxPower {
            kind: "memory".into(),
            on: true,
        });
        assert_eq!(r.saved().map(|s| s.grid[1].power), Some(false));
    }

    #[gpui_kit::test]
    fn overview_memory_modes_render(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::mem_box::{
            MEM_BOX_ID, MEM_CHART_SPLITTER_ID, MEM_COMP_CHART_ID, MEM_SPLITTER_ID,
            MEM_USED_CHART_ID, spark_id,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        let set = |r: &mut Rig, mode: &str| {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "memory".into(),
                mode: mode.into(),
            });
        };
        let shown = |r: &mut Rig, id: &str| {
            let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
            r.vcx.debug_bounds(id).is_some()
        };
        let parts = |r: &mut Rig| {
            [
                shown(r, MEM_SPLITTER_ID),
                shown(r, &spark_id(0)),
                shown(r, MEM_USED_CHART_ID),
                shown(r, MEM_CHART_SPLITTER_ID),
                shown(r, MEM_COMP_CHART_ID),
            ]
        };

        // The default: text and meter part, splitter and a 96 px sparkline column.
        assert_eq!(parts(&mut r), [true, true, false, false, false]);
        assert_eq!(r.layout().split(BoxKind::Memory), Some(0.7));
        let frame = r.bounds_of(MEM_BOX_ID.into());
        let spark = r.bounds_of(spark_id(0));
        assert!((f32::from(spark.size.width) - 96.).abs() < 1., "{spark:?}");
        assert!(spark.right() <= frame.right());

        // The used chart over the rows, with a horizontal splitter and no sparklines.
        set(&mut r, "chart_rows");
        assert_eq!(parts(&mut r), [false, false, true, true, false]);
        assert_eq!(r.layout().split(BoxKind::Memory), Some(0.5));
        let chart = r.bounds_of(MEM_USED_CHART_ID.into());
        let gap = r.bounds_of(MEM_CHART_SPLITTER_ID.into());
        assert!(chart.bottom() <= gap.top() + px(1.), "{chart:?} {gap:?}");
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());

        // The composition chart alone: no rows, no splitter, no split.
        set(&mut r, "chart");
        assert_eq!(parts(&mut r), [false, false, false, false, true]);
        assert_eq!(r.layout().split(BoxKind::Memory), None);
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());

        // Back to the default mode: the default split returns.
        set(&mut r, "rows");
        assert_eq!(parts(&mut r), [true, true, false, false, false]);
        assert_eq!(r.layout().split(BoxKind::Memory), Some(0.7));
    }

    /// UI-PRF-060: "By app" shows the memory-by-app chart in its Overview place, saves the mode,
    /// and the page asks for the process sweep only while the box shows it.
    #[gpui_kit::test]
    fn overview_memory_by_app_mode(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::memory_apps::{APPS_ID, AppsPlace};
        use tl_ui::pages::performance::overview::mem_box::MEM_APPS_ID;
        let mut r = open_rooted(cx, true, 1200., 900.);
        let page = r.page.clone();
        let sweep = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| page.read(cx).demand_part().process_top)
        };
        assert!(!sweep(&mut r));
        let menu = r.vcx.update(|_, cx| {
            BoxKind::Memory
                .modes()
                .iter()
                .any(|m| m.id == "apps" && tl_ui::i18n::tr(cx, m.label).as_ref() == "By app")
        });
        assert!(menu, "the Layout submenu lists By app");

        r.page_action(&PerfOverviewSetBoxMode {
            kind: "memory".into(),
            mode: "apps".into(),
        });
        assert!(r.vcx.debug_bounds(MEM_APPS_ID).is_some());
        assert!(r.vcx.debug_bounds(APPS_ID).is_some());
        let place = r.vcx.update(|_, cx| {
            let mem = page.read(cx).mem_box().read(cx);
            mem.apps().map(|a| a.read(cx).place())
        });
        assert_eq!(place, Some(AppsPlace::Overview));
        assert!(sweep(&mut r));
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());
        assert_eq!(r.layout().config(BoxKind::Memory).mode.id, "apps");

        r.page_action(&PerfOverviewSetBoxMode {
            kind: "memory".into(),
            mode: "rows".into(),
        });
        assert!(r.vcx.debug_bounds(APPS_ID).is_none());
        assert!(!sweep(&mut r));
    }

    #[gpui_kit::test]
    fn overview_mem_comp_chart_has_legend(cx: &mut TestAppContext) {
        use tl_ui::chart::labels::LabelKind;
        let mut r = open_rooted(cx, true, 1200., 900.);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "memory".into(),
            mode: "chart".into(),
        });
        let page = r.page.clone();
        let words: Vec<Option<String>> = r.vcx.update(|_, cx| {
            let chart = page.read(cx).mem_box().read(cx).comp_chart().clone();
            let chart = chart.read(cx);
            (0..3)
                .map(|i| {
                    chart
                        .label_cache()
                        .text(LabelKind::Legend(i))
                        .map(str::to_owned)
                })
                .collect()
        });
        let want = ["In use", "Cached", "Available"].map(|w| Some(w.to_owned()));
        assert_eq!(words, want);
    }

    #[gpui_kit::test]
    fn overview_gpu_modes_render(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::gpu_box::{
            GPU_BOX_ID, GPU_SPLITTER_ID, chart_id, engine_cell_id, engine_row_id,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        let page = r.page.clone();
        let keys = |r: &mut Rig| -> Vec<&'static str> {
            r.vcx.update(|_, cx| {
                page.read(cx).gpu_box().read(cx).sections()[0]
                    .engines
                    .iter()
                    .map(|e| e.key)
                    .collect()
            })
        };
        let set = |r: &mut Rig, mode: &str| {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "gpu".into(),
                mode: mode.into(),
            });
        };
        let shown = |r: &mut Rig, id: String| {
            let id: &'static str = Box::leak(id.into_boxed_str());
            r.vcx.debug_bounds(id).is_some()
        };
        let engines = keys(&mut r);
        assert!(engines.len() >= 2, "{engines:?}");
        let parts = |r: &mut Rig| {
            [
                shown(r, GPU_SPLITTER_ID.into()),
                shown(r, chart_id(0)),
                shown(r, engine_row_id(0, engines[0])),
                shown(r, engine_cell_id(0, engines[0])),
            ]
        };

        // The default since D-026: one chart per engine, no rows, no splitter, no split.
        assert_eq!(parts(&mut r), [false, false, false, true]);
        assert_eq!(r.layout().split(BoxKind::Gpu), None);
        let frame = r.bounds_of(GPU_BOX_ID.into());
        for k in &engines {
            let cell = r.bounds_of(engine_cell_id(0, k));
            assert!(
                cell.size.width >= px(120.) && cell.size.height >= px(48.),
                "{cell:?}"
            );
            assert!(cell.right() <= frame.right() && cell.bottom() <= frame.bottom());
        }

        // Chart, engine rows and the splitter under the chart.
        set(&mut r, "chart_engines");
        assert_eq!(parts(&mut r), [true, true, true, false]);
        assert_eq!(r.layout().split(BoxKind::Gpu), Some(0.5));
        let chart = r.bounds_of(chart_id(0));
        let gap = r.bounds_of(GPU_SPLITTER_ID.into());
        assert!(chart.bottom() <= gap.bottom(), "{chart:?} {gap:?}");
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());

        // The utilization chart and the VRAM row only.
        set(&mut r, "chart");
        assert_eq!(parts(&mut r), [false, true, false, false]);
        assert_eq!(r.layout().split(BoxKind::Gpu), None);
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());

        // Back to the default mode: no split.
        set(&mut r, "engine_charts");
        assert_eq!(parts(&mut r), [false, false, false, true]);
        assert_eq!(r.layout().split(BoxKind::Gpu), None);
    }

    #[gpui_kit::test]
    fn overview_gpu_engine_charts_one_per_engine(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::gpu_box::engine_cell_id;
        let mut r = open_rooted(cx, true, 1200., 900.);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "gpu".into(),
            mode: "engine_charts".into(),
        });
        let page = r.page.clone();
        let keys: Vec<&'static str> = r.vcx.update(|_, cx| {
            page.read(cx).gpu_box().read(cx).sections()[0]
                .engines
                .iter()
                .map(|e| e.key)
                .collect()
        });
        assert!(keys.len() >= 2, "{keys:?}");
        let mut cells = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            let id: &'static str = Box::leak(engine_cell_id(0, k).into_boxed_str());
            let b = r.vcx.debug_bounds(id);
            assert!(b.is_some(), "no cell for engine {i} {k}");
            cells.push(b.unwrap());
        }
        // Distinct cells: no two share a place.
        for (i, a) in cells.iter().enumerate() {
            for b in &cells[i + 1..] {
                assert!(a.origin != b.origin, "{a:?} {b:?}");
            }
        }
        // No cell for a key that is not shown.
        let id: &'static str = Box::leak(engine_cell_id(0, "nope").into_boxed_str());
        assert!(r.vcx.debug_bounds(id).is_none());
    }

    /// D-029: the GPU box's Engines submenu leaves engine charts and VRAM out, the last shown item
    /// cannot be hidden, it saves as `hidden`, and Reset Box shows all.
    #[gpui_kit::test]
    fn overview_gpu_engine_choice(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::PerfOverviewSetBoxEngine;
        use tl_ui::pages::performance::overview::gpu_box::{engine_cell_id, vram_id};
        for (w, h) in [(1920., 1200.), (1280., 800.)] {
            let mut r = open_rooted(cx, true, w, h);
            let page = r.page.clone();
            let names: Vec<String> = r.vcx.update(|_, cx| {
                page.read(cx)
                    .gpu_box()
                    .read(cx)
                    .engine_names(cx)
                    .iter()
                    .map(|n| n.to_string())
                    .collect()
            });
            assert!(
                names.len() >= 3 && names.last().unwrap() == "VRAM",
                "{names:?}"
            );
            let keys: Vec<&'static str> = r.vcx.update(|_, cx| {
                page.read(cx).gpu_box().read(cx).sections()[0]
                    .engines
                    .iter()
                    .map(|e| e.key)
                    .collect()
            });
            let cell = |r: &mut Rig, k: &str| {
                let id: &'static str = Box::leak(engine_cell_id(0, k).into_boxed_str());
                r.vcx.debug_bounds(id).is_some()
            };
            let vram = |r: &mut Rig| {
                let id: &'static str = Box::leak(vram_id(0).into_boxed_str());
                r.vcx.debug_bounds(id).is_some()
            };
            let act = |r: &mut Rig, name: &str, shown: bool| {
                r.page_action(&PerfOverviewSetBoxEngine {
                    kind: "gpu".into(),
                    engine: name.into(),
                    shown,
                });
            };
            assert!(keys.iter().all(|k| cell(&mut r, k)) && vram(&mut r));

            // Hide the first engine: its cell goes, the others stay, and it saves.
            act(&mut r, &names[0], false);
            assert_eq!(r.layout().hidden_gpu(), [names[0].clone()]);
            assert!(!cell(&mut r, keys[0]), "{w}");
            assert!(keys[1..].iter().all(|k| cell(&mut r, k)), "{w}");
            assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());

            // Hide VRAM: the row goes.
            act(&mut r, "VRAM", false);
            assert!(!vram(&mut r), "{w}");
            assert!(keys[1..].iter().all(|k| cell(&mut r, k)), "{w}");

            // Hide all but one: the last shown item stays.
            for n in &names[1..names.len() - 1] {
                act(&mut r, n, false);
            }
            let last = &names[names.len() - 2];
            assert!(!r.layout().hidden_gpu().contains(last));
            act(&mut r, last, false);
            assert!(!r.layout().hidden_gpu().contains(last));
            assert!(cell(&mut r, keys[keys.len() - 1]), "{w}");

            // Reset Box shows all.
            r.page_action(&PerfOverviewResetBox { kind: "gpu".into() });
            assert!(r.layout().hidden_gpu().is_empty());
            assert!(keys.iter().all(|k| cell(&mut r, k)) && vram(&mut r), "{w}");
            assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        }
    }

    /// D-026: a layout with no saved mode shows the GPU box as one chart per engine; a saved
    /// mode still wins.
    #[gpui_kit::test]
    fn overview_defaults_d026(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::gpu_box::{engine_cell_id, engine_row_id};
        let mut r = open(cx, true, 1200., 900.);
        let page = r.page.clone();
        let mode = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| page.read(cx).gpu_box().read(cx).mode())
        };
        assert_eq!(r.layout().config(BoxKind::Gpu).mode.id, "engine_charts");
        assert_eq!(mode(&mut r), "engine_charts");
        assert!(
            r.vcx
                .debug_bounds(Box::leak(engine_cell_id(0, "gpu").into_boxed_str()))
                .is_some()
        );
        assert_eq!(r.saved(), None, "nothing is written for the default");

        // Net: stacked, Download above Upload, each the full box width.
        assert_eq!(r.layout().config(BoxKind::Net).mode.id, "stacked");
        let net_mode = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| page.read(cx).net_box().read(cx).mode())
        };
        assert_eq!(net_mode(&mut r), "stacked");

        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                let net = layout
                    .grid
                    .iter_mut()
                    .find(|b| b.kind == "net")
                    .expect("a Net box");
                net.mode = Some("columns".to_owned());
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert_eq!(net_mode(&mut r), "columns", "a saved mode wins");

        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                let gpu = layout
                    .grid
                    .iter_mut()
                    .find(|b| b.kind == "gpu")
                    .expect("a GPU box");
                gpu.mode = Some("chart_engines".to_owned());
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert_eq!(mode(&mut r), "chart_engines");
        assert!(
            r.vcx
                .debug_bounds(Box::leak(engine_row_id(0, "gpu").into_boxed_str()))
                .is_some()
        );
    }

    /// crw.73: in `chart_engines` a drag far down stops where every engine row and the VRAM row
    /// still end inside the body.
    #[gpui_kit::test]
    fn overview_gpu_chart_engines_drag_keeps_rows(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::gpu_box::{
            GPU_BOX_ID, GPU_SPLITTER_ID, engine_row_id, rows_id,
        };
        use tl_ui::pages::performance::overview::parts::body_id;
        let mut r = open_rooted(cx, true, 1280., 800.);
        r.edit_layout();
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "gpu".into(),
            mode: "chart_engines".into(),
        });
        let at = center(r.gap(GPU_SPLITTER_ID));
        r.press(at, 1);
        r.move_to(point(at.x, px(5000.)));
        r.release(point(at.x, px(5000.)));
        draw(r.vcx);
        let axis = r.bounds_of(body_id(GPU_BOX_ID));
        let rows = r.bounds_of(rows_id(0));
        assert!(rows.bottom() <= axis.bottom(), "{rows:?} above {axis:?}");
        for key in ["gpu", "enc", "dec"] {
            let row = r.bounds_of(engine_row_id(0, key));
            assert!(
                row.bottom() <= axis.bottom(),
                "{key} {row:?} above {axis:?}"
            );
        }
        let split = r.layout().split(BoxKind::Gpu).expect("a split");
        assert!(split > 0.5, "the drag moved the split: {split}");
        // A drag back up works too.
        let at = center(r.gap(GPU_SPLITTER_ID));
        r.press(at, 1);
        r.move_to(point(at.x, px(-5000.)));
        r.release(point(at.x, px(-5000.)));
        assert!(r.layout().split(BoxKind::Gpu).expect("a split") < split);
    }

    #[gpui_kit::test]
    fn overview_disk_modes_render(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::{
            CELL_MIN_H, CELL_MIN_W, DISK_BOX_ID, DISK_SPLITTER_ID, TOTAL_CHART_ID, cell_id,
            io_row_id,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        let page = r.page.clone();
        let key = r.vcx.update(|_, cx| {
            let rows = page.read(cx).disk_box().read(cx).rows().to_vec();
            assert!(!rows.is_empty());
            rows[0].key.clone()
        });
        let set = |r: &mut Rig, mode: &str| {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "disk".into(),
                mode: mode.into(),
            });
        };
        let shown = |r: &mut Rig, id: String| {
            let id: &'static str = Box::leak(id.into_boxed_str());
            r.vcx.debug_bounds(id).is_some()
        };
        let parts = |r: &mut Rig| {
            [
                shown(r, DISK_SPLITTER_ID.into()),
                shown(r, io_row_id(&key, "read")),
                shown(r, io_row_id(&key, "read-spark")),
                shown(r, cell_id(&key)),
                shown(r, TOTAL_CHART_ID.into()),
            ]
        };

        // The default (D-026): one read and write chart per disk, no splitter, no split.
        assert_eq!(parts(&mut r), [false, false, false, true, false]);
        assert_eq!(r.layout().split(BoxKind::Disk), None);

        // The sections: label part, splitter and sparkline part, level with each other.
        set(&mut r, "rows");
        assert_eq!(parts(&mut r), [true, true, true, false, false]);
        assert_eq!(r.layout().split(BoxKind::Disk), Some(0.4));
        let text = r.bounds_of(io_row_id(&key, "read"));
        let spark = r.bounds_of(io_row_id(&key, "read-spark"));
        let gap = r.bounds_of(DISK_SPLITTER_ID.into());
        assert!(text.right() <= gap.left() + px(1.), "{text:?} {gap:?}");
        assert!(spark.left() >= gap.right() - px(1.), "{spark:?} {gap:?}");
        assert_eq!(text.origin.y, spark.origin.y);

        // One read and write chart per disk in a grid, no splitter, no split.
        set(&mut r, "disk_charts");
        let frame = r.bounds_of(DISK_BOX_ID.into());
        assert_eq!(parts(&mut r), [false, false, false, true, false]);
        assert_eq!(r.layout().split(BoxKind::Disk), None);
        let cell = r.bounds_of(cell_id(&key));
        assert!(
            cell.size.width >= px(CELL_MIN_W) && cell.size.height >= px(CELL_MIN_H),
            "{cell:?}"
        );
        assert!(cell.right() <= frame.right() && cell.bottom() <= frame.bottom());
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());

        // One chart of the summed rates.
        set(&mut r, "total_chart");
        assert_eq!(parts(&mut r), [false, false, false, false, true]);
        assert_eq!(r.layout().split(BoxKind::Disk), None);
        let total = r.bounds_of(TOTAL_CHART_ID.into());
        assert!(total.right() <= frame.right() && total.bottom() <= frame.bottom());
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());

        // Back to the default mode: no split.
        set(&mut r, "disk_charts");
        assert_eq!(parts(&mut r), [false, false, false, true, false]);
        assert_eq!(r.layout().split(BoxKind::Disk), None);
    }

    /// D-026: a new layout shows the Disk box as one chart per disk, and a saved mode still wins.
    #[gpui_kit::test]
    fn overview_disk_defaults_d026(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::{DISK_SPLITTER_ID, GRID_ID, cell_id};
        let mut r = open_rooted(cx, true, 1200., 900.);
        let page = r.page.clone();
        let mode = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| page.read(cx).disk_box().read(cx).mode())
        };
        assert_eq!(r.layout().config(BoxKind::Disk).mode.id, "disk_charts");
        assert_eq!(mode(&mut r), "disk_charts");
        assert!(r.vcx.debug_bounds(GRID_ID).is_some());
        assert_eq!(r.saved(), None, "nothing is written for the default");

        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                let disk = layout
                    .grid
                    .iter_mut()
                    .find(|b| b.kind == "disk")
                    .expect("a Disk box");
                disk.mode = Some("rows".to_owned());
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert_eq!(mode(&mut r), "rows");
        assert!(r.vcx.debug_bounds(DISK_SPLITTER_ID).is_some());
        let key = r
            .vcx
            .update(|_, cx| page.read(cx).disk_box().read(cx).rows()[0].key.clone());
        assert!(
            r.vcx
                .debug_bounds(Box::leak(cell_id(&key).into_boxed_str()))
                .is_none()
        );
    }

    /// D-026: the `disk_charts` cells are equal, at least 160 × 60 px, fill the box, and the
    /// grid scrolls when they do not fit.
    #[gpui_kit::test]
    fn overview_disk_grid_cells_fill_box(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::{
            CELL_MIN_H, CELL_MIN_W, GRID_ID, cell_id,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        set_disk_arrange(&mut r, "auto");
        let page = r.page.clone();
        let keys: Vec<_> = r.vcx.update(|_, cx| {
            page.read(cx)
                .disk_box()
                .read(cx)
                .rows()
                .iter()
                .map(|d| d.key.clone())
                .collect()
        });
        assert!(keys.len() >= 2, "{} disks", keys.len());
        let cells = |r: &mut Rig| {
            keys.iter()
                .map(|k| r.bounds_of(cell_id(k)))
                .collect::<Vec<_>>()
        };
        let grid = r.bounds_of(GRID_ID.into());
        let cs = cells(&mut r);
        for c in &cs {
            assert_eq!(c.size, cs[0].size, "equal cells");
            assert!(c.size.width >= px(CELL_MIN_W) && c.size.height >= px(CELL_MIN_H));
            assert!(c.right() <= grid.right() && c.bottom() <= grid.bottom());
        }
        // The cells fill the grid: the last row ends within one cell gap of its bottom, or the
        // width is used by a single column.
        let last = cs.iter().map(|c| c.bottom()).max().expect("cells");
        let right = cs.iter().map(|c| c.right()).max().expect("cells");
        assert!(
            grid.bottom() - last < px(CELL_MIN_H) && grid.right() - right < px(CELL_MIN_W),
            "{cs:?} in {grid:?}"
        );
        // Taller than the minimum: the cells grow with the box.
        assert!(cs[0].size.height > px(CELL_MIN_H), "{:?}", cs[0]);
    }

    /// Adds `extra` disks, sdc onward from index 6, to the catalog and returns the Disk box's
    /// disk keys.
    fn more_disks(r: &mut Rig, extra: u16) -> Vec<DeviceKey> {
        let mut cat = catalog(true);
        let mut entries = cat.entries.to_vec();
        for i in 0..extra {
            entries.push(entry(
                6 + i,
                DeviceIdentity::Disk {
                    major: 8,
                    minor: u32::from(6 + i),
                    name: Arc::from(format!("sd{}", (b'c' + i as u8) as char)),
                    wwn_or_serial: None,
                },
                None,
            ));
        }
        cat.entries = Arc::from(entries);
        // Past the published version, so a second call is seen as a new catalog too.
        r.engine.publish_with(|s| {
            cat.version = cat.version.max(s.devices.version) + 1;
            s.devices = Arc::new(cat);
        });
        r.vcx.run_until_parked();
        draw(r.vcx);
        let page = r.page.clone();
        let keys: Vec<_> = r.vcx.update(|_, cx| {
            page.read(cx)
                .disk_box()
                .read(cx)
                .rows()
                .iter()
                .map(|d| d.key.clone())
                .collect()
        });
        assert_eq!(keys.len(), 2 + usize::from(extra), "{keys:?}");
        keys
    }

    /// Adds a third disk, sdc at 6, to the catalog and returns the Disk box's disk keys.
    fn three_disks(r: &mut Rig) -> Vec<DeviceKey> {
        more_disks(r, 1)
    }

    /// How many `disk_charts` cells the Disk box shows; the rest are in "+n more".
    fn shown_disks(r: &mut Rig) -> usize {
        let page = r.page.clone();
        r.vcx
            .update(|_, cx| page.read(cx).disk_box().read(cx).shown())
    }

    /// Places the Disk box's `disk_charts` cells as `arrange` says, as the Arrange items do.
    fn set_disk_arrange(r: &mut Rig, arrange: &str) {
        r.page_action(&PerfOverviewSetBoxArrange {
            kind: "disk".into(),
            arrange: arrange.into(),
        });
    }

    /// D-026: with three disks at 1200 × 900 the Disk chart areas (each cell's chart right below
    /// its header, measured) are about as tall as the GPU engine chart areas in the same row,
    /// where the old cells were the 18 px rows. None is taller than the one Net adapter chart.
    #[gpui_kit::test]
    fn overview_disk_charts_as_tall_as_gpu(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::{DISK_BOX_ID, cell_id, chart_id};
        use tl_ui::pages::performance::overview::gpu_box::{
            GPU_BOX_ID, engine_cell_id, engine_chart_id,
        };
        use tl_ui::pages::performance::overview::net_box::{
            cell_id as net_cell_id, chart_id as net_chart_id,
        };
        /// The header line of a cell: `PLOT_LINE` px at the test's `ui_scale` of 1.
        const HEAD_LINE: f32 = 16.;
        let mut r = open_rooted(cx, true, 1200., 900.);
        let keys = three_disks(&mut r);
        set_disk_arrange(&mut r, "auto");
        let page = r.page.clone();
        let engines: Vec<&'static str> = r.vcx.update(|_, cx| {
            page.read(cx).gpu_box().read(cx).sections()[0]
                .engines
                .iter()
                .map(|e| e.key)
                .collect()
        });
        assert!(!engines.is_empty());
        // The GPU and Disk boxes share the second band at the same span: the same height, and
        // widths within the 1 px of edge rounding.
        let (gpu, disk) = (
            r.bounds_of(GPU_BOX_ID.into()),
            r.bounds_of(DISK_BOX_ID.into()),
        );
        assert_eq!(gpu.size.height, disk.size.height);
        assert!(
            (gpu.size.width - disk.size.width).abs() <= px(1.),
            "{gpu:?} {disk:?}"
        );
        // A chart area sits at the bottom of its cell, below the header.
        let chart_in = |chart: gpui_kit::Bounds<gpui_kit::Pixels>,
                        cell: gpui_kit::Bounds<gpui_kit::Pixels>,
                        what: &str| {
            assert!(
                chart.left() >= cell.left()
                    && chart.right() <= cell.right()
                    && (f32::from(chart.bottom()) - f32::from(cell.bottom())).abs() < 0.5
                    && chart.top() >= cell.top() + px(HEAD_LINE),
                "{what} chart {chart:?} in cell {cell:?}"
            );
            assert!(
                chart.size.height <= cell.size.height - px(HEAD_LINE),
                "{what} header"
            );
            f32::from(chart.size.height)
        };
        let gpu_chart = chart_in(
            r.bounds_of(engine_chart_id(0, engines[0])),
            r.bounds_of(engine_cell_id(0, engines[0])),
            "GPU",
        );
        assert!(gpu_chart > HEAD_LINE, "GPU chart {gpu_chart} px");
        let disk_charts: Vec<f32> = keys
            .iter()
            .map(|k| chart_in(r.bounds_of(chart_id(k)), r.bounds_of(cell_id(k)), "disk"))
            .collect();
        for &chart in &disk_charts {
            // Up to 1.75 times: with the 8 px above the box title (UI-PRF-059) the charts
            // measure 50 px (Disk) and 29 px (GPU).
            assert!(
                chart >= 0.6 * gpu_chart && chart <= 1.75 * gpu_chart,
                "disk chart {chart} px, GPU chart {gpu_chart} px with {} engines",
                engines.len()
            );
            // The chart is a real chart, not the 18 px row it replaced.
            assert!(chart > HEAD_LINE, "disk chart {chart} px");
        }
        // The Net adapter charts in the box beside them are about as tall too.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "net".into(),
            mode: "adapter_charts".into(),
        });
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let m = tl_model::system::Metric::valid;
            sys.nets = Arc::from([(
                tl_model::ids::DeviceIdx(5),
                tl_model::system::NetTotals {
                    device: tl_model::ids::DeviceIdx(5),
                    rx_bps: m(1000.),
                    tx_bps: m(100.),
                    rx_total: 1 << 30,
                    tx_total: 1 << 29,
                    link_speed_mbps: tl_model::system::Metric::default(),
                    state: tl_model::system::LinkState::Up,
                },
            )]);
        });
        r.vcx.run_until_parked();
        let page = r.page.clone();
        let idxs = r
            .vcx
            .update(|_, cx| page.read(cx).net_box().read(cx).adapter_idxs());
        assert!(!idxs.is_empty(), "the fixture has a counted adapter");
        // One adapter fills the whole Net box, so its chart is the tallest a cell can be there:
        // a Disk chart is never taller.
        let net_chart = chart_in(
            r.bounds_of(net_chart_id(idxs[0])),
            r.bounds_of(net_cell_id(idxs[0])),
            "Net",
        );
        for k in &keys {
            let chart = f32::from(r.bounds_of(chart_id(k)).size.height);
            assert!(
                chart > HEAD_LINE && chart <= net_chart,
                "disk chart {chart} px, Net chart {net_chart} px"
            );
        }
    }

    /// The labels of `kind`'s gear menu, and the label, mark and action of each item of its
    /// Arrange submenu (empty without one). Edit Layout mode must be on.
    #[allow(clippy::type_complexity, reason = "test helper")]
    fn gear_menu(
        r: &mut Rig,
        kind: BoxKind,
    ) -> (
        Vec<String>,
        Vec<(String, bool, Option<Box<dyn gpui_kit::Action>>)>,
    ) {
        let at = center(r.bounds_of(gear_id(kind)));
        r.press(at, 1);
        r.release(at);
        let label =
            |l: Option<gpui_kit::SharedString>| l.map(|l| l.to_string()).unwrap_or_default();
        r.vcx.update(|window, cx| {
            let menu = tl_ui::components::menu::open_context_menu(window, cx)
                .expect("the gear menu is open");
            let entries = menu.read(cx).entries_for_test(window);
            let arrange = entries
                .iter()
                .rev()
                .find(|e| e.label.as_deref() == Some("Arrange"))
                .and_then(|e| e.submenu.clone())
                .map(|sub| {
                    sub.read(cx)
                        .entries_for_test(window)
                        .into_iter()
                        .map(|e| (label(e.label), e.checked, e.action))
                        .collect()
                })
                .unwrap_or_default();
            let labels = entries.into_iter().map(|e| label(e.label)).collect();
            (labels, arrange)
        })
    }

    /// Runs the Arrange item `name` of `kind`'s gear menu on the page.
    fn pick_arrange(r: &mut Rig, kind: BoxKind, name: &str) {
        let (_, items) = gear_menu(r, kind);
        let action = items
            .into_iter()
            .find(|(l, _, _)| l == name)
            .and_then(|(_, _, a)| a)
            .unwrap_or_else(|| panic!("an Arrange item {name}"));
        r.page_action(&*action);
    }

    /// The Disk box's saved `arrange`.
    fn saved_disk_arrange(r: &mut Rig) -> Option<String> {
        r.saved().and_then(|s| {
            s.grid
                .into_iter()
                .find(|b| b.kind == "disk")
                .and_then(|b| b.arrange)
        })
    }

    /// D-027: the Disk box's `disk_charts` cells go in one column by default, in one row or in
    /// the mode's grid by the Arrange choice, each an equal share of the box. When the cells do
    /// not fit, only whole cells show and the rest join "+n more" (UI-PRF-059). The choice saves
    /// and reloads, a saved value wins over the default, and Reset Box gives the column back.
    /// The Net box's Arrange items switch between `stacked` and `columns`.
    #[gpui_kit::test]
    fn overview_arrange_vertical_horizontal_auto(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::{
            CELL_MIN_H, CELL_MIN_W, GRID_ID, cell_id,
        };
        use tl_ui::pages::performance::overview::parts::arrange_grid;
        let mut r = open_rooted(cx, true, 1800., 900.);
        assert_eq!(three_disks(&mut r).len(), 3);
        // A 42 px row unit, so the Disk box's grid has room for three 60 px cells and none
        // joins "+1 more".
        r.set_row_unit(42);
        let page = r.page.clone();
        let shown = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| page.read(cx).disk_box().read(cx).arrange())
        };
        // The cells shown, checked against the helper's fit for the measured grid.
        let cells = |r: &mut Rig, arrange: Arrange| {
            let grid = r.bounds_of(GRID_ID.into());
            let (n, keys): (usize, Vec<DeviceKey>) = r.vcx.update(|_, cx| {
                let disks = page.read(cx).disk_box().read(cx);
                let keys = disks.rows().iter().map(|d| d.key.clone()).collect();
                (disks.shown(), keys)
            });
            let min = size(px(CELL_MIN_W), px(CELL_MIN_H));
            let fit = arrange_grid(arrange, n, grid.size, min, px(2.), px(0.));
            let cs: Vec<_> = keys
                .iter()
                .take(n)
                .map(|k| r.bounds_of(cell_id(k)))
                .collect();
            for c in &cs {
                assert_eq!(c.size, fit.cell, "{arrange:?}: {c:?} in {grid:?}");
            }
            (grid, cs, fit)
        };
        let col = |cs: &[gpui_kit::Bounds<gpui_kit::Pixels>]| {
            cs.windows(2)
                .all(|p| p[0].left() == p[1].left() && p[1].top() >= p[0].bottom())
        };
        let row = |cs: &[gpui_kit::Bounds<gpui_kit::Pixels>]| {
            cs.windows(2)
                .all(|p| p[0].top() == p[1].top() && p[1].left() >= p[0].right())
        };

        // Default: one column as wide as the grid, the cells sharing its height.
        assert_eq!(r.layout().arrange(BoxKind::Disk), Some(Arrange::Vertical));
        assert_eq!(shown(&mut r), Arrange::Vertical);
        let (grid, cs, fit) = cells(&mut r, Arrange::Vertical);
        assert_eq!(cs.len(), 3, "{cs:?} in {grid:?}");
        assert!(col(&cs) && !fit.scrolls, "{cs:?} in {grid:?}");
        assert!(grid.right() - cs[0].right() < px(1.), "{cs:?} in {grid:?}");
        assert!(
            grid.bottom() - cs[2].bottom() < px(4.),
            "{cs:?} in {grid:?}"
        );
        assert!(cs[0].size.height > px(CELL_MIN_H));
        assert_eq!(r.saved(), None, "nothing is written for the default");

        // The gear's Layout items end with the Arrange submenu, the shown choice selected.
        r.edit_layout();
        let (labels, items) = gear_menu(&mut r, BoxKind::Disk);
        assert_eq!(
            labels.last().map(String::as_str),
            Some("Arrange"),
            "{labels:?}"
        );
        let marks: Vec<_> = items.iter().map(|(l, c, _)| (l.as_str(), *c)).collect();
        assert_eq!(
            marks,
            [
                ("Automatic", false),
                ("Stack vertically", true),
                ("Side by side", false)
            ]
        );

        // Side by side: one row as high as the grid, saved, and the saved layout reloads.
        pick_arrange(&mut r, BoxKind::Disk, "Side by side");
        assert_eq!(shown(&mut r), Arrange::Horizontal);
        let (grid, cs, fit) = cells(&mut r, Arrange::Horizontal);
        assert_eq!(cs.len(), 3, "{cs:?} in {grid:?}");
        assert!(row(&cs) && !fit.scrolls_x, "{cs:?} in {grid:?}");
        assert!(
            grid.bottom() - cs[0].bottom() < px(1.),
            "{cs:?} in {grid:?}"
        );
        assert!(grid.right() - cs[2].right() < px(4.), "{cs:?} in {grid:?}");
        assert_eq!(saved_disk_arrange(&mut r).as_deref(), Some("horizontal"));
        let layout = r.layout();
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), layout);
        let (_, items) = gear_menu(&mut r, BoxKind::Disk);
        assert!(items[2].1 && !items[1].1, "Side by side selected");

        // In a narrower window the row does not fit at 160 px: the whole cells fill the row and
        // the third disk joins "+1 more".
        r.vcx.simulate_resize(size(px(1200.), px(900.)));
        draw(r.vcx);
        let (grid, cs, fit) = cells(&mut r, Arrange::Horizontal);
        assert!(
            (1..3).contains(&cs.len()) && row(&cs) && !fit.scrolls_x,
            "{cs:?} in {grid:?}"
        );
        assert!(cs[0].size.width >= px(CELL_MIN_W), "{cs:?}");
        assert!(
            cs.last().is_some_and(|c| c.right() <= grid.right()),
            "{cs:?} in {grid:?}"
        );
        let more = r
            .vcx
            .update(|_, cx| page.read(cx).disk_box().read(cx).shown_more_text());
        assert_eq!(more.as_ref(), format!("+{} more", 3 - cs.len()));

        // Automatic with a fourth disk: the mode's grid, here two columns in a 222 px box, saved
        // as `auto`. Edit Layout holds the row height, and a 650 px window is too short to fill
        // at 32 px rows (D-037), so the box keeps the unit.
        r.page_action(&tl_ui::pages::performance::actions::PerfOverviewEditDone);
        r.set_row_unit(32);
        assert_eq!(more_disks(&mut r, 2).len(), 4);
        r.vcx.simulate_resize(size(px(1200.), px(650.)));
        draw(r.vcx);
        r.edit_layout();
        pick_arrange(&mut r, BoxKind::Disk, "Automatic");
        assert_eq!(shown(&mut r), Arrange::Auto);
        let (grid, cs, fit) = cells(&mut r, Arrange::Auto);
        assert_eq!((fit.cols, fit.rows), (2, 2), "{cs:?} in {grid:?}");
        assert!(cs[0].top() == cs[1].top() && cs[2].left() == cs[0].left());
        assert_eq!(saved_disk_arrange(&mut r).as_deref(), Some("auto"));
        let layout = r.layout();
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), layout);

        // Stack vertically in that window: the whole cells fill the column, the rest join
        // "+n more", and no scrollbar space is kept.
        pick_arrange(&mut r, BoxKind::Disk, "Stack vertically");
        let (grid, cs, fit) = cells(&mut r, Arrange::Vertical);
        assert!(
            cs.len() < 4 && col(&cs) && !fit.scrolls,
            "{cs:?} in {grid:?}"
        );
        assert!(
            cs.iter()
                .all(|c| c.size.height >= px(CELL_MIN_H) && c.bottom() <= grid.bottom()),
            "{cs:?} in {grid:?}"
        );
        assert!(
            cs.iter().all(|c| grid.right() - c.right() < px(1.)),
            "{cs:?} in {grid:?}"
        );
        assert_eq!(
            saved_disk_arrange(&mut r),
            None,
            "the default is not written"
        );

        // A saved value wins over the default.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                let disk = layout
                    .grid
                    .iter_mut()
                    .find(|b| b.kind == "disk")
                    .expect("a Disk box");
                disk.arrange = Some("horizontal".to_owned());
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert_eq!(shown(&mut r), Arrange::Horizontal);
        assert_eq!(r.layout().arrange(BoxKind::Disk), Some(Arrange::Horizontal));

        // Reset Box gives the default column back and writes nothing for the box.
        r.page_action(&PerfOverviewResetBox {
            kind: "disk".into(),
        });
        assert_eq!(shown(&mut r), Arrange::Vertical);
        assert_eq!(r.saved(), Some(OverviewLayoutSetting::grid_default()));

        // A mode without the choice has no Arrange submenu and ignores the action.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "disk".into(),
            mode: "rows".into(),
        });
        let (labels, items) = gear_menu(&mut r, BoxKind::Disk);
        assert!(
            items.is_empty() && !labels.contains(&"Arrange".to_owned()),
            "{labels:?}"
        );
        set_disk_arrange(&mut r, "horizontal");
        assert_eq!(r.layout().arrange(BoxKind::Disk), None);
        assert_eq!(saved_disk_arrange(&mut r), None);

        // Net: Stack vertically is `stacked` (the default), Side by side is `columns`.
        let net_mode = |r: &mut Rig| r.layout().config(BoxKind::Net).mode.id;
        assert_eq!(net_mode(&mut r), "stacked");
        let (_, items) = gear_menu(&mut r, BoxKind::Net);
        let marks: Vec<_> = items.iter().map(|(l, c, _)| (l.as_str(), *c)).collect();
        assert_eq!(marks, [("Stack vertically", true), ("Side by side", false)]);
        pick_arrange(&mut r, BoxKind::Net, "Side by side");
        assert_eq!(net_mode(&mut r), "columns");
        let net = r.page.clone();
        assert_eq!(
            r.vcx.update(|_, cx| net.read(cx).net_box().read(cx).mode()),
            "columns"
        );
        let layout = r.layout();
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), layout);
        let (_, items) = gear_menu(&mut r, BoxKind::Net);
        let marks: Vec<_> = items.iter().map(|(l, c, _)| (l.as_str(), *c)).collect();
        assert_eq!(marks, [("Stack vertically", false), ("Side by side", true)]);
        pick_arrange(&mut r, BoxKind::Net, "Stack vertically");
        assert_eq!(net_mode(&mut r), "stacked");
        // `combined` has no Arrange choice.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "net".into(),
            mode: "combined".into(),
        });
        let (_, items) = gear_menu(&mut r, BoxKind::Net);
        assert!(items.is_empty());
    }

    /// Applies the Arrange choice `arrange` to the box `kind`, as the Arrange items do.
    fn set_arrange(r: &mut Rig, kind: &str, arrange: &str) {
        r.page_action(&PerfOverviewSetBoxArrange {
            kind: kind.into(),
            arrange: arrange.into(),
        });
    }

    /// The saved `arrange` of the box `kind`.
    fn saved_arrange(r: &mut Rig, kind: &str) -> Option<String> {
        r.saved().and_then(|s| {
            s.grid
                .into_iter()
                .find(|b| b.kind == kind)
                .and_then(|b| b.arrange)
        })
    }

    /// The first two of `ids` are in one column (same left, the second below) or in one row
    /// (same top, the second right of the first).
    fn assert_line(r: &mut Rig, ids: [String; 2], column: bool, at: &str) {
        let [a, b] = ids.map(|id| r.bounds_of(id));
        if column {
            assert_eq!(a.left(), b.left(), "{at}: {a:?} {b:?}");
            assert!(b.top() >= a.bottom(), "{at}: {a:?} {b:?}");
        } else {
            assert_eq!(a.top(), b.top(), "{at}: {a:?} {b:?}");
            assert!(b.left() >= a.right(), "{at}: {a:?} {b:?}");
        }
    }

    /// D-027: the CPU `core_charts` cells go in one column or one row, and the `chart_cores`
    /// parts stack or sit side by side. The choice saves, the default is not written, and
    /// Reset Box gives the automatic rule back.
    #[gpui_kit::test]
    fn overview_arrange_cpu_vertical_horizontal_auto(cx: &mut TestAppContext) {
        let mut r = open_rooted(cx, true, 1200., 900.);
        // A 456 px CPU box, tall enough for stacked parts and a 60 px drag.
        r.set_row_unit(60);
        r.edit_layout();
        let page = r.page.clone();
        let side = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| page.read(cx).cpu_box().read(cx).side_by_side())
        };
        let ids = || [core_chart_id(0), core_chart_id(1)];
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: "core_charts".into(),
        });
        assert_eq!(r.layout().arrange(BoxKind::Cpu), Some(Arrange::Auto));
        assert_line(&mut r, ids(), false, "auto fills a row first");
        set_arrange(&mut r, "cpu", "vertical");
        assert_line(&mut r, ids(), true, "vertical");
        assert_eq!(saved_arrange(&mut r, "cpu").as_deref(), Some("vertical"));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        set_arrange(&mut r, "cpu", "horizontal");
        assert_line(&mut r, ids(), false, "horizontal");
        let grid = r.bounds_of(CORE_CHARTS_ID.into());
        let cell = r.bounds_of(core_chart_id(0));
        assert!(
            grid.bottom() - cell.bottom() < px(1.),
            "{cell:?} in {grid:?}"
        );
        set_arrange(&mut r, "cpu", "auto");
        assert_eq!(saved_arrange(&mut r, "cpu"), None);

        // `chart_cores`: the box is wide, so Automatic is side by side.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: "chart_cores".into(),
        });
        let splitter = |r: &mut Rig| r.vcx.debug_bounds(CPU_SPLITTER_ID).is_some();
        assert!(side(&mut r) && splitter(&mut r));
        set_arrange(&mut r, "cpu", "vertical");
        assert!(!side(&mut r) && splitter(&mut r));
        assert_eq!(saved_arrange(&mut r, "cpu").as_deref(), Some("vertical"));

        // Stacked parts get a horizontal splitter at the default 40% of the body height.
        let frame = r.bounds_of(CPU_BOX_ID.into());
        let gap = r.gap(CPU_SPLITTER_ID);
        assert!(
            gap.size.width > gap.size.height,
            "a horizontal gap: {gap:?}"
        );
        assert_eq!(cpu_split(&mut r), 0.4);
        // A drag down moves the gap and saves the new split when the button goes up.
        let start = center(gap);
        r.press(start, 1);
        r.move_to(point(start.x, start.y + px(60.)));
        let moved = f32::from(r.gap(CPU_SPLITTER_ID).top() - gap.top());
        assert!((moved - 60.).abs() <= 1.5, "gap moved {moved}");
        r.release(point(start.x, start.y + px(60.)));
        let split = cpu_split(&mut r);
        assert!(split > 0.4 && split <= 0.8, "{split}");
        assert_eq!(cpu_split_of(&r.saved()), Some(f64::from(split)));
        // The body height the drag implies: 60 px moved the split by `split - 0.4`.
        let body_h = 60. / (split - 0.4);
        // A drag far down stops where the core rows keep 120 px.
        let at = center(r.gap(CPU_SPLITTER_ID));
        r.press(at, 1);
        r.move_to(point(at.x, px(5000.)));
        r.release(point(at.x, px(5000.)));
        let rows_h = f32::from(frame.bottom() - r.gap(CPU_SPLITTER_ID).bottom());
        assert!(rows_h >= 120., "core rows {rows_h}");
        // A double-click resets the default, which is not written.
        let at = center(r.gap(CPU_SPLITTER_ID));
        r.press(at, 2);
        r.release(at);
        assert_eq!(cpu_split(&mut r), 0.4);
        assert_eq!(cpu_split_of(&r.saved()), None);
        // A saved split places the stacked parts.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                let cpu = &mut layout.grid[0];
                cpu.mode = Some("chart_cores".into());
                cpu.arrange = Some("vertical".into());
                cpu.split = Some(0.6);
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert!(!side(&mut r));
        assert_eq!(cpu_split(&mut r), 0.6);
        let saved_gap = r.gap(CPU_SPLITTER_ID);
        assert!(
            saved_gap.size.width > saved_gap.size.height,
            "a horizontal gap: {saved_gap:?}"
        );
        // From 40% to 60% the gap moves down a fifth of the body.
        let moved = f32::from(saved_gap.top() - gap.top());
        assert!(
            (moved - 0.2 * body_h).abs() <= 2.,
            "gap moved {moved} of {body_h}"
        );

        set_arrange(&mut r, "cpu", "horizontal");
        assert!(side(&mut r) && splitter(&mut r));
        let gap = r.gap(CPU_SPLITTER_ID);
        assert!(gap.size.height > gap.size.width, "a vertical gap: {gap:?}");
        // Reset Box gives the automatic rule back and writes nothing for the box.
        r.page_action(&PerfOverviewResetBox { kind: "cpu".into() });
        assert_eq!(r.layout().arrange(BoxKind::Cpu), Some(Arrange::Auto));
        assert_eq!(saved_arrange(&mut r, "cpu"), None);
    }

    /// D-027: Memory `chart_rows` has the used chart on top (Automatic, Stack vertically) or on
    /// the left (Side by side) with a splitter of the matching direction.
    #[gpui_kit::test]
    fn overview_arrange_memory_vertical_horizontal_auto(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::mem_box::{
            MEM_CHART_SPLITTER_ID, MEM_USED_CHART_ID,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "memory".into(),
            mode: "chart_rows".into(),
        });
        assert_eq!(r.layout().arrange(BoxKind::Memory), Some(Arrange::Auto));
        for arrange in ["auto", "vertical"] {
            set_arrange(&mut r, "memory", arrange);
            let chart = r.bounds_of(MEM_USED_CHART_ID.into());
            let gap = r.bounds_of(MEM_CHART_SPLITTER_ID.into());
            assert!(
                chart.bottom() <= gap.top() + px(1.),
                "{arrange}: {chart:?} {gap:?}"
            );
        }
        set_arrange(&mut r, "memory", "auto");
        assert_eq!(saved_arrange(&mut r, "memory"), None);
        set_arrange(&mut r, "memory", "horizontal");
        let chart = r.bounds_of(MEM_USED_CHART_ID.into());
        let gap = r.bounds_of(MEM_CHART_SPLITTER_ID.into());
        let frame = r.bounds_of("perf-overview-box-memory".into());
        assert!(chart.right() <= gap.left() + px(1.), "{chart:?} {gap:?}");
        assert!(gap.size.height > gap.size.width, "a vertical gap: {gap:?}");
        assert!(chart.size.height > px(100.), "{chart:?} in {frame:?}");
        assert_eq!(
            saved_arrange(&mut r, "memory").as_deref(),
            Some("horizontal")
        );
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
    }

    /// D-029: Memory `charts` shows one chart per row (In use, Cached, Available, Swap, Commit),
    /// each under a header with its name, value and percent. The Arrange choice puts the cells in
    /// a grid, one column or one row, each at least 160 x 60 px, and it saves.
    #[gpui_kit::test]
    fn overview_mem_charts_arrange(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::mem_box::{
            CELL_MIN_H, CELL_MIN_W, CHARTS_GRID_ID, cell_chart_id, cell_head_id, cell_id,
        };
        use tl_ui::pages::performance::overview::parts::arrange_grid;
        for (w, h) in [(1920., 1200.), (1280., 800.)] {
            let mut r = open_rooted(cx, true, w, h);
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "memory".into(),
                mode: "charts".into(),
            });
            assert_eq!(r.layout().arrange(BoxKind::Memory), Some(Arrange::Auto));
            let page = r.page.clone();
            let mode = r
                .vcx
                .update(|_, cx| page.read(cx).mem_box().read(cx).mode());
            assert_eq!(mode, "charts");
            let ids = |i: usize| (cell_id(i), cell_chart_id(i));
            let cells = |r: &mut Rig, arrange: Arrange| {
                let grid = r.bounds_of(CHARTS_GRID_ID.into());
                let min = size(px(CELL_MIN_W), px(CELL_MIN_H));
                let fit = arrange_grid(arrange, 5, grid.size, min, px(4.), px(16.));
                let cs: Vec<_> = (0..5).map(|i| r.bounds_of(ids(i).0)).collect();
                for (i, c) in cs.iter().enumerate() {
                    assert_eq!(
                        c.size, fit.cell,
                        "{w}: {arrange:?} cell {i}: {c:?} in {grid:?}"
                    );
                    assert!(c.size.width >= px(CELL_MIN_W), "{w}: {c:?}");
                    assert!(c.size.height >= px(CELL_MIN_H), "{w}: {c:?}");
                }
                (grid, cs, fit)
            };

            // Every cell has its name, value and percent, and a chart below the header.
            for i in 0..5 {
                let cell = r.bounds_of(ids(i).0);
                let chart = r.bounds_of(ids(i).1);
                assert!(chart.top() > cell.top() && chart.bottom() <= cell.bottom() + px(1.));
                for part in ["name", "value", "percent"] {
                    let head = r.bounds_of(cell_head_id(i, part));
                    assert!(head.top() >= cell.top() && head.bottom() <= chart.top() + px(1.));
                }
            }

            // Automatic: the grid, every cell the same size.
            let (grid, _, fit) = cells(&mut r, Arrange::Auto);
            assert!(fit.cols >= 1 && fit.cols <= 5, "{fit:?} in {grid:?}");
            assert_eq!(saved_arrange(&mut r, "memory"), None);

            // Stack vertically: one column as wide as the grid.
            set_arrange(&mut r, "memory", "vertical");
            let (grid, cs, _) = cells(&mut r, Arrange::Vertical);
            assert!(
                cs.windows(2)
                    .all(|p| p[0].left() == p[1].left() && p[1].top() >= p[0].bottom()),
                "{cs:?} in {grid:?}"
            );
            assert_eq!(saved_arrange(&mut r, "memory").as_deref(), Some("vertical"));
            assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());

            // Side by side: one row.
            set_arrange(&mut r, "memory", "horizontal");
            let (grid, cs, _) = cells(&mut r, Arrange::Horizontal);
            assert!(
                cs.windows(2)
                    .all(|p| p[0].top() == p[1].top() && p[1].left() >= p[0].right()),
                "{cs:?} in {grid:?}"
            );
            assert_eq!(
                saved_arrange(&mut r, "memory").as_deref(),
                Some("horizontal")
            );
            assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());

            // Reset Box gives the default mode back, and with it no saved arrangement.
            r.page_action(&PerfOverviewResetBox {
                kind: "memory".into(),
            });
            assert_eq!(r.layout().config(BoxKind::Memory).mode.id, "rows");
            assert_eq!(r.layout().arrange(BoxKind::Memory), None);
        }
    }

    /// D-027: GPU `engine_charts` cells go in one column or one row, and the `chart_engines`
    /// chart sits on the left of its rows for Side by side.
    #[gpui_kit::test]
    fn overview_arrange_gpu_vertical_horizontal_auto(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::gpu_box::{
            GPU_SPLITTER_ID, chart_id, engine_cell_id, engine_row_id,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        let page = r.page.clone();
        let keys: Vec<&'static str> = r.vcx.update(|_, cx| {
            page.read(cx).gpu_box().read(cx).sections()[0]
                .engines
                .iter()
                .map(|e| e.key)
                .collect()
        });
        assert!(keys.len() >= 2, "{keys:?}");
        let ids = || [engine_cell_id(0, keys[0]), engine_cell_id(0, keys[1])];
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "gpu".into(),
            mode: "engine_charts".into(),
        });
        assert_eq!(r.layout().arrange(BoxKind::Gpu), Some(Arrange::Auto));
        set_arrange(&mut r, "gpu", "vertical");
        assert_line(&mut r, ids(), true, "vertical");
        assert_eq!(saved_arrange(&mut r, "gpu").as_deref(), Some("vertical"));
        set_arrange(&mut r, "gpu", "horizontal");
        assert_line(&mut r, ids(), false, "horizontal");
        set_arrange(&mut r, "gpu", "auto");
        assert_eq!(saved_arrange(&mut r, "gpu"), None);

        r.page_action(&PerfOverviewSetBoxMode {
            kind: "gpu".into(),
            mode: "chart_engines".into(),
        });
        let chart = r.bounds_of(chart_id(0));
        let gap = r.bounds_of(GPU_SPLITTER_ID.into());
        assert!(chart.bottom() <= gap.bottom(), "stacked: {chart:?} {gap:?}");
        set_arrange(&mut r, "gpu", "horizontal");
        let chart = r.bounds_of(chart_id(0));
        let gap = r.bounds_of(GPU_SPLITTER_ID.into());
        let row = r.bounds_of(engine_row_id(0, keys[0]));
        assert!(chart.right() <= gap.left() + px(1.), "{chart:?} {gap:?}");
        assert!(row.left() >= gap.right() - px(1.), "{row:?} {gap:?}");
        assert_eq!(saved_arrange(&mut r, "gpu").as_deref(), Some("horizontal"));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
    }

    /// D-027: Net `adapter_charts` is one column by default, the choice saves, and a saved
    /// value wins over the default.
    #[gpui_kit::test]
    fn overview_arrange_net_adapter_charts(cx: &mut TestAppContext) {
        // 1800 px wide, so two 200 px cells fit side by side in the Net body: only whole cells
        // show (UI-PRF-059), and at 1200 px the second would join "+1 more".
        let mut r = open_rooted(cx, true, 1800., 900.);
        let page = r.page.clone();
        let shown = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| page.read(cx).net_box().read(cx).arrange())
        };
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "net".into(),
            mode: "adapter_charts".into(),
        });
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let m = tl_model::system::Metric::valid;
            let adapter = |i: u16| {
                (
                    tl_model::ids::DeviceIdx(i),
                    tl_model::system::NetTotals {
                        device: tl_model::ids::DeviceIdx(i),
                        rx_bps: m(1000.),
                        tx_bps: m(100.),
                        rx_total: 1 << 30,
                        tx_total: 1 << 29,
                        link_speed_mbps: tl_model::system::Metric::default(),
                        state: tl_model::system::LinkState::Up,
                    },
                )
            };
            sys.nets = Arc::from([adapter(5), adapter(6)]);
            let mut devices = s.devices.entries.to_vec();
            devices.push(entry(
                6,
                DeviceIdentity::Net {
                    netns_ino: 1,
                    ifindex: 3,
                    name: Arc::from("wlan0"),
                },
                Some(NetKind::Wireless),
            ));
            let mut catalog = (*s.devices).clone();
            catalog.entries = devices.into();
            s.devices = Arc::new(catalog);
        });
        r.vcx.run_until_parked();
        let idxs = r
            .vcx
            .update(|_, cx| page.read(cx).net_box().read(cx).adapter_idxs());
        assert!(idxs.len() >= 2, "{idxs:?}");
        let ids = || {
            [
                tl_ui::pages::performance::overview::net_box::cell_id(idxs[0]),
                tl_ui::pages::performance::overview::net_box::cell_id(idxs[1]),
            ]
        };
        assert_eq!(r.layout().arrange(BoxKind::Net), Some(Arrange::Vertical));
        assert_eq!(shown(&mut r), Arrange::Vertical);
        assert_line(&mut r, ids(), true, "default one column");
        set_arrange(&mut r, "net", "horizontal");
        assert_eq!(shown(&mut r), Arrange::Horizontal);
        assert_line(&mut r, ids(), false, "horizontal");
        assert_eq!(saved_arrange(&mut r, "net").as_deref(), Some("horizontal"));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        set_arrange(&mut r, "net", "vertical");
        assert_eq!(
            saved_arrange(&mut r, "net"),
            None,
            "the default is not written"
        );
        // A saved value wins over the default.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                let net = layout
                    .grid
                    .iter_mut()
                    .find(|b| b.kind == "net")
                    .expect("a Net box");
                net.mode = Some("adapter_charts".to_owned());
                net.arrange = Some("auto".to_owned());
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert_eq!(shown(&mut r), Arrange::Auto);
        // Automatic fills the grid with the largest cells (`fill_grid`): two adapters sit side
        // by side in a wide grid and stack in a tall one. Both shapes must show.
        let (gap, scale) = r.vcx.update(|_, cx| {
            let m = tl_ui::theme::metrics(cx);
            (m.sp[2], m.ui_scale)
        });
        let (mut wide, mut tall) = (false, false);
        let mut seen = Vec::new();
        for (w, h) in [(1200., 900.), (2400., 700.), (1200., 2400.), (1000., 3000.)] {
            r.vcx.simulate_resize(size(px(w), px(h)));
            draw(r.vcx);
            let grid = r.bounds_of(tl_ui::pages::performance::overview::net_box::GRID_ID.into());
            seen.push((w, h, grid.size));
            let fit = tl_ui::pages::performance::overview::parts::fill_grid(
                2,
                grid.size,
                size(px(200. * scale), px(80. * scale)),
                gap,
                px(16. * scale),
            );
            assert!(
                !fit.scrolls && !fit.scrolls_x,
                "{w}x{h}: {fit:?} in {grid:?}"
            );
            let side = fit.cols == 2;
            assert_line(&mut r, ids(), !side, &format!("auto {w}x{h}"));
            wide |= side;
            tall |= !side;
        }
        assert!(wide && tall, "both shapes: {seen:?}");
        // Stacked and columns keep their switch and have no saved arrangement.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "net".into(),
            mode: "stacked".into(),
        });
        assert_eq!(r.layout().arrange(BoxKind::Net), None);
    }

    /// Checks that every part of disk `k`'s `disk_charts` cell header stays inside its cell with
    /// no overlap and the name keeps its width, with the rates on a second line below
    /// `HEAD_ONE_LINE_W`. Returns the cell.
    fn assert_head_fits(
        r: &mut Rig,
        k: &DeviceKey,
        at: &str,
    ) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        use tl_ui::pages::performance::overview::disk_box::{HEAD_ONE_LINE_W, cell_id, head_id};
        let cell = r.bounds_of(cell_id(k));
        let [name, active, read, write] =
            ["name", "active", "read", "write"].map(|p| r.bounds_of(head_id(k, p)));
        for (p, b) in [
            ("name", name),
            ("active", active),
            ("read", read),
            ("write", write),
        ] {
            assert!(
                b.left() >= cell.left()
                    && b.right() <= cell.right() + px(0.5)
                    && b.top() >= cell.top()
                    && b.bottom() <= cell.bottom() + px(0.5),
                "{at} {p} {b:?} outside {cell:?}"
            );
        }
        assert!(name.size.width >= px(48.), "{at} name {name:?}");
        let row = [name, active, read, write];
        for (i, a) in row.iter().enumerate() {
            for b in &row[i + 1..] {
                let same_line = a.top() < b.bottom() && b.top() < a.bottom();
                let apart = a.right() <= b.left() + px(0.5) || b.right() <= a.left() + px(0.5);
                assert!(!same_line || apart, "{at} {a:?} overlaps {b:?}");
            }
        }
        if cell.size.width < px(HEAD_ONE_LINE_W) {
            assert!(read.top() >= name.bottom(), "{at} rates under the name");
        } else {
            assert!(read.top() < name.bottom(), "{at} one line");
        }
        cell
    }

    /// Publishes the widest real header values for every disk in the Disk box: 100.0% active,
    /// 1023.9 MiB/s read and 1023.8 MiB/s write, and checks the rows show them.
    fn widest_rates(r: &mut Rig) {
        use tl_model::system::{DiskTotals, Metric};
        const MIB: f64 = 1024. * 1024.;
        let page = r.page.clone();
        let idxs: Vec<DeviceIdx> = r.vcx.update(|_, cx| {
            page.read(cx)
                .disk_box()
                .read(cx)
                .rows()
                .iter()
                .map(|d| d.idx)
                .collect()
        });
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.disks = idxs
                .iter()
                .map(|&device| {
                    (
                        device,
                        DiskTotals {
                            device,
                            active_pct: Metric::valid(100.),
                            read_bps: Metric::valid(1023.9 * MIB),
                            write_bps: Metric::valid(1023.8 * MIB),
                            response_ms: Metric::valid(1.),
                            read_total: 1 << 40,
                            written_total: 1 << 40,
                            capacity: 1 << 41,
                            temp_c: Metric::default(),
                        },
                    )
                })
                .collect::<Vec<_>>()
                .into();
        });
        r.vcx.run_until_parked();
        draw(r.vcx);
        let rows = r
            .vcx
            .update(|_, cx| page.read(cx).disk_box().read(cx).rows().to_vec());
        for d in &rows {
            assert_eq!(
                (d.active.as_ref(), d.read.as_ref(), d.write.as_ref()),
                ("100.0%", "1023.9 MiB/s", "1023.8 MiB/s"),
                "{}",
                d.name
            );
        }
    }

    /// D-026: every part of a `disk_charts` cell header stays inside its cell with no overlap,
    /// and the name keeps its width. Below `HEAD_ONE_LINE_W` the rates go on a second line.
    #[gpui_kit::test]
    fn overview_disk_cell_head_fits(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::HEAD_ONE_LINE_W;
        let mut r = open_rooted(cx, true, 1200., 900.);
        let keys = three_disks(&mut r);
        widest_rates(&mut r);
        let mut widths = Vec::new();
        for (w, h) in [(1200., 900.), (1200., 560.), (900., 560.)] {
            r.vcx.simulate_resize(size(px(w), px(h)));
            draw(r.vcx);
            // Cells that do not fit whole are in "+n more" (UI-PRF-059).
            for k in keys.iter().take(shown_disks(&mut r)) {
                let cell = assert_head_fits(&mut r, k, &format!("{w}x{h}"));
                widths.push(f32::from(cell.size.width));
            }
        }
        assert!(
            widths.iter().any(|&w| w < HEAD_ONE_LINE_W),
            "a cell below {HEAD_ONE_LINE_W} px: {widths:?}"
        );
    }

    /// D-026: with eight disks in narrow windows the `disk_charts` cells get narrower than
    /// `HEAD_LABEL_W`, down toward the 160 px minimum. The rates drop their letters there, and
    /// the header still fits its cell with the widest real values.
    #[gpui_kit::test]
    fn overview_disk_narrow_cell_head_fits(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::{CELL_MIN_W, HEAD_LABEL_W};
        let mut r = open_rooted(cx, true, 1200., 900.);
        let keys = more_disks(&mut r, 6);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "disk".into(),
            mode: "disk_charts".into(),
        });
        set_disk_arrange(&mut r, "auto");
        widest_rates(&mut r);
        let mut widths = Vec::new();
        for w in [500., 600., 700., 800., 900., 1000.] {
            for h in [560., 900.] {
                r.vcx.simulate_resize(size(px(w), px(h)));
                draw(r.vcx);
                // Cells that do not fit whole are in "+n more" (UI-PRF-059).
                for k in keys.iter().take(shown_disks(&mut r)) {
                    let cell = assert_head_fits(&mut r, k, &format!("{w}x{h}"));
                    assert!(cell.size.width >= px(CELL_MIN_W), "{w}x{h} {cell:?}");
                    widths.push(f32::from(cell.size.width));
                }
            }
        }
        assert!(
            widths.iter().any(|&w| w < HEAD_LABEL_W),
            "a cell below {HEAD_LABEL_W} px: {widths:?}"
        );
    }

    /// crw.70: in `rows` mode a long model stays on one line inside its header row, and every
    /// rate cell keeps its whole text, down to a 500 px window.
    #[gpui_kit::test]
    fn overview_disk_rows_text_stays_in_row(cx: &mut TestAppContext) {
        use tl_model::snapshot::{DeviceDetailsSnapshot, DiskDetails};
        use tl_model::table::Total;
        use tl_model::validity::Validity;
        use tl_ui::pages::performance::overview::disk_box::{
            DISK_ROW_ID_PREFIX, io_row_id, row_part_id,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        let keys = three_disks(&mut r);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "disk".into(),
            mode: "rows".into(),
        });
        let page = r.page.clone();
        let idxs: Vec<DeviceIdx> = r.vcx.update(|_, cx| {
            page.read(cx)
                .disk_box()
                .read(cx)
                .rows()
                .iter()
                .map(|d| d.idx)
                .collect()
        });
        r.engine.publish_with(|s| {
            let mut details = DeviceDetailsSnapshot::default();
            details.header.version = 1;
            details.disks = idxs
                .iter()
                .map(|&idx| {
                    (
                        idx,
                        DiskDetails {
                            model: Total {
                                value: Arc::from("HGST HDS724040ALE640 with a very long model"),
                                validity: Validity::Valid,
                            },
                            ..DiskDetails::default()
                        },
                    )
                })
                .collect();
            s.device_details = Arc::new(details);
        });
        widest_rates(&mut r);
        let mut shown = 0;
        for (w, h) in [(500., 900.), (660., 900.), (800., 900.), (1200., 900.)] {
            r.vcx.simulate_resize(size(px(w), px(h)));
            draw(r.vcx);
            for k in &keys {
                let at = format!("{w}x{h} {}", k.0);
                let row = r.bounds_of(format!("{DISK_ROW_ID_PREFIX}{}", k.0));
                let model = r.bounds_of(row_part_id(k, "model"));
                let active = r.bounds_of(row_part_id(k, "active"));
                assert!(
                    model.size.height <= row.size.height + px(0.5),
                    "{at} {model:?}"
                );
                assert!(
                    model.bottom() <= row.bottom() + px(0.5),
                    "{at} {model:?} {row:?}"
                );
                assert!(
                    model.right() <= active.left() + px(0.5),
                    "{at} {model:?} {active:?}"
                );
                for which in ["read", "write"] {
                    // A collapsed section has no Read and Write rows.
                    let id: &'static str = Box::leak(io_row_id(k, which).into_boxed_str());
                    let Some(line) = r.vcx.debug_bounds(id) else {
                        continue;
                    };
                    let rate = r.bounds_of(format!("{id}-rate"));
                    shown += 1;
                    assert!(
                        rate.right() <= line.right() + px(0.5) && rate.left() >= line.left(),
                        "{at} {which} {rate:?} outside {line:?}"
                    );
                    // "1023.9 MiB/s" at 12 px is wider than the old 76 px column.
                    assert!(rate.size.width > px(76.), "{at} {which} {rate:?}");
                    assert!(
                        rate.size.height <= line.size.height + px(0.5),
                        "{at} {which}"
                    );
                }
            }
        }
        assert!(shown >= 4, "Read and Write rows shown: {shown}");
    }

    /// crw.71: at 1280 x 800 the Net box's rate lines stay on one row inside the box, in
    /// `stacked` and in the narrower `columns` mode. Narrower pages may leave a line out when it
    /// does not fit whole (crw.100). tasklens-o55.54: the line holds the rate and its peak
    /// ("1.4 Mbps, peak 4.4 Mbps"); the box has no Peak or Total rows.
    #[gpui_kit::test]
    fn overview_net_rate_lines_stay_on_one_line(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::net_box::{DOWN_RATE_ID, NET_BOX_ID, UP_RATE_ID};
        let mut r = open_rooted(cx, true, 1280., 800.);
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let m = tl_model::system::Metric::valid;
            sys.nets = Arc::from([(
                tl_model::ids::DeviceIdx(5),
                tl_model::system::NetTotals {
                    device: tl_model::ids::DeviceIdx(5),
                    rx_bps: m(1000.),
                    tx_bps: m(100.),
                    rx_total: 1023 << 40,
                    tx_total: 1000 << 40,
                    link_speed_mbps: tl_model::system::Metric::default(),
                    state: tl_model::system::LinkState::Up,
                },
            )]);
            let mut devices = s.devices.entries.to_vec();
            devices.push(entry(
                5,
                DeviceIdentity::Net {
                    netns_ino: 1,
                    ifindex: 5,
                    name: Arc::from("eth5"),
                },
                Some(NetKind::Wired),
            ));
            let mut catalog = (*s.devices).clone();
            catalog.entries = devices.into();
            s.devices = Arc::new(catalog);
        });
        for (mode, arrange) in [("stacked", "vertical"), ("columns", "horizontal")] {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "net".into(),
                mode: mode.into(),
            });
            set_arrange(&mut r, "net", arrange);
            for width in [1280., 700., 480., 360., 330.] {
                r.vcx.simulate_resize(size(px(width), px(800.)));
                draw(r.vcx);
                let frame = r.bounds_of(NET_BOX_ID.into());
                for id in [DOWN_RATE_ID, UP_RATE_ID] {
                    // A narrow page stacks the boxes at their kind minimum, where a line that
                    // does not fit whole is left out (UI-PRF-017, crw.100).
                    let Some(line) = r.vcx.debug_bounds(id) else {
                        assert!(
                            width < 720. || id == UP_RATE_ID,
                            "{mode} {width} {id} drawn"
                        );
                        continue;
                    };
                    // One t_sm row; a wrapped value is two rows tall.
                    assert!(line.size.height < px(20.), "{mode} {width} {id} {line:?}");
                    assert!(
                        line.right() <= frame.right(),
                        "{mode} {width} {id} {line:?}"
                    );
                }
            }
        }
    }

    /// crw.71: in `adapter_charts` a long adapter name stays on one line in its cell header.
    /// Only whole cells show, inside the grid; the adapters left out are in "+n more"
    /// (UI-PRF-059, which replaces crw.71's every adapter in a cell).
    #[gpui_kit::test]
    fn overview_net_adapter_names_stay_in_cells(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::net_box::{GRID_ID, cell_id, name_id};
        let mut r = open_rooted(cx, true, 1280., 800.);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "net".into(),
            mode: "adapter_charts".into(),
        });
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let m = tl_model::system::Metric::valid;
            sys.nets = (5..12)
                .map(|i| {
                    (
                        tl_model::ids::DeviceIdx(i),
                        tl_model::system::NetTotals {
                            device: tl_model::ids::DeviceIdx(i),
                            rx_bps: m(1000.),
                            tx_bps: m(100.),
                            rx_total: 1 << 30,
                            tx_total: 1 << 29,
                            link_speed_mbps: tl_model::system::Metric::default(),
                            state: tl_model::system::LinkState::Up,
                        },
                    )
                })
                .collect();
            let mut catalog = (*s.devices).clone();
            let mut devices = catalog.entries.to_vec();
            for i in 5..12u16 {
                devices.push(entry(
                    i,
                    DeviceIdentity::Net {
                        netns_ino: 1,
                        ifindex: u32::from(i),
                        name: Arc::from(format!("enx00e04c680a{i}-with-a-very-long-adapter-name")),
                    },
                    Some(NetKind::Wired),
                ));
            }
            catalog.entries = devices.into();
            s.devices = Arc::new(catalog);
        });
        r.vcx.run_until_parked();
        let page = r.page.clone();
        for arrange in ["vertical", "horizontal"] {
            set_arrange(&mut r, "net", arrange);
            for (w, h) in [(1280., 800.), (700., 800.), (360., 800.)] {
                r.vcx.simulate_resize(size(px(w), px(h)));
                draw(r.vcx);
                let (idxs, cells) = r.vcx.update(|_, cx| {
                    let b = page.read(cx).net_box().read(cx);
                    (b.adapter_idxs(), b.cells())
                });
                assert_eq!(idxs.len(), 7, "{arrange} {w}x{h}");
                assert!((1..=7).contains(&cells), "{arrange} {w}x{h}: {cells} cells");
                let grid = r.bounds_of(GRID_ID.into());
                for idx in idxs.into_iter().take(cells) {
                    let cell = r.bounds_of(cell_id(idx));
                    assert!(
                        cell.bottom() <= grid.bottom() + px(0.5)
                            && cell.right() <= grid.right() + px(0.5),
                        "{arrange} {w}x{h}: {cell:?} cut by {grid:?}"
                    );
                    let name = r.bounds_of(name_id(idx));
                    // One t_sm row; a wrapped name is two rows tall and draws over the chart.
                    assert!(name.size.height < px(20.), "{arrange} {w}x{h} {name:?}");
                    assert!(
                        name.right() <= cell.right() + px(0.5),
                        "{arrange} {w}x{h} {name:?} {cell:?}"
                    );
                }
            }
        }
    }

    #[gpui_kit::test]
    fn overview_net_modes_render(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::net_box::{
            COMBINED_CHART_ID, NET_BOX_ID, NET_SPLITTER_ID, NET_STACK_SPLITTER_ID, cell_id,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        // A 390 px Net box, so the stacked charts keep 120 px on each side of the splitter.
        r.set_row_unit(60);
        let page = r.page.clone();
        let set = |r: &mut Rig, mode: &str| {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "net".into(),
                mode: mode.into(),
            });
        };
        let shown = |r: &mut Rig, id: String| {
            let id: &'static str = Box::leak(id.into_boxed_str());
            r.vcx.debug_bounds(id).is_some()
        };

        // The default (D-026): Download over Upload, each the full width, a splitter between.
        assert!(shown(&mut r, NET_STACK_SPLITTER_ID.into()));
        assert!(!shown(&mut r, NET_SPLITTER_ID.into()));
        assert!(!shown(&mut r, COMBINED_CHART_ID.into()));
        assert_eq!(r.layout().split(BoxKind::Net), Some(0.5));
        // The splitter is horizontal: it spans the box width, with 120 px of body on each side.
        let frame = r.bounds_of(NET_BOX_ID.into());
        let gap = r.gap(NET_STACK_SPLITTER_ID);
        assert!(
            gap.size.width >= frame.size.width * 0.8,
            "{gap:?} in {frame:?}"
        );
        assert!(gap.size.width > gap.size.height);
        assert!(gap.top() - frame.top() >= px(120.), "{gap:?} in {frame:?}");
        assert!(
            frame.bottom() - gap.bottom() >= px(120.),
            "{gap:?} in {frame:?}"
        );

        // Two columns with the splitter between them.
        set(&mut r, "columns");
        assert!(shown(&mut r, NET_SPLITTER_ID.into()));
        assert!(!shown(&mut r, NET_STACK_SPLITTER_ID.into()));
        assert_eq!(r.layout().split(BoxKind::Net), Some(0.5));

        // One combined chart, no splitter, no split.
        set(&mut r, "combined");
        assert!(!shown(&mut r, NET_SPLITTER_ID.into()));
        assert!(shown(&mut r, COMBINED_CHART_ID.into()));
        assert_eq!(r.layout().split(BoxKind::Net), None);
        let frame = r.bounds_of(NET_BOX_ID.into());
        let chart = r.bounds_of(COMBINED_CHART_ID.into());
        assert!(chart.right() <= frame.right() && chart.bottom() <= frame.bottom());
        let setting = r.saved();
        assert_eq!(OverviewLayout::from_setting(setting.as_ref()), r.layout());

        // One combined chart per counted adapter in a grid.
        // The fixture's catalog has eth0 (idx 5); give it rates so it is counted.
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let m = tl_model::system::Metric::valid;
            sys.nets = Arc::from([(
                tl_model::ids::DeviceIdx(5),
                tl_model::system::NetTotals {
                    device: tl_model::ids::DeviceIdx(5),
                    rx_bps: m(1000.),
                    tx_bps: m(100.),
                    rx_total: 1 << 30,
                    tx_total: 1 << 29,
                    link_speed_mbps: tl_model::system::Metric::default(),
                    state: tl_model::system::LinkState::Up,
                },
            )]);
        });
        r.vcx.run_until_parked();
        set(&mut r, "adapter_charts");
        assert!(!shown(&mut r, NET_SPLITTER_ID.into()));
        assert!(!shown(&mut r, COMBINED_CHART_ID.into()));
        let idxs = r
            .vcx
            .update(|_, cx| page.read(cx).net_box().read(cx).adapter_idxs());
        assert!(!idxs.is_empty(), "the fixture has a counted adapter");
        let cells = r
            .vcx
            .update(|_, cx| page.read(cx).net_box().read(cx).cells());
        assert!(cells >= 1);
        let frame = r.bounds_of(NET_BOX_ID.into());
        let cell = r.bounds_of(cell_id(idxs[0]));
        assert!(
            cell.size.width >= px(200.) && cell.size.height >= px(80.),
            "{cell:?}"
        );
        assert!(cell.right() <= frame.right() && cell.bottom() <= frame.bottom());
        assert_eq!(r.layout().split(BoxKind::Net), None);

        // Back to the default mode: the splitter and the default split return.
        set(&mut r, "stacked");
        assert!(shown(&mut r, NET_STACK_SPLITTER_ID.into()));
        assert!(!shown(&mut r, cell_id(idxs[0])));
        assert_eq!(r.layout().split(BoxKind::Net), Some(0.5));
    }

    #[gpui_kit::test]
    fn overview_inner_split_drag_and_reset(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        // CPU and Memory share the top 12 columns equally, so in the CPU body the 120 px chart minimum is
        // above the 20% floor.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                layout.grid[0].cols = 6;
                layout.grid[1].col = 6;
                layout.grid[1].cols = 6;
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        r.edit_layout();
        let frame = r.vcx.debug_bounds(CPU_BOX_ID).expect("CPU box");
        assert!(frame.size.width >= px(560.), "side by side: {frame:?}");
        let pad = r
            .vcx
            .update(|_, cx| f32::from(tl_ui::theme::metrics(cx).sp[5]));
        let chart_w = |gap: gpui_kit::Bounds<gpui_kit::Pixels>| {
            f32::from(gap.left() - frame.left()) - 1. - pad
        };
        let before = r.saved();

        // Hover shows the accent line after 150 ms.
        let at = center(r.gap(CPU_SPLITTER_ID));
        r.vcx.simulate_mouse_move(at, None, Modifiers::none());
        draw(r.vcx);
        assert!(r.vcx.debug_bounds(CPU_SPLITTER_LINE_ID).is_none());
        r.vcx.executor().advance_clock(Duration::from_millis(150));
        draw(r.vcx);
        let line = r.vcx.debug_bounds(CPU_SPLITTER_LINE_ID).expect("line");
        assert_eq!(f32::from(line.size.width), 2.);

        // A drag far left stops at the chart's 120 px minimum. Nothing is saved until the
        // button goes up.
        r.press(at, 1);
        r.move_to(point(px(-5000.), at.y));
        assert!(
            r.vcx.debug_bounds(CPU_SPLITTER_LINE_ID).is_some(),
            "lit while held"
        );
        assert_eq!(r.saved(), before, "nothing is saved during the drag");
        r.release(point(px(-5000.), at.y));
        let w = chart_w(r.gap(CPU_SPLITTER_ID));
        assert!((w - 120.).abs() <= 1.5, "chart {w}");
        let split = cpu_split(&mut r);
        assert!(split > 0.2, "{split}");
        assert_eq!(cpu_split_of(&r.saved()), Some(f64::from(split)));

        // A double-click resets the mode's default, which is not written.
        let at = center(r.gap(CPU_SPLITTER_ID));
        r.press(at, 2);
        r.release(at);
        assert_eq!(cpu_split(&mut r), 0.4);
        assert_eq!(cpu_split_of(&r.saved()), None);
        let w = chart_w(r.gap(CPU_SPLITTER_ID));
        let body = f32::from(frame.size.width) - 2. - 2. * pad;
        assert!((w - 0.4 * body).abs() <= 1.5, "chart {w} of {body}");
    }

    impl Rig<'_> {
        /// Whether the gap `id` has its hit area, which carries the resize cursor.
        fn has_hit_area(&mut self, id: &str) -> bool {
            let hit: &'static str = Box::leak(hit_selector(id).into_boxed_str());
            self.vcx.debug_bounds(hit).is_some()
        }

        /// A hover past the accent line delay, a drag along both axes and a double-click on
        /// the middle of the gap `id`.
        fn hover_drag_and_double_click(&mut self, id: &'static str) {
            let at = center(self.gap(id));
            self.vcx.simulate_mouse_move(at, None, Modifiers::none());
            draw(self.vcx);
            self.vcx
                .executor()
                .advance_clock(Duration::from_millis(150));
            draw(self.vcx);
            self.press(at, 1);
            let to = point(at.x - px(120.), at.y - px(120.));
            self.move_to(to);
            self.release(to);
            self.press(at, 2);
            self.release(at);
        }
    }

    /// D-033: outside Edit Layout an inner splitter has no hit area, so no resize cursor and no
    /// accent line, and a drag or a double-click leaves the split where it was.
    #[gpui_kit::test]
    fn overview_inner_split_inert_outside_edit(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        // The default layout's two-part modes: CPU chart and core rows, the Memory rows, and
        // the stacked Net charts.
        let ids = [CPU_SPLITTER_ID, MEM_SPLITTER_ID, NET_STACK_SPLITTER_ID];
        for id in ids {
            assert!(!r.has_hit_area(id), "{id}: no hit area");
            let before = r.gap(id);
            r.hover_drag_and_double_click(id);
            assert!(
                r.vcx.debug_bounds(CPU_SPLITTER_LINE_ID).is_none(),
                "{id}: no accent line"
            );
            assert_eq!(r.gap(id), before, "{id} stays");
        }
        assert_eq!(r.layout(), OverviewLayout::grid_default(), "splits kept");
        assert_eq!(r.saved(), None, "nothing saved");
        // In Edit Layout the same splitters have their hit areas.
        r.edit_layout();
        for id in ids {
            assert!(r.has_hit_area(id), "{id}: hit area in Edit Layout");
        }
    }

    /// D-033: Esc during an inner splitter drag leaves Edit Layout and ends the drag: the split
    /// goes back to where it was, and a later move or release neither moves nor saves it.
    #[gpui_kit::test]
    fn overview_inner_split_escape_during_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.bind_keys();
        // Edit Layout draws a taller title row, so the gap sits lower there: the place to come
        // back to is the one outside it.
        let rest = r.gap(CPU_SPLITTER_ID);
        r.edit_layout();
        let before = r.gap(CPU_SPLITTER_ID);
        let at = center(before);
        r.press(at, 1);
        r.move_to(point(at.x - px(120.), at.y - px(120.)));
        assert_ne!(
            r.gap(CPU_SPLITTER_ID),
            before,
            "the gap moves during the drag"
        );
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        assert!(!r.editing());
        assert_eq!(r.escapes.get(), 0, "Esc did not reach the global Esc rule");
        let to = point(at.x + px(200.), at.y + px(200.));
        r.move_to(to);
        assert_eq!(
            r.gap(CPU_SPLITTER_ID),
            rest,
            "a move after Esc changes nothing"
        );
        r.release(to);
        assert_eq!(r.gap(CPU_SPLITTER_ID), rest, "the release changes nothing");
        assert!(
            r.vcx.debug_bounds(CPU_SPLITTER_LINE_ID).is_none(),
            "no accent line"
        );
        assert_eq!(r.layout(), OverviewLayout::grid_default(), "split kept");
        assert_eq!(r.saved(), None, "nothing saved");
    }

    /// UI-PRF-019, D-029: in Edit Layout mode the title value ends before the gear and the hide
    /// button of every box, at both smoke sizes.
    #[gpui_kit::test]
    fn overview_edit_title_no_overlap(cx: &mut TestAppContext) {
        for (w, h) in [(1920., 1200.), (1280., 800.)] {
            let mut r = open_rooted(cx, true, w, h);
            draw(r.vcx);
            r.edit_layout();
            draw(r.vcx);
            let mut checked = 0;
            for kind in BoxKind::ALL {
                let box_id = format!("perf-overview-box-{}", kind.key());
                let value_id: &'static str = Box::leak(
                    tl_ui::pages::performance::overview::parts::title_value_id(&box_id)
                        .into_boxed_str(),
                );
                let Some(value) = r.vcx.debug_bounds(value_id) else {
                    continue;
                };
                let gear = r.bounds_of(gear_id(kind));
                let hide = r.bounds_of(hide_button_id(kind));
                for (name, ctl) in [("gear", gear), ("hide", hide)] {
                    assert!(
                        value.right() <= ctl.left(),
                        "{w}x{h} {kind:?}: value {value:?} overlaps {name} {ctl:?}"
                    );
                }
                checked += 1;
            }
            assert!(checked >= 3, "{w}x{h}: only {checked} boxes checked");
        }
    }

    /// The bounds of each device box frame drawn and of its parts named by `parts`, in
    /// `BoxKind::ALL` order.
    fn box_parts<const N: usize>(
        r: &mut Rig,
        parts: [fn(&str) -> String; N],
    ) -> Vec<(
        BoxKind,
        gpui_kit::Bounds<gpui_kit::Pixels>,
        [gpui_kit::Bounds<gpui_kit::Pixels>; N],
    )> {
        let mut out = Vec::new();
        for kind in BoxKind::ALL {
            // Neither the Top processes box nor the timeline is a device box.
            if matches!(kind, BoxKind::TopProcs | BoxKind::Timeline) {
                continue;
            }
            let box_id = box_id(kind);
            let Some(frame) = r.vcx.debug_bounds(box_id) else {
                continue;
            };
            let found = parts.map(|part| r.bounds_of(part(box_id)));
            out.push((kind, frame, found));
        }
        out
    }

    /// UI-PRF-059: every box's 22 px title line sits inside the border, 2 px under the 1 px top
    /// border (tasklens-o55.54), with the icon, title and range figures in it and the body below
    /// it. In Edit Layout mode the grip, gear and Hide box button sit on that line.
    #[gpui_kit::test]
    fn overview_box_title_inside_border(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::edit::grip_id;
        use tl_ui::pages::performance::overview::parts::{figures_id, title_name_id, title_row_id};
        for (w, h) in [(1920., 1200.), (1280., 800.)] {
            let mut r = open_rooted(cx, true, w, h);
            draw(r.vcx);
            let boxes = box_parts(&mut r, [title_row_id, title_name_id, figures_id]);
            assert_eq!(boxes.len(), 5, "{w}x{h}: every device box drawn");
            for (kind, frame, [row, name, figures]) in &boxes {
                assert_eq!(
                    row.top() - frame.top(),
                    px(3.),
                    "{w}x{h} {kind:?}: {row:?} in {frame:?}"
                );
                assert_eq!(row.size.height, px(22.), "{w}x{h} {kind:?}");
                assert!(
                    name.top() >= row.top()
                        && name.bottom() <= row.bottom()
                        && name.left() >= frame.left() + px(1.),
                    "{w}x{h} {kind:?}: {name:?} in {row:?}"
                );
                // In the title line after the title, or left out below it when the line is
                // too narrow.
                assert!(
                    (figures.top() == row.top() && figures.left() > name.right())
                        || figures.top() >= row.bottom(),
                    "{w}x{h} {kind:?}: figures {figures:?} and the title {row:?}"
                );
            }
            r.edit_layout();
            draw(r.vcx);
            // The Edit Layout toolbar moves the boxes down, so measure the title lines again.
            for (kind, _, [row]) in &box_parts(&mut r, [title_row_id]) {
                let mid = row.center().y;
                for (name, ctl) in [
                    ("grip", r.bounds_of(grip_id(*kind))),
                    ("gear", r.bounds_of(gear_id(*kind))),
                    ("hide", r.bounds_of(hide_button_id(*kind))),
                ] {
                    assert!(
                        (ctl.center().y - mid).abs() <= px(1.),
                        "{w}x{h} {kind:?}: {name} {ctl:?} off the title line {row:?}"
                    );
                }
            }
        }
    }

    /// UI-PRF-059: the title line and the body are 12 px in from the left and the right border
    /// of every box, and the body ends 4 px above the bottom border (tasklens-o55.54). A
    /// `disk_charts` list that does not fit keeps no scrollbar space, so its cells end 12 px
    /// from the right border too.
    #[gpui_kit::test]
    fn overview_box_padding_equal(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::{DISK_BOX_ID, cell_id};
        use tl_ui::pages::performance::overview::parts::{body_id, title_row_id};
        // The 1 px border and the 12 px padding.
        let inset = px(13.);
        for (w, h) in [(1920., 1200.), (1280., 800.)] {
            let mut r = open_rooted(cx, true, w, h);
            draw(r.vcx);
            let boxes = box_parts(&mut r, [title_row_id, body_id]);
            assert_eq!(boxes.len(), 5, "{w}x{h}: every device box drawn");
            for (kind, frame, [row, axis]) in &boxes {
                for (part, b) in [("title", row), ("body", axis)] {
                    assert_eq!(
                        b.left() - frame.left(),
                        inset,
                        "{w}x{h} {kind:?} {part} left: {b:?} in {frame:?}"
                    );
                    assert_eq!(
                        frame.right() - b.right(),
                        inset,
                        "{w}x{h} {kind:?} {part} right: {b:?} in {frame:?}"
                    );
                }
                assert_eq!(
                    frame.bottom() - axis.bottom(),
                    px(5.),
                    "{w}x{h} {kind:?} bottom: {axis:?} in {frame:?}"
                );
            }
        }
        let mut r = open_rooted(cx, true, 1200., 560.);
        let keys = more_disks(&mut r, 6);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "disk".into(),
            mode: "disk_charts".into(),
        });
        draw(r.vcx);
        let page = r.page.clone();
        let shown = r
            .vcx
            .update(|_, cx| page.read(cx).disk_box().read(cx).shown());
        assert!(
            (1..keys.len()).contains(&shown),
            "{shown} of {} cells",
            keys.len()
        );
        let frame = r.bounds_of(DISK_BOX_ID.into());
        let cell = r.bounds_of(cell_id(&keys[0]));
        // Cells are whole px wide, so up to 1 px is left over.
        let right = frame.right() - cell.right();
        assert!(
            right >= inset && right < inset + px(1.),
            "{cell:?} ends {right:?} from the right of {frame:?}"
        );
    }

    /// UI-PRF-059: the Disk box `disk_charts` cells show only whole: in each Arrange choice, a
    /// cell that would be cut is left out and counted in "+n more", and nothing scrolls.
    #[gpui_kit::test]
    fn box_list_shows_whole_rows(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::disk_box::{
            CELL_MIN_H, CELL_MIN_W, GRID_ID, cell_id,
        };
        let mut r = open_rooted(cx, true, 1200., 900.);
        let keys = more_disks(&mut r, 6);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "disk".into(),
            mode: "disk_charts".into(),
        });
        let page = r.page.clone();
        for arrange in ["vertical", "auto", "horizontal"] {
            set_disk_arrange(&mut r, arrange);
            let mut cut = false;
            for (w, h) in [(1200., 560.), (1200., 640.), (1000., 560.), (1800., 900.)] {
                r.vcx.simulate_resize(size(px(w), px(h)));
                draw(r.vcx);
                let (shown, more) = r.vcx.update(|_, cx| {
                    let d = page.read(cx).disk_box().read(cx);
                    (d.shown(), d.shown_more_text().to_string())
                });
                let grid = r.bounds_of(GRID_ID.into());
                for (i, key) in keys.iter().enumerate() {
                    let id: &'static str = Box::leak(cell_id(key).into_boxed_str());
                    let cell = r.vcx.debug_bounds(id);
                    if i >= shown {
                        assert!(cell.is_none(), "{arrange} {w}x{h}: cell {i} left out");
                        continue;
                    }
                    let c = cell.unwrap_or_else(|| panic!("{arrange} {w}x{h}: cell {i}"));
                    assert!(
                        c.top() >= grid.top()
                            && c.left() >= grid.left()
                            && c.bottom() <= grid.bottom()
                            && c.right() <= grid.right(),
                        "{arrange} {w}x{h}: cell {i} {c:?} whole in {grid:?}"
                    );
                    assert!(
                        c.size.width >= px(CELL_MIN_W) || arrange == "vertical",
                        "{arrange} {w}x{h}: {c:?}"
                    );
                    assert!(
                        c.size.height >= px(CELL_MIN_H) || arrange == "horizontal",
                        "{arrange} {w}x{h}: {c:?}"
                    );
                }
                if shown < keys.len() {
                    cut = true;
                    assert_eq!(
                        more,
                        format!("+{} more", keys.len() - shown),
                        "{arrange} {w}x{h}"
                    );
                } else {
                    assert_eq!(more, "", "{arrange} {w}x{h}");
                }
            }
            assert!(cut, "{arrange}: some window leaves cells out");
        }
    }

    /// UI-PRF-018: in Edit Layout mode the grip sits left of the title and clear of the value
    /// text, at both smoke sizes.
    #[gpui_kit::test]
    fn overview_edit_grip_before_title(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::edit::grip_id;
        for (w, h) in [(1920., 1200.), (1280., 800.)] {
            let mut r = open_rooted(cx, true, w, h);
            draw(r.vcx);
            r.edit_layout();
            draw(r.vcx);
            let mut checked = 0;
            for kind in BoxKind::ALL {
                let box_id = format!("perf-overview-box-{}", kind.key());
                let tab_id: &'static str = Box::leak(
                    tl_ui::pages::performance::overview::parts::title_name_id(&box_id)
                        .into_boxed_str(),
                );
                let value_id: &'static str = Box::leak(
                    tl_ui::pages::performance::overview::parts::title_value_id(&box_id)
                        .into_boxed_str(),
                );
                let (Some(tab), Some(value)) =
                    (r.vcx.debug_bounds(tab_id), r.vcx.debug_bounds(value_id))
                else {
                    continue;
                };
                let grip = r.bounds_of(grip_id(kind));
                assert!(
                    grip.right() <= tab.left(),
                    "{w}x{h} {kind:?}: grip {grip:?} is not left of the title {tab:?}"
                );
                assert!(
                    !grip.intersects(&value),
                    "{w}x{h} {kind:?}: grip {grip:?} overlaps the value {value:?}"
                );
                checked += 1;
            }
            assert!(checked >= 3, "{w}x{h}: only {checked} boxes checked");
        }
    }

    #[gpui_kit::test]
    fn overview_box_reset_and_gear(cx: &mut TestAppContext) {
        let mut r = open_rooted(cx, true, 1200., 900.);
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                layout.grid[0].split = Some(0.6);
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert_eq!(cpu_split(&mut r), 0.6);
        // A mode the box does not offer changes nothing.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: "rows".into(),
        });
        assert_eq!(cpu_split(&mut r), 0.6);
        // Reset Box: the default mode and split, saved.
        r.page_action(&PerfOverviewResetBox { kind: "cpu".into() });
        assert_eq!(cpu_split(&mut r), 0.4);
        assert_eq!(r.saved(), Some(OverviewLayoutSetting::grid_default()));

        // No gear outside Edit Layout mode; in it, one per box, left of Hide box.
        assert!(r.vcx.debug_bounds("perf-overview-gear-cpu").is_none());
        r.edit_layout();
        for kind in [BoxKind::Cpu, BoxKind::Memory, BoxKind::Net] {
            let gear = r.bounds_of(gear_id(kind));
            let hide = r.bounds_of(hide_button_id(kind));
            assert!(gear.right() <= hide.left(), "{kind:?}: {gear:?} {hide:?}");
            assert!(gear.top() >= r.bounds_of(title_handle_id(kind)).top());
        }
        // The gear opens the box's modes, the shown one selected.
        let at = center(r.bounds_of(gear_id(BoxKind::Cpu)));
        r.press(at, 1);
        r.release(at);
        let items: Vec<(String, bool)> = r.vcx.update(|window, cx| {
            let menu = tl_ui::components::menu::open_context_menu(window, cx)
                .expect("the gear menu is open");
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| {
                    (
                        e.label.map(|l| l.to_string()).unwrap_or_default(),
                        e.checked,
                    )
                })
                .collect()
        });
        let expected: Vec<(String, bool)> = [
            ("Chart and cores", true),
            ("Chart", false),
            ("Cores", false),
            ("Core charts", false),
            // The default snapshot reads as RAPL readable: a separator and the power item.
            ("", false),
            ("Show power draw", false),
            ("", false),
            ("Arrange", false),
        ]
        .map(|(l, c)| (l.to_owned(), c))
        .into();
        assert_eq!(items, expected);
    }

    #[gpui_kit::test]
    fn overview_gap_accent_line_after_150ms(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let at = center(r.gap("perf-overview-splitter-col-cpu"));
        r.vcx.simulate_mouse_move(at, None, Modifiers::none());
        draw(r.vcx);
        assert!(r.vcx.debug_bounds("perf-overview-splitter-line").is_none());
        r.vcx.executor().advance_clock(Duration::from_millis(150));
        draw(r.vcx);
        let line = r
            .vcx
            .debug_bounds("perf-overview-splitter-line")
            .expect("line");
        assert_eq!(f32::from(line.size.width), 2.);
        r.vcx
            .simulate_mouse_move(point(px(2.), px(2.)), None, Modifiers::none());
        draw(r.vcx);
        assert!(r.vcx.debug_bounds("perf-overview-splitter-line").is_none());
    }

    /// D-033: outside Edit Layout a gap between boxes has no hit area, so no resize cursor and no
    /// accent line, and a drag or a double-click leaves the boxes on their cells.
    #[gpui_kit::test]
    fn overview_gap_inert_outside_edit(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        let ids = [
            "perf-overview-splitter-col-cpu",
            "perf-overview-splitter-row-memory",
        ];
        for id in ids {
            assert!(!r.has_hit_area(id), "{id}: no hit area");
            r.hover_drag_and_double_click(id);
            assert!(
                r.vcx.debug_bounds("perf-overview-splitter-line").is_none(),
                "{id}: no accent line"
            );
        }
        assert_eq!(r.layout(), OverviewLayout::grid_default(), "spans kept");
        assert_eq!(r.saved(), None, "nothing saved");
        // In Edit Layout the same gaps have their hit areas.
        r.edit_layout();
        for id in ids {
            assert!(r.has_hit_area(id), "{id}: hit area in Edit Layout");
        }
    }

    impl Rig<'_> {
        /// Turns Edit Layout mode on the way the page menu does: the action on the page.
        fn edit_layout(&mut self) {
            self.page_action(&PerfOverviewEditLayout);
            assert!(self.editing());
        }

        fn page_action(&mut self, action: &dyn gpui_kit::Action) {
            let page = self.page.clone();
            self.vcx.update(|window, cx| {
                let focus = page.read(cx).focus_handle(cx);
                focus.dispatch_action(action, window, cx);
            });
            draw(self.vcx);
        }

        fn editing(&mut self) -> bool {
            let page = self.page.clone();
            self.vcx.update(|_, cx| page.read(cx).is_editing())
        }

        /// The placed kinds in reading order.
        fn kinds(&mut self) -> Vec<BoxKind> {
            self.layout()
                .reading_order()
                .iter()
                .map(|b| b.kind)
                .collect()
        }

        fn bounds_of(&mut self, id: String) -> gpui_kit::Bounds<gpui_kit::Pixels> {
            let id: &'static str = Box::leak(id.into_boxed_str());
            self.vcx.debug_bounds(id).expect(id)
        }

        /// A click on `kind`'s Hide box button.
        fn hide(&mut self, kind: BoxKind) {
            let at = center(self.bounds_of(hide_button_id(kind)));
            self.press(at, 1);
            self.release(at);
        }
    }

    /// UI-PRF-056: the toolbar and handles show in Edit Layout; Hide box frees the cells and
    /// Add Box puts the kind at its kind minimum in the first free place.
    #[gpui_kit::test]
    fn overview_edit_hide_and_add_box(cx: &mut TestAppContext) {
        use BoxKind::{Cpu, Disk, Gpu, Memory, Net, Timeline};
        use tl_ui::pages::performance::actions::PerfOverviewAddBox;
        let mut r = open(cx, true, 1200., 900.);
        assert!(r.vcx.debug_bounds(TOOLBAR_ID).is_none());
        r.edit_layout();
        let toolbar = r.vcx.debug_bounds(TOOLBAR_ID).expect("toolbar");
        assert_eq!(f32::from(toolbar.size.height), 40.);
        // Every box has its title line handle and a 16 px Hide box button.
        for kind in [Cpu, Memory, Gpu, Disk, Net] {
            r.bounds_of(title_handle_id(kind));
            let hide = r.bounds_of(hide_button_id(kind));
            assert_eq!(f32::from(hide.size.width), 16.);
        }
        r.hide(Gpu);
        assert_eq!(r.kinds(), [Cpu, Memory, Disk, Net, Timeline]);
        assert_eq!(
            r.layout().get(Disk),
            OverviewLayout::grid_default().get(Disk),
            "the others keep their cells"
        );
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        r.page_action(&PerfOverviewAddBox {
            kind: "processes".into(),
        });
        assert_eq!(
            r.layout().get(BoxKind::TopProcs),
            Some(GridBox {
                kind: BoxKind::TopProcs,
                col: 0,
                row: 17,
                cols: 4,
                rows: 7
            }),
            "4 × 7 does not fit GPU's old 6 rows above the timeline, so it goes under it"
        );
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        assert!(r.editing(), "the mode stays on");
        // Reset Layout restores the default and keeps the mode on.
        r.page_action(&PerfOverviewResetLayout);
        assert_eq!(r.layout(), OverviewLayout::grid_default());
        assert!(r.editing());
    }

    #[gpui_kit::test]
    fn overview_edit_hide_last_box_refused(cx: &mut TestAppContext) {
        let mut r = open_rooted(cx, true, 1200., 900.);
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.performance.overview = Some(OverviewLayoutSetting {
                    grid: [("cpu", 0), ("memory", 6)]
                        .map(|(kind, col)| OverviewGridSetting {
                            kind: kind.into(),
                            col,
                            row: 0,
                            cols: 6,
                            rows: 7,
                            ..OverviewGridSetting::default()
                        })
                        .to_vec(),
                    rows: Vec::new(),
                });
            });
        });
        draw(r.vcx);
        r.edit_layout();
        r.hide(BoxKind::Memory);
        assert_eq!(r.kinds(), [BoxKind::Cpu]);
        r.hide(BoxKind::Cpu);
        assert_eq!(r.kinds(), [BoxKind::Cpu], "the last box stays");
        let queue = r.toasts.clone().expect("a toast queue");
        let shown = r.vcx.update(|_, cx| queue.read(cx).visible_texts());
        assert_eq!(shown, ["At least one box must stay on the Overview page"]);
        assert_eq!(
            OverviewLayout::from_setting(r.saved().as_ref()),
            r.layout(),
            "the hide of Memory was saved"
        );
    }

    /// D-037: the rows fill the page outside Edit Layout. Edit Layout keeps the drawn row height
    /// from when it started, through the toolbar taking height and a hidden box, so cells do not
    /// move; Done fills the page again.
    #[gpui_kit::test]
    fn overview_grid_edit_freezes_row_height(cx: &mut TestAppContext) {
        use BoxKind::{Cpu, Gpu, Timeline};
        use tl_ui::pages::performance::actions::PerfOverviewEditDone;
        let mut r = open(cx, true, 1160., 1200.);
        let page = r.page.clone();
        let placement = |r: &mut Rig| r.vcx.update(|_, cx| page.read(cx).placement().clone());
        let before = placement(&mut r);
        assert!(before.grid.row > before.grid.unit, "{before:?}");
        assert!(!before.scrolls);
        let bottom = |p: &tl_ui::pages::performance::overview::layout::Placement| {
            p.boxes.iter().map(|b| b.bounds.bottom()).max()
        };
        assert_eq!(bottom(&before), Some(before.content_h - px(16.)));
        let cpu = r.vcx.debug_bounds(box_id(Cpu)).expect("cpu").size;

        r.edit_layout();
        draw(r.vcx);
        let editing = placement(&mut r);
        assert_eq!(
            editing.grid.row, before.grid.row,
            "the toolbar does not move cells"
        );
        assert!(
            editing.scrolls,
            "the toolbar's 40 px make the frozen grid scroll"
        );
        r.hide(Timeline);
        draw(r.vcx);
        let hidden = placement(&mut r);
        assert_eq!(
            hidden.grid.row, before.grid.row,
            "a hidden box does not move cells"
        );
        assert_eq!(r.vcx.debug_bounds(box_id(Cpu)).expect("cpu").size, cpu);
        assert!(bottom(&hidden) < Some(hidden.content_h), "{hidden:?}");

        // Done: 13 rows now fill the page, so every row grows.
        r.page_action(&PerfOverviewEditDone);
        draw(r.vcx);
        assert!(!r.editing());
        let after = placement(&mut r);
        assert!(after.grid.row > before.grid.row, "{after:?}");
        assert!(!after.scrolls);
        assert_eq!(bottom(&after), Some(after.content_h - px(16.)));
        // In the window, the lowest boxes end at the bottom padding of the 1200 px window.
        let gpu = r.vcx.debug_bounds(box_id(Gpu)).expect("gpu");
        assert_eq!(gpu.bottom(), px(1200. - 16.));
    }

    #[gpui_kit::test]
    fn overview_edit_esc_leaves_mode(cx: &mut TestAppContext) {
        use BoxKind::{Cpu, Disk};
        let mut r = open(cx, true, 1200., 900.);
        r.vcx.update(|_, cx| {
            tl_ui::actions::bind_keys_global(cx);
            bind_keys_performance(cx);
        });
        r.edit_layout();
        // A press on a title line focuses that box and changes nothing.
        let title = center(r.bounds_of(title_handle_id(Disk)));
        r.press(title, 1);
        r.release(title);
        assert_eq!(r.saved(), None, "a click changes nothing");
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        assert!(!r.editing());
        assert!(r.vcx.debug_bounds(TOOLBAR_ID).is_none());
        assert_eq!(r.escapes.get(), 0, "Esc did not reach the global Esc rule");
        // Out of the mode, Esc goes to the global rule.
        r.vcx.simulate_keystrokes("escape");
        assert_eq!(r.escapes.get(), 1);
        // The boxes are back at the top of the page, 8 px down.
        let cpu = r.vcx.debug_bounds(box_id(Cpu)).expect("cpu");
        assert_eq!(cpu.top(), px(8.));
    }

    /// UI-PRF-056: a box moves by its title line in whole cells. The ghost outline shows the
    /// target cells, the other boxes make room while the button is down, and the layout is
    /// saved when it goes up, at the cells under the pointer then.
    #[gpui_kit::test]
    fn overview_edit_drag_moves_box_with_ghost(cx: &mut TestAppContext) {
        use BoxKind::{Cpu, Disk, Gpu, Memory, Net};
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let from = center(r.bounds_of(title_handle_id(Cpu)));
        r.press(from, 1);
        assert!(
            r.vcx.debug_bounds(GHOST_ID).is_none(),
            "no ghost before a move"
        );
        // Edit Layout holds the rows at the height they had: one row and its gutter apart.
        let page = r.page.clone();
        let pitch = r.vcx.update(|_, cx| {
            let g = page.read(cx).placement().grid;
            g.row + g.gutter
        });
        // 3 rows down: CPU at row 3, GPU and Disk pushed below it.
        r.move_to(point(from.x, from.y + px(3. * pitch)));
        assert_eq!(cells(&mut r, Cpu), (0, 3, 8, 7));
        assert_eq!(cells(&mut r, Gpu), (0, 10, 4, 6));
        let ghost = r.vcx.debug_bounds(GHOST_ID).expect("ghost");
        assert_eq!(ghost, r.vcx.debug_bounds(box_id(Cpu)).expect("cpu"));
        assert_eq!(r.saved(), None, "nothing is saved during the drag");
        // The button goes up 6 rows down, past the last move: GPU and Disk close up into
        // CPU's space and CPU goes under them.
        r.release(point(from.x, from.y + px(6. * pitch)));
        assert!(r.vcx.debug_bounds(GHOST_ID).is_none());
        assert_eq!(cells(&mut r, Gpu), (0, 0, 4, 6));
        assert_eq!(cells(&mut r, Disk), (4, 0, 4, 6));
        assert_eq!(cells(&mut r, Cpu), (0, 6, 8, 7));
        assert_eq!(cells(&mut r, Memory), (8, 0, 4, 7));
        assert_eq!(cells(&mut r, Net), (8, 7, 4, 6));
        assert_eq!(r.layout().validate(), Ok(()));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        assert!(r.editing(), "the mode stays on");
        let gpu = r.vcx.debug_bounds(box_id(Gpu)).expect("gpu");
        let cpu = r.vcx.debug_bounds(box_id(Cpu)).expect("cpu");
        assert_eq!(cpu.top() - gpu.bottom(), px(6.));

        // Esc during a drag leaves the mode and puts the boxes back.
        r.vcx.update(|_, cx| {
            tl_ui::actions::bind_keys_global(cx);
            bind_keys_performance(cx);
        });
        let saved = r.saved();
        let before = r.layout();
        let from = center(r.bounds_of(title_handle_id(Net)));
        r.press(from, 1);
        r.move_to(point(from.x - px(400.), from.y));
        assert_eq!(cells(&mut r, Net).0, 4);
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        r.release(point(from.x - px(400.), from.y));
        assert!(!r.editing());
        assert_eq!(r.layout(), before);
        assert_eq!(r.saved(), saved);
    }

    /// Delete during a title drag hides the dragged box and ends the drag: the other boxes go
    /// back to their cells, and a later move or release does not bring the box back.
    #[gpui_kit::test]
    fn overview_edit_delete_during_drag_hides_box(cx: &mut TestAppContext) {
        use BoxKind::{Cpu, Gpu};
        let mut r = open(cx, true, 1200., 900.);
        r.vcx.update(|_, cx| {
            tl_ui::actions::bind_keys_global(cx);
            bind_keys_performance(cx);
        });
        r.edit_layout();
        let before = r.layout();
        let from = center(r.bounds_of(title_handle_id(Cpu)));
        r.press(from, 1);
        r.move_to(point(from.x, from.y + px(150.)));
        assert_eq!(
            cells(&mut r, Gpu),
            (0, 10, 4, 6),
            "GPU pushed down mid-drag"
        );
        r.vcx.simulate_keystrokes("delete");
        draw(r.vcx);
        let hidden = r.layout();
        assert!(!hidden.has(Cpu));
        let rest: Vec<GridBox> = before
            .boxes
            .iter()
            .copied()
            .filter(|b| b.kind != Cpu)
            .collect();
        assert_eq!(hidden.boxes.as_slice(), rest.as_slice());
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), hidden);
        assert!(r.vcx.debug_bounds(GHOST_ID).is_none());
        // The button is still down: a move to another cell and the release change nothing.
        r.move_to(point(from.x + px(200.), from.y + px(300.)));
        assert_eq!(r.layout(), hidden);
        r.release(point(from.x + px(200.), from.y + px(300.)));
        assert_eq!(r.layout(), hidden);
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), hidden);
        assert!(r.editing(), "the mode stays on");
    }

    impl Rig<'_> {
        /// Holds CPU's title line 3 rows down, runs `action`, then moves the pointer to another
        /// cell and lets go. Returns the layout right after `action`, and checks that the move
        /// and the release keep it.
        fn drag_through(&mut self, action: impl FnOnce(&mut Self)) -> OverviewLayout {
            let before = self.layout();
            let from = center(self.bounds_of(title_handle_id(BoxKind::Cpu)));
            self.press(from, 1);
            self.move_to(point(from.x, from.y + px(150.)));
            assert_ne!(self.layout(), before, "the boxes move during the drag");
            action(self);
            let after = self.layout();
            assert!(self.vcx.debug_bounds(GHOST_ID).is_none(), "the ghost goes");
            let to = point(from.x + px(200.), from.y + px(300.));
            self.move_to(to);
            assert_eq!(
                self.layout(),
                after,
                "a move after the action changes nothing"
            );
            self.release(to);
            assert_eq!(self.layout(), after, "the release changes nothing");
            after
        }

        fn bind_keys(&mut self) {
            self.vcx.update(|_, cx| {
                tl_ui::actions::bind_keys_global(cx);
                bind_keys_performance(cx);
            });
        }
    }

    /// Reset Layout during a title drag ends the drag: the default layout stays and is saved.
    #[gpui_kit::test]
    fn overview_edit_reset_during_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        r.hide(BoxKind::Net);
        let after = r.drag_through(|r| r.page_action(&PerfOverviewResetLayout));
        assert_eq!(after, OverviewLayout::grid_default());
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), after);
        assert!(r.editing(), "the mode stays on");
    }

    /// Add Box during a title drag ends the drag: the boxes go back to their cells and the new
    /// box joins them.
    #[gpui_kit::test]
    fn overview_edit_add_box_during_drag(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::PerfOverviewAddBox;
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let before = r.layout();
        let after = r.drag_through(|r| {
            r.page_action(&PerfOverviewAddBox {
                kind: "processes".into(),
            })
        });
        assert!(after.has(BoxKind::TopProcs));
        let rest: Vec<GridBox> = after
            .boxes
            .iter()
            .copied()
            .filter(|b| b.kind != BoxKind::TopProcs)
            .collect();
        assert_eq!(rest.as_slice(), before.boxes.as_slice());
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), after);
    }

    /// Done during a title drag leaves the mode and puts the boxes back, unsaved.
    #[gpui_kit::test]
    fn overview_edit_done_during_drag(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::PerfOverviewEditDone;
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let before = r.layout();
        let after = r.drag_through(|r| r.page_action(&PerfOverviewEditDone));
        assert_eq!(after, before);
        assert!(!r.editing());
        assert_eq!(r.saved(), None);
    }

    /// Esc during a title drag leaves the mode and puts the boxes back, unsaved.
    #[gpui_kit::test]
    fn overview_edit_escape_during_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.bind_keys();
        r.edit_layout();
        let before = r.layout();
        let after = r.drag_through(|r| {
            r.vcx.simulate_keystrokes("escape");
            draw(r.vcx);
        });
        assert_eq!(after, before);
        assert!(!r.editing());
        assert_eq!(r.saved(), None);
        assert_eq!(r.escapes.get(), 0, "Esc did not reach the global Esc rule");
    }

    /// Window focus loss during a title drag puts the boxes back, unsaved; the mode stays on.
    #[gpui_kit::test]
    fn overview_edit_focus_loss_during_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.vcx.update(|window, _| window.activate_window());
        r.edit_layout();
        let before = r.layout();
        let after = r.drag_through(|r| {
            r.vcx.deactivate_window();
            draw(r.vcx);
        });
        assert_eq!(after, before);
        assert_eq!(r.saved(), None);
        assert!(r.editing(), "the mode stays on");
    }

    /// Switching page during a title drag puts the boxes back, unsaved.
    #[gpui_kit::test]
    fn overview_edit_page_switch_during_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.set_active(true);
        r.edit_layout();
        let before = r.layout();
        let after = r.drag_through(|r| {
            r.set_active(false);
            draw(r.vcx);
        });
        assert_eq!(after, before);
        assert_eq!(r.saved(), None);
    }

    /// Reset Layout during a gap drag ends the drag: a later move or release does not move the
    /// shared edge of the default layout.
    #[gpui_kit::test]
    fn overview_edit_reset_during_gap_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        r.hide(BoxKind::Net);
        let start = center(r.gap("perf-overview-splitter-col-cpu"));
        r.press(start, 1);
        r.move_to(point(start.x - px(100.), start.y));
        assert_eq!(cells(&mut r, BoxKind::Cpu), (0, 0, 7, 7));
        r.page_action(&PerfOverviewResetLayout);
        let reset = OverviewLayout::grid_default();
        assert_eq!(r.layout(), reset);
        r.move_to(point(start.x - px(300.), start.y));
        assert_eq!(r.layout(), reset);
        r.release(point(start.x - px(300.), start.y));
        assert_eq!(r.layout(), reset);
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), reset);
    }

    /// Alt+Shift+Left during a title drag ends the drag: the held box goes back to its cells
    /// and then shrinks one column, and a later move or release keeps that.
    #[gpui_kit::test]
    fn overview_edit_resize_key_during_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.bind_keys();
        r.edit_layout();
        let before = r.layout();
        let after = r.drag_through(|r| {
            r.vcx.simulate_keystrokes("alt-shift-left");
            draw(r.vcx);
        });
        let cpu = after.get(BoxKind::Cpu).expect("placed");
        assert_eq!((cpu.col, cpu.row, cpu.cols, cpu.rows), (0, 0, 7, 7));
        for kind in [BoxKind::Memory, BoxKind::Gpu, BoxKind::Disk, BoxKind::Net] {
            assert_eq!(after.get(kind), before.get(kind), "{kind:?} stays");
        }
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), after);
    }

    /// Alt+Right during a title drag ends the drag: the held box goes back to row 0 and then
    /// moves one column right, and a later move or release keeps that.
    #[gpui_kit::test]
    fn overview_edit_move_key_during_drag(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 1200., 900.);
        r.bind_keys();
        r.edit_layout();
        let after = r.drag_through(|r| {
            r.vcx.simulate_keystrokes("alt-right");
            draw(r.vcx);
        });
        let cpu = after.get(BoxKind::Cpu).expect("placed");
        assert_eq!((cpu.col, cpu.row, cpu.cols, cpu.rows), (1, 0, 8, 7));
        assert_eq!(after.validate(), Ok(()));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), after);
    }

    /// Alt+Shift+Left on a focused box during a gap drag ends the gap drag: the shared edge
    /// goes back, the box shrinks one column, and a later move or release keeps that.
    #[gpui_kit::test]
    fn overview_edit_resize_key_during_gap_drag(cx: &mut TestAppContext) {
        use BoxKind::{Cpu, Disk, Memory};
        let mut r = open(cx, true, 1200., 900.);
        r.bind_keys();
        r.edit_layout();
        // A quick click on the Disks title gives Disks focus.
        let title = center(r.bounds_of(title_handle_id(Disk)));
        r.press(title, 1);
        r.release(title);
        assert_eq!(cells(&mut r, Disk), (4, 7, 4, 6));
        let start = center(r.gap("perf-overview-splitter-col-cpu"));
        r.press(start, 1);
        r.move_to(point(start.x - px(100.), start.y));
        assert_eq!(cells(&mut r, Cpu), (0, 0, 7, 7));
        r.vcx.simulate_keystrokes("alt-shift-left");
        draw(r.vcx);
        let after = r.layout();
        assert_eq!(cells(&mut r, Cpu), (0, 0, 8, 7), "the gap drag ended");
        assert_eq!(cells(&mut r, Memory), (8, 0, 4, 7));
        assert_eq!(cells(&mut r, Disk), (4, 7, 3, 6), "the key acted");
        r.move_to(point(start.x - px(300.), start.y));
        assert_eq!(r.layout(), after, "a move after the key changes nothing");
        r.release(point(start.x - px(300.), start.y));
        assert_eq!(r.layout(), after, "the release changes nothing");
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), after);
    }

    /// UI-PRF-056: the bottom-right corner handle resizes in whole cells; a double-click sets
    /// the box to its kind minimum.
    #[gpui_kit::test]
    fn overview_edit_corner_resizes_box(cx: &mut TestAppContext) {
        use BoxKind::{Disk, Net};
        let mut r = open(cx, true, 1200., 900.);
        assert!(r.vcx.debug_bounds("perf-overview-corner-net").is_none());
        r.edit_layout();
        let net = r.vcx.debug_bounds(box_id(Net)).expect("net");
        let corner = r.bounds_of(corner_id(Net));
        assert_eq!(
            (corner.right(), corner.bottom()),
            (net.right(), net.bottom())
        );
        assert_eq!(f32::from(corner.size.width), 12.);
        // Down 1.1 rows and left 1.5 columns: one more row and two fewer columns, at the
        // kind minimum of 3 columns.
        let from = center(corner);
        r.press(from, 1);
        r.move_to(point(from.x, from.y + px(20.)));
        assert_eq!(cells(&mut r, Net), (8, 7, 4, 6), "under half a cell");
        r.move_to(point(from.x - px(147.), from.y + px(50.)));
        assert_eq!(cells(&mut r, Net), (8, 7, 3, 7));
        assert_eq!(r.saved(), None);
        r.release(point(from.x - px(147.), from.y + px(50.)));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        // 7 rows of the row height held since Edit Layout started (D-037), edges rounded.
        let page = r.page.clone();
        let row = r.vcx.update(|_, cx| page.read(cx).placement().grid.row);
        let top = 16. + 7. * (row + 6.);
        let net = r.vcx.debug_bounds(box_id(Net)).expect("net");
        assert_eq!(
            f32::from(net.size.height),
            (top + 7. * row + 6. * 6.).round() - top.round()
        );
        // A double-click on the corner sets the kind minimum, 3 × 6 at 1200 px, and saves.
        let at = center(r.bounds_of(corner_id(Net)));
        r.press(at, 2);
        r.release(at);
        assert_eq!(cells(&mut r, Net), (8, 7, 3, 6));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        assert_eq!(cells(&mut r, Disk), (4, 7, 4, 6), "Disk keeps its cells");
    }

    /// UI-PRF-056: with a box focused, Alt+arrow keys move it one cell and Alt+Shift+arrow
    /// keys grow or shrink it one cell at its right or bottom edge.
    #[gpui_kit::test]
    fn overview_edit_keys_move_and_resize(cx: &mut TestAppContext) {
        use BoxKind::{Disk, Gpu};
        let mut r = open(cx, true, 1200., 900.);
        r.vcx.update(|_, cx| {
            tl_ui::actions::bind_keys_global(cx);
            bind_keys_performance(cx);
        });
        r.edit_layout();
        // A quick click: the button goes up before the next frame, so only the title line
        // sees the release. The box takes focus and no drag is left pending.
        let title = center(r.bounds_of(title_handle_id(Disk)));
        r.vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: title,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        r.vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: title,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        draw(r.vcx);
        // Disk one column left onto GPU's last column: GPU moves down below it.
        r.vcx.simulate_keystrokes("alt-left");
        draw(r.vcx);
        assert_eq!(cells(&mut r, Disk), (3, 7, 4, 6));
        assert_eq!(cells(&mut r, Gpu), (0, 13, 4, 6));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        r.vcx.simulate_keystrokes("alt-shift-down");
        draw(r.vcx);
        assert_eq!(cells(&mut r, Disk), (3, 7, 4, 7));
        assert_eq!(cells(&mut r, Gpu), (0, 14, 4, 6));
        // Shrinking stops at the kind minimum of 3 columns.
        r.vcx.simulate_keystrokes("alt-shift-left");
        r.vcx.simulate_keystrokes("alt-shift-left");
        draw(r.vcx);
        assert_eq!(cells(&mut r, Disk), (3, 7, 3, 7));
        assert_eq!(OverviewLayout::from_setting(r.saved().as_ref()), r.layout());
        assert_eq!(r.layout().validate(), Ok(()));

        // A press whose release was never seen: a later move without the button ends it where
        // it was, and the box does not jump to the pointer.
        let before = r.layout();
        let title = center(r.bounds_of(title_handle_id(Disk)));
        r.press(title, 1);
        r.vcx
            .simulate_mouse_move(point(px(100.), px(800.)), None, Modifiers::none());
        draw(r.vcx);
        assert_eq!(r.layout(), before);
        r.vcx.simulate_keystrokes("alt-right");
        draw(r.vcx);
        assert_eq!(cells(&mut r, Disk), (4, 7, 3, 7), "no drag is left pending");
    }

    /// The stacked layout below 720 px has no resize handles, and a title drag moves nothing.
    #[gpui_kit::test]
    fn overview_edit_stacked_has_no_box_drags(cx: &mut TestAppContext) {
        let mut r = open(cx, true, 600., 900.);
        r.edit_layout();
        assert!(r.vcx.debug_bounds("perf-overview-corner-cpu").is_none());
        let from = center(r.bounds_of(title_handle_id(BoxKind::Net)));
        r.press(from, 1);
        r.move_to(point(from.x, from.y - px(600.)));
        r.release(point(from.x, from.y - px(600.)));
        assert!(r.vcx.debug_bounds(GHOST_ID).is_none());
        assert_eq!(r.layout(), OverviewLayout::grid_default());
        assert_eq!(r.saved(), None);
    }

    /// tasklens-o55.54: the CPU and Network charts draw their x ticks inside the plot along its
    /// bottom edge, with no row of their own, and the CPU chart keeps a 16 px gutter to the core
    /// rows.
    #[gpui_kit::test]
    fn chart_ticks_inside_plot_bottom(cx: &mut TestAppContext) {
        use tl_ui::chart::labels::LabelKind;
        use tl_ui::pages::performance::overview::net_box::COMBINED_CHART_ID;
        let mut r = open(cx, true, 1920., 1200.);
        // 70 px rows: the Network box is tall enough for a chart with both label rows.
        r.set_row_unit(70);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "net".into(),
            mode: "combined".into(),
        });
        r.cpu_tick();
        draw(r.vcx);
        let net = r.gap(COMBINED_CHART_ID);
        assert!(
            net.size.width >= px(320.) && net.size.height >= px(140.),
            "room for ticks: {net:?}"
        );
        let page = r.page.clone();
        let charts = r.vcx.update(|_, cx| {
            let p = page.read(cx);
            [
                ("cpu", p.cpu_box().read(cx).total_chart().clone()),
                ("net", p.net_box().read(cx).combined_chart().clone()),
            ]
        });
        for (name, chart) in charts {
            r.vcx.update(|_, cx| {
                let c = chart.read(cx).label_cache();
                for i in 0..3 {
                    let tick = c
                        .painted(LabelKind::XTick(i))
                        .unwrap_or_else(|| panic!("{name}: tick {i} painted"));
                    let tick = tick.bounds();
                    assert_eq!(tick.size.height, px(18.), "{name}: an 18 px line");
                    assert!(
                        tick.left() >= c.plot.left()
                            && tick.right() <= c.plot.right()
                            && tick.bottom() <= c.plot.bottom(),
                        "{name}: tick {i} {tick:?} inside the plot {:?}",
                        c.plot
                    );
                    assert!(
                        c.plot.bottom() - tick.bottom() <= px(4.),
                        "{name}: tick {i} {tick:?} on the plot's bottom edge {:?}",
                        c.plot
                    );
                }
            });
        }
        let gutter = r.gap(CPU_SPLITTER_ID);
        assert_eq!(gutter.size.width, px(16.), "chart to core rows: {gutter:?}");
    }

    #[gpui_kit::test]
    fn overview_stacked_layout_has_no_splitters(cx: &mut TestAppContext) {
        let r = open(cx, true, 600., 900.);
        assert!(
            r.vcx
                .debug_bounds("perf-overview-splitter-col-cpu")
                .is_none()
        );
        assert!(
            r.vcx
                .debug_bounds("perf-overview-splitter-row-memory")
                .is_none()
        );
    }
}

/// The Overview page inside `PerformancePage` in the main window, reached through the sidebar
/// (crw.42).
mod wired {
    use std::cell::Cell;
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, FocusHandle, IntoElement, Modifiers, MouseButton,
        MouseDownEvent, MouseUpEvent, Render, TestAppContext, VisualTestContext, Window, div, px,
        size,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::demand::{CpuDemand, Demand};
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
    use tl_model::ids::{DeviceIdx, GroupKey, MetricId, ProcessKey, SeriesId};
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{
        AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
    };
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, SeriesSet};
    use tl_model::time::{BootId, now_boot_ns};
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
    use tl_ui::chart::labels::LabelKind;
    use tl_ui::components::kv_grid;
    use tl_ui::components::menu::{MenuEntryKind, open_context_menu};
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::actions::PerfOverviewEditLayout;
    use tl_ui::pages::performance::overview::cpu_box::CPU_BOX_ID;
    use tl_ui::pages::performance::overview::disk_box::{DISK_BOX_ID, DISK_ROW_ID_PREFIX};
    use tl_ui::pages::performance::overview::edit::title_handle_id;
    use tl_ui::pages::performance::overview::gpu_box::GPU_BOX_ID;
    use tl_ui::pages::performance::overview::mem_box::MEM_BOX_ID;
    use tl_ui::pages::performance::overview::net_box::NET_BOX_ID;
    use tl_ui::pages::performance::overview::parts::{
        body_id, box_accent, title_name_id, title_row_id,
    };
    use tl_ui::pages::performance::overview::procs_box::{
        PROC_ROW_ID_PREFIX, PROCS_BOX_ID, ProcsBox,
    };
    use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
    use tl_ui::pages::performance::page::DevicePage as _;
    use tl_ui::pages::performance::{
        DeviceKey, DeviceKind, PAGE_PANEL_ID, PerformancePage, SIDEBAR_PANEL_ID,
    };
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::theme::current;
    use tl_ui::window::main::{
        self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
    };
    use tl_ui::window::titlebar::TitleBarProbe;

    fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("dev"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind,
            hot_c: None,
        }
    }

    /// CPU, memory, a GPU, two disks and a wired interface. [`open`] hides the GPU, disk and net
    /// cards, so only the Overview part asks for GPUs, disks and nets.
    fn catalog() -> Vec<DeviceEntry> {
        vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            entry(
                2,
                DeviceIdentity::Gpu {
                    uuid: [0; 16],
                    pci_bus_id: Arc::from("0000:01:00.0"),
                },
                None,
            ),
            entry(
                4,
                DeviceIdentity::Disk {
                    major: 8,
                    minor: 16,
                    name: Arc::from("sdb"),
                    wwn_or_serial: None,
                },
                None,
            ),
            entry(
                3,
                DeviceIdentity::Disk {
                    major: 8,
                    minor: 0,
                    name: Arc::from("sda"),
                    wwn_or_serial: None,
                },
                None,
            ),
            entry(
                5,
                DeviceIdentity::Net {
                    netns_ino: 1,
                    ifindex: 2,
                    name: Arc::from("eth0"),
                },
                Some(NetKind::Wired),
            ),
        ]
    }

    struct FakeProcesses {
        focus: FocusHandle,
        render_count: Cell<u32>,
    }

    impl gpui_kit::EventEmitter<tl_ui::window::status::StatusChanged> for FakeProcesses {}

    impl Render for FakeProcesses {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::{InteractiveElement as _, Styled as _};
            self.render_count.set(self.render_count.get() + 1);
            div().track_focus(&self.focus).size_full()
        }
    }

    impl shell::ShellPage for FakeProcesses {
        fn primary_focus(&self) -> FocusHandle {
            self.focus.clone()
        }
    }

    struct Rig<'a> {
        main: Entity<MainWindow>,
        page: Entity<PerformancePage>,
        demand: Entity<DemandAggregator>,
        engine: Arc<FakeEngine>,
        vcx: &'a mut VisualTestContext,
    }

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..2 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
    }

    /// The main window, 1600 × 1000, on the Performance tab with CPU selected and the sidebar
    /// focused. Animations are off. The GPU, disk and net cards are hidden.
    fn open(cx: &mut TestAppContext) -> Rig<'_> {
        open_with(cx, false)
    }

    /// [`open`]; with `all_cards` every card is shown.
    fn open_with(cx: &mut TestAppContext, all_cards: bool) -> Rig<'_> {
        open_with_theme(cx, all_cards, "Default Dark")
    }

    fn open_with_theme<'a>(
        cx: &'a mut TestAppContext,
        all_cards: bool,
        theme_name: &str,
    ) -> Rig<'a> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: Arc::from(catalog()),
            });
        });
        let fake = Arc::clone(&engine);
        let client: Arc<dyn EngineClient> = engine;
        let demand = cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            let theme_settings = ThemeSettings {
                active: theme_name.to_owned(),
                ..ThemeSettings::default()
            };
            tl_ui::theme::store::init(
                cx,
                &theme_settings,
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            init_strings(cx, "en-US");
            kv_grid::init(cx);
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            let mut settings = Settings {
                theme: theme_settings,
                ..Settings::default()
            };
            settings.performance.show_disks = all_cards;
            settings.performance.show_gpus = all_cards;
            settings.performance.show_network_wired = all_cards;
            cx.set_global(BootSettings {
                settings,
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub));
            cx.set_global(GlobalDemand(demand.clone()));
            tl_ui::components::toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            tl_ui::pages::performance::actions::bind_keys_performance(cx);
            cx.set_global(StartOptions::default());
            set_test_page(cx, Tab::Processes, |_, cx: &mut Context<MainWindow>| {
                let page = cx.new(|cx| FakeProcesses {
                    focus: cx.focus_handle().tab_stop(true),
                    render_count: Cell::new(0),
                });
                PageSlot::new(page, cx)
            });
            cx.set_global(TitleBarProbe {
                client: Some(true),
                ..TitleBarProbe::default()
            });
            tl_ui::window::menubar::init(cx);
            shell::init(cx);
            demand
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let vcx = VisualTestContext::from_window(handle.into(), cx).into_mut();
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1600.), px(1000.)));
        vcx.update(|window, _| window.activate_window());
        draw(vcx);
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        vcx.dispatch_action(tl_ui::actions::SelectTab(Tab::Performance as usize));
        draw(vcx);
        let page = vcx
            .read_entity(&main, |m, _| {
                m.page_entity::<PerformancePage>(Tab::Performance)
            })
            .expect("PerformancePage built");
        Rig {
            main,
            page,
            demand,
            engine: fake,
            vcx,
        }
    }

    fn key(s: &str) -> DeviceKey {
        DeviceKey(Arc::from(s))
    }

    fn select(r: &mut Rig, k: &str) {
        let page = r.page.clone();
        let k = key(k);
        r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().clone();
            sidebar.update(cx, |s, cx| s.select(k, cx));
        });
        draw(r.vcx);
    }

    fn selected(r: &mut Rig) -> Option<String> {
        let page = r.page.clone();
        r.vcx
            .update(|_, cx| page.read(cx).selected().map(|k| k.0.to_string()))
    }

    fn overview(r: &mut Rig) -> Option<Entity<OverviewPage>> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            page.read(cx)
                .page(&key(OVERVIEW_KEY))
                .and_then(|v| v.clone().downcast::<OverviewPage>().ok())
        })
    }

    fn merged(r: &mut Rig) -> Demand {
        let demand = r.demand.clone();
        r.vcx.update(|_, cx| demand.read(cx).merged())
    }

    fn overview_render_counts(r: &mut Rig) -> ([u32; 5], [u32; 7]) {
        let page = overview(r).expect("an OverviewPage");
        r.vcx.update(|_, cx| {
            let page = page.read(cx);
            let cpu = page.cpu_box().read(cx);
            let memory = page.mem_box().read(cx);
            let gpu = page.gpu_box().read(cx);
            let disk = page.disk_box().read(cx);
            let net = page.net_box().read(cx);
            (
                [
                    cpu.render_count.get(),
                    memory.render_count.get(),
                    gpu.render_count.get(),
                    disk.render_count.get(),
                    net.render_count.get(),
                ],
                [
                    cpu.total_chart().read(cx).render_count.get(),
                    memory.sparks()[0].read(cx).render_count.get(),
                    gpu.sections()[0].chart.read(cx).render_count.get(),
                    disk.rows()[0].read_chart.read(cx).render_count.get(),
                    disk.rows()[0].write_chart.read(cx).render_count.get(),
                    net.down_chart().read(cx).render_count.get(),
                    net.up_chart().read(cx).render_count.get(),
                ],
            )
        })
    }

    fn publish_cpu_series(r: &mut Rig) {
        let newest = now_boot_ns();
        r.engine.publish_with(|s| {
            let version = s.series.header.version + 1;
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: Arc::from([Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(0),
                        metric: MetricId::CPU_TOTAL,
                    },
                    version,
                    period_ns: 100_000_000,
                    samples: Arc::from([
                        Sample::valid(newest - 100_000_000, 20.),
                        Sample::valid(newest, 30.),
                    ]),
                })]),
                slow: Arc::from([]),
            });
        });
        draw(r.vcx);
    }

    /// The Overview part is in the merged demand: only it asks for disks, nets and GPUs here.
    fn has_overview_part(d: &Demand) -> bool {
        d.cpu == CpuDemand::PerCore && d.cpu_fast && d.disks && d.nets && d.gpus
    }

    fn mouse(
        cx: &mut VisualTestContext,
        button: MouseButton,
        at: gpui_kit::Point<gpui_kit::Pixels>,
    ) {
        cx.simulate_event(MouseDownEvent {
            button,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn overview_built_on_select(cx: &mut TestAppContext) {
        let mut r = open(cx);
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
        assert!(overview(&mut r).is_none(), "built on first visit only");
        // The Overview card is the first card: sp4 (8 px) below the sidebar top.
        let panel = r.vcx.debug_bounds(SIDEBAR_PANEL_ID).expect("the sidebar");
        let card = r.vcx.debug_bounds("perf-card").expect("a card");
        let at = gpui_kit::point(
            card.center().x,
            panel.top() + px(8.) + card.size.height / 2.,
        );
        mouse(r.vcx, MouseButton::Left, at);
        draw(r.vcx);
        assert_eq!(selected(&mut r).as_deref(), Some(OVERVIEW_KEY));
        let page = overview(&mut r).expect("an OverviewPage");
        assert!(r.vcx.read_entity(&page, |p, _| p.is_active()));
    }

    #[gpui_kit::test]
    fn overview_demand_follows_selection(cx: &mut TestAppContext) {
        let mut r = open(cx);
        select(&mut r, OVERVIEW_KEY);
        assert!(has_overview_part(&merged(&mut r)), "{:?}", merged(&mut r));
        select(&mut r, "cpu");
        let on_cpu = merged(&mut r);
        assert!(!on_cpu.disks && !on_cpu.nets && !on_cpu.gpus, "{on_cpu:?}");
        assert!(on_cpu.cpu_fast, "the CPU page part is sent");
        let page = overview(&mut r).expect("kept");
        assert!(!r.vcx.read_entity(&page, |p, _| p.is_active()));
        select(&mut r, OVERVIEW_KEY);
        assert!(has_overview_part(&merged(&mut r)));
    }

    #[gpui_kit::test]
    fn overview_demand_withdrawn_on_tab_leave(cx: &mut TestAppContext) {
        let mut r = open(cx);
        select(&mut r, OVERVIEW_KEY);
        assert!(has_overview_part(&merged(&mut r)));
        r.vcx
            .dispatch_action(tl_ui::actions::SelectTab(Tab::Processes as usize));
        draw(r.vcx);
        let away = merged(&mut r);
        assert!(!away.disks && !away.cpu_fast, "{away:?}");
        r.vcx
            .dispatch_action(tl_ui::actions::SelectTab(Tab::Performance as usize));
        draw(r.vcx);
        assert!(has_overview_part(&merged(&mut r)));
    }

    #[gpui_kit::test]
    fn overview_summary_round_trip(cx: &mut TestAppContext) {
        let mut r = open(cx);
        select(&mut r, OVERVIEW_KEY);
        r.vcx.simulate_keystrokes("ctrl-m");
        draw(r.vcx);
        let page = r.page.clone();
        assert!(r.vcx.read_entity(&page, |p, _| p.summary().is_some()));
        let during = merged(&mut r);
        assert!(!during.disks && !during.nets, "{during:?}");
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        assert!(r.vcx.read_entity(&page, |p, _| p.summary().is_none()));
        assert_eq!(selected(&mut r).as_deref(), Some(OVERVIEW_KEY));
        assert!(has_overview_part(&merged(&mut r)));
    }

    /// The kind and label of each entry of the open context menu, and its first entry's
    /// submenu entries with their marks.
    #[allow(clippy::type_complexity, reason = "test helper")]
    fn open_menu(r: &mut Rig) -> (Vec<(MenuEntryKind, String)>, Vec<(String, bool)>) {
        r.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("a context menu is open");
            let entries = menu.read(cx).entries_for_test(window);
            let sub = entries[0]
                .submenu
                .as_ref()
                .map(|sub| {
                    sub.read(cx)
                        .entries_for_test(window)
                        .into_iter()
                        .map(|e| {
                            (
                                e.label.map(|l| l.to_string()).unwrap_or_default(),
                                e.checked,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let labels = entries
                .into_iter()
                .map(|e| (e.kind, e.label.map(|l| l.to_string()).unwrap_or_default()))
                .collect();
            (labels, sub)
        })
    }

    /// The kind, label, tooltip and mark of each entry of the Layout submenu of the open menu.
    #[allow(clippy::type_complexity, reason = "test helper")]
    fn layout_entries(r: &mut Rig) -> Vec<(MenuEntryKind, String, Option<String>, bool)> {
        r.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("a context menu is open");
            let layout = menu.read(cx).entries_for_test(window)[0]
                .submenu
                .clone()
                .expect("the Layout submenu");
            let entries = layout.read(cx).entries_for_test(window);
            entries
                .into_iter()
                .map(|e| {
                    (
                        e.kind,
                        e.label.map(|l| l.to_string()).unwrap_or_default(),
                        e.tooltip.map(|t| t.to_string()),
                        e.checked,
                    )
                })
                .collect()
        })
    }

    /// D-028: "Show power draw" follows the mode radios in the CPU Layout submenu. It is a check
    /// item with RAPL readable, disabled with "Needs setup" and followed by "Set up CPU power
    /// draw…" when RAPL is unreadable, and missing without a RAPL zone.
    #[gpui_kit::test]
    fn overview_cpu_power_menu_item(cx: &mut TestAppContext) {
        use tl_model::snapshot::RaplState;
        let mut r = open(cx);
        select(&mut r, OVERVIEW_KEY);
        r.vcx.update(|_, cx| tl_ui::components::menu::bind_keys(cx));
        let title = r
            .vcx
            .debug_bounds(Box::leak(title_row_id(CPU_BOX_ID).into_boxed_str()))
            .expect("the CPU title line");
        let tail = |r: &mut Rig, rapl: Option<RaplState>| {
            r.engine.publish_with(|s| {
                let d = Arc::make_mut(&mut s.device_details);
                Arc::make_mut(&mut d.cpu).rapl = rapl;
                d.header.version += 1;
            });
            draw(r.vcx);
            mouse(r.vcx, MouseButton::Right, title.center());
            let entries = layout_entries(r);
            r.vcx.simulate_keystrokes("escape");
            draw(r.vcx);
            // The four mode radios come first.
            entries[4..].to_vec()
        };
        // The Arrange submenu closes the list, after a separator.
        let arrange = || (MenuEntryKind::Submenu, "Arrange".to_owned(), None, false);
        let sep = || (MenuEntryKind::Separator, String::new(), None, false);
        assert_eq!(tail(&mut r, Some(RaplState::NoZone)), [sep(), arrange()]);
        assert_eq!(
            tail(&mut r, Some(RaplState::Readable)),
            [
                (MenuEntryKind::Separator, String::new(), None, false),
                (
                    MenuEntryKind::Item,
                    "Show power draw".to_owned(),
                    None,
                    false
                ),
                sep(),
                arrange(),
            ]
        );
        assert_eq!(
            tail(&mut r, Some(RaplState::Unreadable)),
            [
                (MenuEntryKind::Separator, String::new(), None, false),
                (
                    MenuEntryKind::Disabled,
                    "Show power draw".to_owned(),
                    Some("Needs setup".to_owned()),
                    false
                ),
                (
                    MenuEntryKind::Item,
                    "Set up CPU power draw…".to_owned(),
                    None,
                    false
                ),
                sep(),
                arrange(),
            ]
        );
    }

    #[gpui_kit::test]
    fn overview_box_menu_switches_mode_and_saves(cx: &mut TestAppContext) {
        let mut r = open(cx);
        select(&mut r, OVERVIEW_KEY);
        r.vcx.update(|_, cx| tl_ui::components::menu::bind_keys(cx));
        let page = overview(&mut r).expect("an OverviewPage");
        let saved = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| TlSettings::settings(cx).performance.overview.clone())
        };
        let cpu_split = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                page.read(cx)
                    .layout()
                    .split(tl_ui::pages::performance::overview::layout::BoxKind::Cpu)
            })
        };
        let title = r
            .vcx
            .debug_bounds(Box::leak(title_row_id(CPU_BOX_ID).into_boxed_str()))
            .expect("the CPU title line");

        // Right-click on the title line: Layout, Reset Box, a separator, then the page menu.
        mouse(r.vcx, MouseButton::Right, title.center());
        let (labels, modes) = open_menu(&mut r);
        assert_eq!(
            labels[..4],
            [
                (MenuEntryKind::Submenu, "Layout".to_owned()),
                (MenuEntryKind::Item, "Reset Box".to_owned()),
                (MenuEntryKind::Separator, String::new()),
                (MenuEntryKind::Item, "Copy".to_owned()),
            ]
        );
        // One radio item per mode the box offers, the shown one selected.
        assert_eq!(
            modes,
            [
                ("Chart and cores".to_owned(), true),
                ("Chart".to_owned(), false),
                ("Cores".to_owned(), false),
                ("Core charts".to_owned(), false),
                // The default snapshot reads as RAPL readable.
                (String::new(), false),
                ("Show power draw".to_owned(), false),
                (String::new(), false),
                ("Arrange".to_owned(), false),
            ]
        );
        // Choosing the shown mode changes and saves nothing.
        r.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("the box menu is open");
            let layout = menu.read(cx).entries_for_test(window)[0]
                .submenu
                .clone()
                .expect("the Layout submenu");
            let action = layout
                .read(cx)
                .entries_for_test(window)
                .into_iter()
                .next()
                .and_then(|e| e.action)
                .expect("a mode item action");
            let focus = gpui_kit::Focusable::focus_handle(page.read(cx), cx);
            focus.dispatch_action(&*action, window, cx);
        });
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        assert_eq!(saved(&mut r), None);

        // Reset Box after a saved split: the default split, saved.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let mut layout = OverviewLayoutSetting::grid_default();
                layout.grid[0].split = Some(0.6);
                d.settings.performance.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert_eq!(cpu_split(&mut r), Some(0.6));
        mouse(r.vcx, MouseButton::Right, title.center());
        r.vcx.simulate_keystrokes("down down enter");
        draw(r.vcx);
        assert_eq!(cpu_split(&mut r), Some(0.4));
        assert_eq!(saved(&mut r), Some(OverviewLayoutSetting::grid_default()));

        // A right-click in the box body keeps the plain page menu.
        let frame = r.vcx.debug_bounds(CPU_BOX_ID).expect("the CPU box");
        mouse(r.vcx, MouseButton::Right, frame.center());
        let (labels, _) = open_menu(&mut r);
        assert_eq!(labels[0], (MenuEntryKind::Item, "Copy".to_owned()));
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);

        // In Edit Layout mode the title handle covers the title line: a right-click on it
        // still opens the box menu.
        r.vcx.update(|window, cx| {
            let focus = gpui_kit::Focusable::focus_handle(page.read(cx), cx);
            focus.dispatch_action(&PerfOverviewEditLayout, window, cx);
        });
        draw(r.vcx);
        let handle = r
            .vcx
            .debug_bounds(Box::leak(
                title_handle_id(tl_ui::pages::performance::overview::layout::BoxKind::Cpu)
                    .into_boxed_str(),
            ))
            .expect("the CPU title handle");
        mouse(r.vcx, MouseButton::Right, handle.center());
        let (labels, modes) = open_menu(&mut r);
        assert_eq!(
            labels[..3],
            [
                (MenuEntryKind::Submenu, "Layout".to_owned()),
                (MenuEntryKind::Item, "Reset Box".to_owned()),
                (MenuEntryKind::Separator, String::new()),
            ]
        );
        assert_eq!(
            modes,
            [
                ("Chart and cores".to_owned(), true),
                ("Chart".to_owned(), false),
                ("Cores".to_owned(), false),
                ("Core charts".to_owned(), false),
                // The default snapshot reads as RAPL readable.
                (String::new(), false),
                ("Show power draw".to_owned(), false),
                (String::new(), false),
                ("Arrange".to_owned(), false),
            ]
        );
    }

    #[gpui_kit::test]
    fn overview_page_menu_and_copy(cx: &mut TestAppContext) {
        let mut r = open(cx);
        select(&mut r, OVERVIEW_KEY);
        let at = r
            .vcx
            .debug_bounds(PAGE_PANEL_ID)
            .expect("the page panel")
            .center();
        mouse(r.vcx, MouseButton::Right, at);
        let labels: Vec<(MenuEntryKind, String)> = r.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("a context menu is open");
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| (e.kind, e.label.map(|l| l.to_string()).unwrap_or_default()))
                .collect()
        });
        assert_eq!(
            labels[..3],
            [
                (MenuEntryKind::Item, "Copy".to_owned()),
                (MenuEntryKind::Item, "Graph Summary View".to_owned()),
                (MenuEntryKind::Submenu, "Time range".to_owned()),
            ]
        );
        let page = overview(&mut r).expect("an OverviewPage");
        let mut text = String::new();
        r.vcx.read_entity(&page, |p, _| p.copy_text(&mut text));
        assert!(text.starts_with("Overview"), "{text:?}");

        // Copy puts every box on the clipboard (copy_all), not the title alone.
        let mut all = String::new();
        r.vcx.update(|_, cx| page.read(cx).copy_all(&mut all, cx));
        r.vcx.update(|_, cx| tl_ui::components::menu::bind_keys(cx));
        r.vcx.simulate_keystrokes("down enter");
        r.vcx.run_until_parked();
        let copied = r.vcx.read_from_clipboard().and_then(|c| c.text());
        assert_eq!(copied.as_deref(), Some(all.as_str()));
        assert!(all.len() > text.len(), "copy_all adds the boxes");
    }

    fn selector_id(id: String) -> &'static str {
        Box::leak(id.into_boxed_str())
    }

    fn center_of(r: &mut Rig, id: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
        let id = selector_id(id.to_owned());
        r.vcx.debug_bounds(id).expect(id).center()
    }

    /// UI-PRF-058: every box title tab starts with a 20 px device icon.
    #[gpui_kit::test]
    fn overview_box_titles_show_device_icons(cx: &mut TestAppContext) {
        let mut r = open_with(cx, true);
        select(&mut r, OVERVIEW_KEY);
        draw(r.vcx);
        for id in [CPU_BOX_ID, MEM_BOX_ID, GPU_BOX_ID, DISK_BOX_ID, NET_BOX_ID] {
            let icon = r
                .vcx
                .debug_bounds(selector_id(format!("{id}-title-icon")))
                .unwrap_or_else(|| panic!("{id} title icon drawn"));
            assert_eq!(icon.size.width, px(20.), "{id} icon width");
            assert_eq!(icon.size.height, px(20.), "{id} icon height");
        }
    }

    /// crw.44, wlf.37: the page measures its size during the first frame and lays out after it.
    /// Its uncached traversal can continue, but all five cached boxes are settled after select.
    #[gpui_kit::test]
    fn overview_first_frame_after_select_shows_all_boxes(cx: &mut TestAppContext) {
        let mut r = open_with(cx, true);
        select(&mut r, OVERVIEW_KEY);
        for id in [CPU_BOX_ID, MEM_BOX_ID, GPU_BOX_ID, DISK_BOX_ID, NET_BOX_ID] {
            assert!(r.vcx.debug_bounds(id).is_some(), "{id} drawn");
        }
        let before = overview_render_counts(&mut r);
        draw(r.vcx);
        assert_eq!(overview_render_counts(&mut r), before, "box caches settled");
    }

    /// wlf.37: the real main-window nesting leaves Overview's box and chart caches independent.
    #[gpui_kit::test]
    fn cpu_series_renders_only_overview_cpu_box_and_chart(cx: &mut TestAppContext) {
        let mut r = open_with(cx, true);
        select(&mut r, OVERVIEW_KEY);
        draw(r.vcx);
        let before = overview_render_counts(&mut r);

        publish_cpu_series(&mut r);

        let after = overview_render_counts(&mut r);
        let box_delta = std::array::from_fn::<_, 5, _>(|i| after.0[i] - before.0[i]);
        let chart_delta = std::array::from_fn::<_, 7, _>(|i| after.1[i] - before.1[i]);
        assert_eq!(box_delta, [1, 0, 0, 0, 0], "box render deltas");
        assert_eq!(chart_delta, [1, 0, 0, 0, 0, 0, 0], "chart render deltas");
    }

    /// wlf.37: the Performance exception does not remove page caching from another tab.
    #[gpui_kit::test]
    fn processes_page_remains_cached(cx: &mut TestAppContext) {
        let r = open(cx);
        r.vcx
            .dispatch_action(tl_ui::actions::SelectTab(Tab::Processes as usize));
        draw(r.vcx);
        let main = r.main.clone();
        let page = r
            .vcx
            .read_entity(&main, |m, _| m.page_entity::<FakeProcesses>(Tab::Processes))
            .expect("Processes page built");
        let before = r.vcx.read_entity(&page, |p, _| p.render_count.get());

        r.vcx.update(|_, cx| main.update(cx, |_, cx| cx.notify()));
        draw(r.vcx);

        assert_eq!(
            r.vcx.read_entity(&page, |p, _| p.render_count.get()),
            before,
            "clean non-Performance page cache reused"
        );
    }

    fn assert_overview_frame_anatomy(r: &mut Rig) {
        select(r, OVERVIEW_KEY);
        let theme = r.vcx.update(|_, cx| current(cx).clone());
        for (id, want) in [
            (CPU_BOX_ID, theme.graph.cpu_line),
            (MEM_BOX_ID, theme.graph.mem_line),
            (GPU_BOX_ID, theme.graph.gpu_line),
            (DISK_BOX_ID, theme.graph.disk_line),
            (NET_BOX_ID, theme.graph.net_line),
        ] {
            let frame = r.vcx.debug_bounds(id).expect("box frame");
            let row_id: &'static str = Box::leak(title_row_id(id).into_boxed_str());
            let name_id: &'static str = Box::leak(title_name_id(id).into_boxed_str());
            let row = r.vcx.debug_bounds(row_id).expect("title line");
            let name = r.vcx.debug_bounds(name_id).expect("title name");
            // UI-PRF-059: the 1 px border, then 2 px, then the title line (tasklens-o55.54).
            let row_inset = row.top() - frame.top();
            let name_inset = name.top() - frame.top();
            assert!(
                (px(3.)..=px(4.)).contains(&row_inset),
                "{id}: title line inset is {row_inset:?}"
            );
            assert!(
                (px(3.)..=px(4.)).contains(&name_inset),
                "{id}: title name inset is {name_inset:?}"
            );
            assert_eq!(name.size.height, row.size.height, "{id}: full title height");
            assert_eq!(box_accent(id, &theme), want, "{id}: device accent");
            // tasklens-o55.54: no time axis row under the body; the body ends 4 px above the
            // bottom border.
            let body = r
                .vcx
                .debug_bounds(selector_id(body_id(id)))
                .expect("box body");
            assert_eq!(frame.bottom() - body.bottom(), px(5.), "{id}: body bottom");
        }

        let overview = overview(r).expect("an OverviewPage");
        let charts = r.vcx.update(|_, cx| {
            let page = overview.read(cx);
            let cpu = page.cpu_box().read(cx).total_chart().clone();
            let memory = page.mem_box().read(cx).sparks()[0].clone();
            let gpu = page.gpu_box().read(cx).sections()[0].chart.clone();
            let disk = &page.disk_box().read(cx).rows()[0];
            let (disk_read, disk_write) = (disk.read_chart.clone(), disk.write_chart.clone());
            let net = page.net_box().read(cx);
            [
                cpu,
                memory,
                gpu,
                disk_read,
                disk_write,
                net.down_chart().clone(),
                net.up_chart().clone(),
            ]
        });
        for chart in charts {
            r.vcx.read_entity(&chart, |chart, _| {
                assert!(
                    chart.labels.range_word.is_empty(),
                    "internal caption removed"
                );
                assert!(
                    chart.label_cache().painted(LabelKind::Range).is_none(),
                    "internal caption not painted"
                );
                assert!(chart.spec.grid.lines, "faint grid enabled");
            });
        }

        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let sidebar_entity = page.read(cx).sidebar().clone();
            let sidebar = sidebar_entity.read(cx);
            for card in sidebar.cards() {
                let want = match card.row.kind {
                    DeviceKind::Overview | DeviceKind::Cpu => Some(theme.graph.cpu_line),
                    DeviceKind::Memory => Some(theme.graph.mem_line),
                    DeviceKind::Gpu => Some(theme.graph.gpu_line),
                    DeviceKind::Disk => Some(theme.graph.disk_line),
                    DeviceKind::Net => Some(theme.graph.net_line),
                    DeviceKind::Fan
                    | DeviceKind::Battery
                    | DeviceKind::AggregateDisk
                    | DeviceKind::AggregateGpu => None,
                };
                if let Some(want) = want {
                    assert_eq!(
                        card.chart.read(cx).spec.series[0].line,
                        want,
                        "{:?} sidebar mini-chart accent",
                        card.row.kind
                    );
                }
            }
        });
    }

    /// crw.52: Default Dark uses the shared title frame and device accent.
    #[gpui_kit::test]
    fn overview_title_lines_and_device_accents(cx: &mut TestAppContext) {
        let mut r = open_with(cx, true);
        assert_overview_frame_anatomy(&mut r);
    }

    /// crw.52: Default Light uses the same frame anatomy and device token mapping.
    #[gpui_kit::test]
    fn overview_title_lines_and_device_accents_light(cx: &mut TestAppContext) {
        let mut r = open_with_theme(cx, true, "Default Light");
        assert_eq!(
            &*r.vcx.update(|_, cx| current(cx).name.clone()),
            "Default Light"
        );
        assert_overview_frame_anatomy(&mut r);
    }

    /// crw.44: a click on a box title opens that device's page; a disk row opens its disk.
    #[gpui_kit::test]
    fn overview_titles_open_device_pages(cx: &mut TestAppContext) {
        let mut r = open_with(cx, true);
        // The disk row is a `rows` part; `disk_charts` has cells with headers instead (D-026).
        select(&mut r, OVERVIEW_KEY);
        let page = overview(&mut r).expect("an OverviewPage");
        r.vcx.update(|_, cx| {
            let disk = page.read(cx).disk_box().clone();
            disk.update(cx, |d, cx| d.set_mode("rows", cx));
        });
        draw(r.vcx);
        for (target, want) in [
            (title_row_id(CPU_BOX_ID), "cpu"),
            (title_row_id(MEM_BOX_ID), "memory"),
            (title_row_id(GPU_BOX_ID), "gpu:pci:0000:01:00.0"),
            // sdb comes first in the catalog, so it is the first disk row.
            (title_row_id(DISK_BOX_ID), "disk:sdb"),
            (format!("{DISK_ROW_ID_PREFIX}disk:sda"), "disk:sda"),
            (title_row_id(NET_BOX_ID), "net:eth0"),
        ] {
            select(&mut r, OVERVIEW_KEY);
            let at = center_of(&mut r, &target);
            mouse(r.vcx, MouseButton::Left, at);
            draw(r.vcx);
            assert_eq!(selected(&mut r).as_deref(), Some(want), "{target}");
        }
    }

    /// crw.44: a click on a chart does nothing, and pointer moves over the titles render
    /// nothing.
    #[gpui_kit::test]
    fn overview_chart_click_and_title_hover_do_nothing(cx: &mut TestAppContext) {
        let mut r = open_with(cx, true);
        select(&mut r, OVERVIEW_KEY);
        let page = overview(&mut r).expect("an OverviewPage");
        // The CPU box body, below the title row.
        let b = r.vcx.debug_bounds(CPU_BOX_ID).expect("the CPU box");
        let at = gpui_kit::point(b.left() + b.size.width / 4., b.top() + b.size.height * 0.6);
        mouse(r.vcx, MouseButton::Left, at);
        draw(r.vcx);
        assert_eq!(
            selected(&mut r).as_deref(),
            Some(OVERVIEW_KEY),
            "chart click"
        );

        let renders = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let p = page.read(cx);
                [
                    p.cpu_box().read(cx).render_count.get(),
                    p.mem_box().read(cx).render_count.get(),
                    p.gpu_box().read(cx).render_count.get(),
                    p.disk_box().read(cx).render_count.get(),
                    p.net_box().read(cx).render_count.get(),
                ]
            })
        };
        let before = renders(&mut r);
        for id in [CPU_BOX_ID, MEM_BOX_ID, GPU_BOX_ID, DISK_BOX_ID, NET_BOX_ID] {
            let row: &'static str = Box::leak(title_row_id(id).into_boxed_str());
            let t = r.vcx.debug_bounds(row).expect(row);
            for i in 0..10 {
                let x = t.left() + t.size.width * ((i as f32 + 0.5) / 10.);
                r.vcx.simulate_event(gpui_kit::MouseMoveEvent {
                    position: gpui_kit::point(x, t.center().y),
                    pressed_button: None,
                    modifiers: Modifiers::none(),
                });
            }
            draw(r.vcx);
        }
        assert_eq!(renders(&mut r), before, "title hover renders nothing");
    }

    #[gpui_kit::test]
    fn overview_page_kept(cx: &mut TestAppContext) {
        let mut r = open(cx);
        select(&mut r, OVERVIEW_KEY);
        let first = overview(&mut r).expect("an OverviewPage").entity_id();
        let page = r.page.clone();
        let count = r.vcx.read_entity(&page, |p, _| p.page_count());
        select(&mut r, "cpu");
        select(&mut r, OVERVIEW_KEY);
        assert_eq!(overview(&mut r).expect("kept").entity_id(), first);
        assert_eq!(r.vcx.read_entity(&page, |p, _| p.page_count()), count);
    }

    fn proc_key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    pub(super) fn proc_record(pid: u32, cpu_pct: f32) -> ProcessRecord {
        let name: Arc<str> = Arc::from(format!("proc{pid}"));
        ProcessRecord {
            key: proc_key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: name.clone(),
                name: name.clone(),
                exe: None,
                cmdline: None,
                user: Arc::from("user"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: name,
            }),
            group: GroupKey(0),
            state: ProcState::Running,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.,
            io_write_bps: 0.,
            net_rx_bps: 0.,
            net_tx_bps: 0.,
            gpu_util_pct: 0.,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    /// 12 processes: pid `10 + i` at `i` % CPU, so pids 21 down to 12 are the top 10. Each call
    /// is a new `PROCESSES` version.
    fn publish_procs(r: &mut Rig, exited: Option<u32>) {
        r.engine.publish_with(|s| {
            let version = s.processes.header.version + 1;
            let records: Vec<ProcessRecord> = (0..12u32)
                .map(|i| {
                    let mut rec = proc_record(10 + i, i as f32);
                    if exited == Some(10 + i) {
                        rec.flags = ProcFlags::EXITED;
                    }
                    rec
                })
                .collect();
            s.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                records: Arc::from(records),
                ..ProcessSnapshot::default()
            });
        });
        draw(r.vcx);
    }

    fn set_top_procs(r: &mut Rig, on: bool) {
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let unit = d.settings.performance.overview_row_unit_px;
                d.settings.performance.overview =
                    on.then(|| OverviewLayoutSetting::with_top_procs(unit));
            });
        });
        draw(r.vcx);
    }

    fn procs_box(r: &mut Rig) -> Option<Entity<ProcsBox>> {
        let page = overview(r).expect("an OverviewPage");
        r.vcx.update(|_, cx| page.read(cx).procs_box().cloned())
    }

    /// crw.45: the Top processes box is off by default, and the Overview asks for no process
    /// sweep.
    #[gpui_kit::test]
    fn overview_top_procs_off_by_default(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_procs(&mut r, None);
        select(&mut r, OVERVIEW_KEY);
        assert!(procs_box(&mut r).is_none());
        assert!(r.vcx.debug_bounds(PROCS_BOX_ID).is_none());
        let d = merged(&mut r);
        assert!(has_overview_part(&d), "{d:?}");
        assert!(!d.process_top && !d.process_table, "{d:?}");
    }

    /// crw.45: with the setting on, the box shows the 10 busiest live processes and the page
    /// asks for `process_top`; a tick with the same top 10 renders nothing; a row click selects
    /// that process; the setting off removes the box and the demand.
    #[gpui_kit::test]
    fn overview_top_procs_rows_ticks_and_click(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_procs(&mut r, None);
        select(&mut r, OVERVIEW_KEY);
        set_top_procs(&mut r, true);
        let procs = procs_box(&mut r).expect("the Top processes box");
        assert!(r.vcx.debug_bounds(PROCS_BOX_ID).is_some(), "placed");
        let title_row = selector_id(title_row_id(PROCS_BOX_ID));
        let title_name = selector_id(title_name_id(PROCS_BOX_ID));
        assert!(r.vcx.debug_bounds(title_row).is_some(), "shared title line");
        assert!(
            r.vcx.debug_bounds(title_name).is_some(),
            "shared title name"
        );
        let theme = r.vcx.update(|_, cx| current(cx).clone());
        assert_eq!(box_accent(PROCS_BOX_ID, &theme), theme.graph.cpu_line);
        let d = merged(&mut r);
        assert!(d.process_top && !d.process_table, "{d:?}");

        let pids = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                procs
                    .read(cx)
                    .rows()
                    .iter()
                    .map(|row| row.key.pid)
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(pids(&mut r), (12..=21).rev().collect::<Vec<_>>());
        let first = r.vcx.update(|_, cx| procs.read(cx).rows()[0].clone());
        assert_eq!(
            (first.name.as_ref(), first.pid.as_ref(), first.cpu.as_ref()),
            ("proc21", "21", "11%")
        );

        // The same top 10 at the same percents: nothing renders.
        let counts = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let b = procs.read(cx);
                (b.render_count.get(), b.notify_count.get())
            })
        };
        let before = counts(&mut r);
        publish_procs(&mut r, None);
        assert_eq!(counts(&mut r), before, "same top 10");
        // The busiest process exits: the rows change once.
        publish_procs(&mut r, Some(21));
        assert_eq!(pids(&mut r), (11..=20).rev().collect::<Vec<_>>());
        assert_eq!(counts(&mut r).1, before.1 + 1);

        let at = center_of(&mut r, &format!("{PROC_ROW_ID_PREFIX}18"));
        mouse(r.vcx, MouseButton::Left, at);
        draw(r.vcx);
        let jumped = r.vcx.update(|_, cx| procs.read(cx).last_jump.get());
        assert_eq!(jumped, Some(proc_key(18)));
        // The click switched to the Processes tab. The rest of the test needs the Performance tab.
        let tab = r.vcx.update(|_, cx| {
            main_window_view(cx)
                .expect("main window")
                .read(cx)
                .active_tab()
        });
        assert_eq!(tab, Tab::Processes);
        r.vcx
            .dispatch_action(tl_ui::actions::SelectTab(Tab::Performance as usize));
        draw(r.vcx);

        // Copy carries the rows.
        let page = overview(&mut r).expect("an OverviewPage");
        let text = r.vcx.update(|_, cx| {
            let mut out = String::new();
            page.read(cx).copy_all(&mut out, cx);
            out
        });
        assert!(text.contains("Top processes\n"), "{text}");
        assert!(text.contains("proc20\t20\t10%\n"), "{text}");

        set_top_procs(&mut r, false);
        assert!(procs_box(&mut r).is_none());
        assert!(r.vcx.debug_bounds(PROCS_BOX_ID).is_none());
        let d = merged(&mut r);
        assert!(has_overview_part(&d) && !d.process_top, "{d:?}");
    }
}

/// The box title line (UI-PRF-017, UI-PRF-058, tasklens-o55.54): icon, title, summary, the range
/// figures and the value on one line. A line too narrow for all of them leaves out the figures
/// first, then the summary; what shows is never cut.
mod title_row {
    use gpui_kit::{
        Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, Window, div, px, size,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_ui::pages::performance::overview::net_box::NET_BOX_ID;
    use tl_ui::pages::performance::overview::parts::{
        box_frame, figures_id, title_line_id, title_name_id, title_row_id, title_value_id,
    };
    use tl_ui::pages::performance::overview::range::RangeText;
    use tl_ui::theme::{current, metrics};

    const LONG: &str = "Realtek Semiconductor Co., Ltd. RTL8125 2.5GbE Controller";

    struct Frame {
        w: f32,
    }

    impl Render for Frame {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let m = *metrics(cx);
            let theme = current(cx).clone();
            let range = RangeText {
                figures: "min 2.2 Mbps  avg 10.9 Mbps  peak 50.2 Mbps".into(),
            };
            div().w(px(self.w)).h(px(200.)).child(box_frame(
                NET_BOX_ID,
                None,
                "Network".into(),
                "4.7 Mbps".into(),
                LONG.into(),
                &range,
                div().into_any_element(),
                false,
                &m,
                &theme,
            ))
        }
    }

    fn bounds(vcx: &mut VisualTestContext, id: String) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        vcx.debug_bounds(Box::leak(id.into_boxed_str()))
            .expect("drawn")
    }

    #[gpui_kit::test]
    fn title_line_drops_figures_then_summary(cx: &mut TestAppContext) {
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
        let (view, vcx) = cx.add_window_view(|_, _| Frame { w: 2000. });
        vcx.simulate_resize(size(px(2400.), px(300.)));
        let at = |vcx: &mut VisualTestContext, w: f32| {
            vcx.update(|_, cx| {
                view.update(cx, |f, cx| {
                    f.w = w;
                    cx.notify();
                })
            });
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            let row = bounds(vcx, title_row_id(NET_BOX_ID));
            let shown = |vcx: &mut VisualTestContext, id: String| {
                let b = bounds(vcx, id);
                assert!(
                    b.top() == row.top() || b.top() >= row.bottom(),
                    "{w}: {b:?} on the line {row:?} or under it"
                );
                b.top() == row.top()
            };
            let line = shown(vcx, title_line_id(NET_BOX_ID));
            let figures = shown(vcx, figures_id(NET_BOX_ID));
            let name = bounds(vcx, title_name_id(NET_BOX_ID));
            let value = bounds(vcx, title_value_id(NET_BOX_ID));
            for b in [name, value] {
                assert!(
                    b.top() >= row.top() && b.bottom() <= row.bottom(),
                    "{w}: {b:?} in the line {row:?}"
                );
            }
            (line, figures, row, name, value)
        };
        // Wide: everything on the line, the figures right before the value.
        let (line, figures, _, name, value) = at(vcx, 2000.);
        assert_eq!((line, figures), (true, true), "2000 px: all shown");
        let summary_w = bounds(vcx, title_line_id(NET_BOX_ID)).size.width;
        let figures_b = bounds(vcx, figures_id(NET_BOX_ID));
        assert!(
            figures_b.right() <= value.left() && value.left() - figures_b.right() <= px(9.),
            "figures {figures_b:?} before the value {value:?}"
        );
        // The width outside the summary and figures part: the line's ends and its two gaps.
        let middle = value.left() - name.right() - px(16.);
        let fixed = 2000. - f32::from(middle);
        let summary_w = f32::from(summary_w);
        let figures_w = f32::from(figures_b.size.width);
        // Room for the summary but not the figures: the figures go.
        let (line, figures, ..) = at(vcx, fixed + summary_w + figures_w / 2.);
        assert_eq!((line, figures), (true, false), "the figures go first");
        // Room for neither: the summary goes too; the title and value stay.
        let (line, figures, ..) = at(vcx, fixed + summary_w / 2.);
        assert_eq!((line, figures), (false, false), "then the summary");
    }
}

/// tasklens-crw.100: the whole-rows rule (UI-PRF-017) holds for every box kind and mode at every
/// size, and a value that does not fit drops decimals before it would lose its unit.
mod sweep {
    use std::sync::Arc;

    use gpui_kit::{
        AnyView, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Pixels,
        Render, SharedString, StyleRefinement, Styled as _, TestAppContext, VisualTestContext,
        Window, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
    use tl_model::ids::DeviceIdx;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
    use tl_model::system::{DiskTotals, GpuTotals, LinkState, Metric, NetTotals};
    use tl_model::validity::Validity;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::DeviceKey;
    use tl_ui::pages::performance::memory_apps;
    use tl_ui::pages::performance::overview::cpu_box::{
        CORE_CHARTS_ID, CPU_CHART_ID, CpuBox, POWER_STRIP_ID, core_chart_id,
    };
    use tl_ui::pages::performance::overview::disk_box::{self, DiskBox};
    use tl_ui::pages::performance::overview::fit::{
        FitTextPaint, fit_text, round_decimals, shorter_forms, take_fit_texts,
    };
    use tl_ui::pages::performance::overview::gpu_box::{self, GpuBox};
    use tl_ui::pages::performance::overview::layout::BoxKind;
    use tl_ui::pages::performance::overview::mem_box::{self, CHARTS_GRID_ID, MemBox};
    use tl_ui::pages::performance::overview::net_box::{self, NetBox};
    use tl_ui::pages::performance::overview::parts::{
        CORE_ROWS_SCROLL_ID, body_id, core_spark_id, take_core_labels, title_row_id,
    };
    use tl_ui::pages::performance::overview::procs_box::{PROC_ROW_ID_PREFIX, ProcsBox};
    use tl_ui::pages::performance::overview::timeline::{
        BANDS_ID, MARKER_ROW_ID, TICK_ROW_ID, TimelineBox,
    };
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;

    const GIB: u64 = 1 << 30;
    const CPUS: u16 = 8;

    #[test]
    fn overview_values_drop_decimals() {
        assert_eq!(
            round_decimals("3.98 GiB / 15.92 GiB", 1),
            "4.0 GiB / 15.9 GiB"
        );
        assert_eq!(round_decimals("3.98 GiB / 15.92 GiB", 0), "4 GiB / 16 GiB");
        assert_eq!(round_decimals("9.96 MiB/s", 1), "10.0 MiB/s");
        assert_eq!(round_decimals("1,999.5 KiB", 0), "2,000 KiB");
        assert_eq!(round_decimals("12%", 0), "12%");
        assert_eq!(round_decimals("v1. 2.5", 0), "v1. 3");
        assert_eq!(
            shorter_forms("3.98 GiB / 15.92 GiB"),
            ["4.0 GiB / 15.9 GiB", "4 GiB / 16 GiB"]
        );
        assert!(shorter_forms("55%").is_empty());
    }

    /// A value `fit_text` in a flex row `w` px wide.
    struct Probe(f32);

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().flex().w(px(self.0)).child(fit_text("12.5 GiB"))
        }
    }

    /// The text `fit_text` draws in a row `w` px wide, and its width.
    fn probe(cx: &mut TestAppContext, w: f32) -> Option<(SharedString, Pixels)> {
        let _ = take_fit_texts();
        let (_view, vcx) = cx.add_window_view(move |_, _| Probe(w));
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(600.), px(100.)));
        draw(vcx);
        take_fit_texts()
            .last()
            .map(|t| (t.text.clone(), t.bounds.size.width))
    }

    /// A value draws whole when it fits, without decimals when only that fits, and not at all
    /// when even that does not fit (crw.100).
    #[gpui_kit::test]
    fn overview_fit_text_drops_decimals(cx: &mut TestAppContext) {
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
        let (whole, w) = probe(cx, 300.).expect("drawn");
        assert_eq!(whole, "12.5 GiB");
        let (short, short_w) = probe(cx, f32::from(w) - 1.).expect("drawn shorter");
        assert_eq!(short, "13 GiB");
        assert!(short_w < w);
        assert_eq!(probe(cx, f32::from(short_w) - 1.), None);
    }

    /// One box of `w × h` at the window's top left.
    struct Host {
        boxed: Boxed,
        w: f32,
        h: f32,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div().w(px(self.w)).h(px(self.h)).child(
                    self.boxed
                        .view()
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
        }
    }

    fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("dev"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind,
            hot_c: None,
        }
    }

    /// CPU package 0 with logical CPUs at 100.., memory 1, a GPU at 10, disks at 20 to 22, eth0
    /// at 30, wlan0 at 33, the disk and net aggregates at 50 and 51.
    fn catalog() -> DeviceCatalog {
        let mut entries = vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            entry(
                10,
                DeviceIdentity::Gpu {
                    uuid: [1; 16],
                    pci_bus_id: Arc::from("0000:01:00.0"),
                },
                None,
            ),
        ];
        for (idx, minor, name) in [(20, 0, "sda"), (21, 16, "sdb"), (22, 32, "nvme0n1")] {
            entries.push(entry(
                idx,
                DeviceIdentity::Disk {
                    major: 8,
                    minor,
                    name: Arc::from(name),
                    wwn_or_serial: None,
                },
                None,
            ));
        }
        for (idx, ifindex, name, kind) in [
            (30, 2, "eth0", NetKind::Wired),
            (33, 4, "wlan0", NetKind::Wireless),
        ] {
            entries.push(entry(
                idx,
                DeviceIdentity::Net {
                    netns_ino: 1,
                    ifindex,
                    name: Arc::from(name),
                },
                Some(kind),
            ));
        }
        entries.push(entry(
            50,
            DeviceIdentity::Aggregate(AggregateKind::Disks),
            None,
        ));
        entries.push(entry(
            51,
            DeviceIdentity::Aggregate(AggregateKind::Nets),
            None,
        ));
        entries
            .extend((0..CPUS).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id }, None)));
        DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        }
    }

    /// A GPU with 3D, encode and decode engines and the owner's VRAM of 1.0.7: 3.98 of 15.92 GiB.
    fn gpu() -> GpuTotals {
        GpuTotals {
            device: DeviceIdx(10),
            util_pct: Metric::valid(25.),
            mem_util_pct: Metric::default(),
            mem_used: Metric::valid(398 * GIB / 100),
            mem_total: Metric::valid(1592 * GIB / 100),
            shared_used: Metric::default(),
            shared_total: Metric::default(),
            mem_reserved: Metric::default(),
            enc_pct: Metric::valid(10.),
            dec_pct: Metric::valid(5.),
            enc_sessions: Metric::default(),
            enc_dec_shared: false,
            temp_c: Metric::valid(62.),
            temp_edge_c: Metric::default(),
            temp_junction_c: Metric::default(),
            temp_mem_c: Metric::default(),
            power_w: Metric::valid(120.),
            power_limit_w: Metric::default(),
            clock_gfx_mhz: Metric::default(),
            clock_mem_mhz: Metric::default(),
            fan_pct: Metric::default(),
            pcie_rx_bps: Metric::default(),
            pcie_tx_bps: Metric::default(),
            engines: Default::default(),
            throttling: Default::default(),
            fdinfo_partial: false,
            energy_since_boot: Metric::default(),
            clock_video_mhz: Metric::default(),
            pstate: Metric::default(),
            clock_event_reasons: Metric::default(),
            fan_rpm: Metric::default(),
            pcie_gen: Metric::default(),
            pcie_width: Metric::default(),
            info: Arc::default(),
        }
    }

    fn disk(device: u16, read_bps: f64) -> (DeviceIdx, DiskTotals) {
        (
            DeviceIdx(device),
            DiskTotals {
                device: DeviceIdx(device),
                active_pct: Metric::valid(40.),
                read_bps: Metric::valid(read_bps),
                write_bps: Metric::valid(read_bps / 3.),
                response_ms: Metric::default(),
                read_total: 0,
                written_total: 0,
                capacity: 0,
                temp_c: Metric::default(),
            },
        )
    }

    fn net(device: u16, bps: f64) -> (DeviceIdx, NetTotals) {
        (
            DeviceIdx(device),
            NetTotals {
                device: DeviceIdx(device),
                rx_bps: Metric::valid(bps),
                tx_bps: Metric::valid(bps / 7.),
                rx_total: 123_456_789_012,
                tx_total: 9_876_543_210,
                link_speed_mbps: Metric::default(),
                state: LinkState::Up,
            },
        )
    }

    fn devices(kind: &str, idx: &[u16]) -> Vec<(DeviceKey, DeviceIdx, SharedString)> {
        idx.iter()
            .enumerate()
            .map(|(n, i)| {
                (
                    DeviceKey(Arc::from(format!("{kind}{n}"))),
                    DeviceIdx(*i),
                    SharedString::from(format!("{kind} {n}")),
                )
            })
            .collect()
    }

    #[derive(Clone)]
    enum Boxed {
        Cpu(Entity<CpuBox>),
        Mem(Entity<MemBox>),
        Gpu(Entity<GpuBox>),
        Disk(Entity<DiskBox>),
        Net(Entity<NetBox>),
        Procs(Entity<ProcsBox>),
        Timeline(Entity<TimelineBox>),
    }

    impl Boxed {
        fn view(&self) -> AnyView {
            match self {
                Self::Cpu(e) => e.clone().into(),
                Self::Mem(e) => e.clone().into(),
                Self::Gpu(e) => e.clone().into(),
                Self::Disk(e) => e.clone().into(),
                Self::Net(e) => e.clone().into(),
                Self::Procs(e) => e.clone().into(),
                Self::Timeline(e) => e.clone().into(),
            }
        }

        fn set_mode(&self, mode: &str, cx: &mut gpui_kit::App) {
            match self {
                Self::Cpu(e) => e.update(cx, |b, cx| b.set_mode(mode, cx)),
                Self::Mem(e) => e.update(cx, |b, cx| b.set_mode(mode, cx)),
                Self::Gpu(e) => e.update(cx, |b, cx| b.set_mode(mode, cx)),
                Self::Disk(e) => e.update(cx, |b, cx| b.set_mode(mode, cx)),
                Self::Net(e) => e.update(cx, |b, cx| b.set_mode(mode, cx)),
                Self::Procs(_) | Self::Timeline(_) => {}
            }
        }
    }

    /// The row, cell and part selectors of `kind` whose bounds the sweep checks.
    fn parts(kind: BoxKind) -> Vec<String> {
        let mut ids = vec![];
        match kind {
            BoxKind::Memory => {
                ids.extend([
                    mem_box::MEM_USED_CHART_ID.to_owned(),
                    mem_box::MEM_COMP_CHART_ID.to_owned(),
                    memory_apps::APPS_ID.to_owned(),
                    memory_apps::LEGEND_ID.to_owned(),
                ]);
                for i in 0..5 {
                    ids.extend([
                        mem_box::row_id(i),
                        mem_box::spark_id(i),
                        mem_box::value_id(i),
                        mem_box::meter_id(i),
                        mem_box::cell_id(i),
                        mem_box::cell_head_id(i, "percent"),
                    ]);
                }
            }
            BoxKind::Gpu => {
                ids.extend([
                    gpu_box::rows_id(0),
                    gpu_box::chart_id(0),
                    gpu_box::vram_id(0),
                    gpu_box::grid_id(0),
                ]);
                for key in ["gpu", "enc", "dec"] {
                    ids.push(gpu_box::engine_row_id(0, key));
                    ids.push(gpu_box::engine_cell_id(0, key));
                }
            }
            BoxKind::Disk => {
                ids.push(disk_box::TOTAL_CHART_ID.to_owned());
                for (n, _) in devices("disk", &[20, 21, 22]).iter().enumerate() {
                    let key = DeviceKey(Arc::from(format!("disk{n}")));
                    ids.push(format!("{}{}", disk_box::DISK_ROW_ID_PREFIX, key.0));
                    ids.push(disk_box::cell_id(&key));
                    ids.push(disk_box::head_id(&key, "active"));
                    for which in ["read", "write", "read-spark", "write-spark"] {
                        ids.push(disk_box::io_row_id(&key, which));
                    }
                }
            }
            BoxKind::Net => {
                ids.extend([
                    net_box::DOWN_RATE_ID.to_owned(),
                    net_box::UP_RATE_ID.to_owned(),
                    net_box::COMBINED_CHART_ID.to_owned(),
                    net_box::cell_id(DeviceIdx(30)),
                    net_box::cell_id(DeviceIdx(33)),
                ]);
            }
            BoxKind::Cpu => {
                ids.extend([CPU_CHART_ID.to_owned(), POWER_STRIP_ID.to_owned()]);
                for i in 0..usize::from(CPUS) {
                    ids.extend([core_spark_id(i), core_chart_id(i)]);
                }
            }
            BoxKind::TopProcs => {
                ids.extend(PROCS.map(|pid| format!("{PROC_ROW_ID_PREFIX}{pid}")));
            }
            BoxKind::Timeline => {
                ids.extend([MARKER_ROW_ID, BANDS_ID, TICK_ROW_ID].map(str::to_owned));
            }
        }
        ids
    }

    /// The name the sweep gives each core row the CPU box paints, as a part of its own.
    const CORE_ROWS: &str = "core rows";

    /// The pids of the process fixtures: the top 10 by CPU are 12 to 21.
    const PROCS: std::ops::Range<u32> = 10..22;

    /// The parts of `kind` that scroll their content by design (UI-PRF-019), each with the
    /// parts it holds. Only those may be clipped at its edge.
    fn scroll_areas(kind: BoxKind) -> Vec<(&'static str, Vec<String>)> {
        let cores = 0..usize::from(CPUS);
        match kind {
            BoxKind::Cpu => vec![
                (CORE_CHARTS_ID, cores.clone().map(core_chart_id).collect()),
                (
                    CORE_ROWS_SCROLL_ID,
                    cores
                        .map(core_spark_id)
                        .chain([CORE_ROWS.to_owned()])
                        .collect(),
                ),
            ],
            BoxKind::Memory => vec![(
                CHARTS_GRID_ID,
                (0..5)
                    .flat_map(|i| {
                        [
                            mem_box::cell_id(i),
                            mem_box::cell_head_id(i, "name"),
                            mem_box::cell_head_id(i, "value"),
                            mem_box::cell_head_id(i, "percent"),
                        ]
                    })
                    .collect(),
            )],
            _ => vec![],
        }
    }

    /// A drawn scroll area: its bounds and the parts it holds.
    struct Area {
        bounds: Bounds<Pixels>,
        content: Vec<&'static str>,
    }

    /// Checks one part `id` drawn at `b`: what shows of it lies inside `inner`. A part a scroll
    /// area holds shows only inside that area. Returns whether any of it shows.
    fn check_part(
        what: &str,
        id: &str,
        b: Bounds<Pixels>,
        inner: Bounds<Pixels>,
        areas: &[Area],
        failures: &mut Failures,
    ) -> bool {
        if b.size.height <= px(0.) || b.size.width <= px(0.) {
            return false;
        }
        let shown = match areas.iter().find(|a| a.content.contains(&id)) {
            Some(a) if !b.intersects(&a.bounds) => return false,
            Some(a) => b.intersect(&a.bounds),
            None => b,
        };
        if !inside(inner, shown) {
            failures.push(what, id, format!("{shown:?} outside {inner:?}"));
        }
        true
    }

    /// The failures by box, mode and kind of check: the first one and how many there were.
    #[derive(Default)]
    struct Failures(std::collections::BTreeMap<String, (String, usize)>);

    impl Failures {
        fn push(&mut self, what: &str, check: &str, detail: String) {
            let mode = what.split(" at ").next().unwrap_or(what);
            let e = self
                .0
                .entry(format!("{mode}: {check}"))
                .or_insert_with(|| (format!("{what}: {detail}"), 0));
            e.1 += 1;
        }
    }

    impl std::fmt::Display for Failures {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for (key, (first, n)) in &self.0 {
                writeln!(f, "{key}: {n} times, first {first}")?;
            }
            Ok(())
        }
    }

    fn leak(id: String) -> &'static str {
        Box::leak(id.into_boxed_str())
    }

    /// `text` with every number replaced by `#`: what a shorter form must keep.
    fn skeleton(text: &str) -> String {
        let mut out = String::new();
        let mut in_number = false;
        for c in text.chars() {
            let digit = c.is_ascii_digit() || (in_number && (c == '.' || c == ','));
            if digit && !in_number {
                out.push('#');
            } else if !digit {
                out.push(c);
            }
            in_number = digit;
        }
        out
    }

    fn inside(inner: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
        let e = px(0.01);
        b.left() >= inner.left() - e
            && b.right() <= inner.right() + e
            && b.top() >= inner.top() - e
            && b.bottom() <= inner.bottom() + e
    }

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
    }

    /// For every box kind and every mode, at 600 x 400 px and at heights from the kind minimum
    /// down to 0 in 2 px steps and widths 200 to 600 px in 20 px steps: no row or text crosses
    /// the bottom of the box body or the box edge, and every drawn value keeps its unit
    /// (UI-PRF-017, UI-PRF-019).
    #[gpui_kit::test]
    fn overview_boxes_never_draw_below_body(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(catalog());
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.memory.validity = Validity::Valid;
            sys.memory.total = 64 * GIB;
            sys.memory.used = 23 * GIB;
            sys.memory.available = 41 * GIB;
            sys.memory.cached = 6 * GIB;
            sys.memory.swap_total = 8 * GIB;
            sys.memory.swap_free = 7 * GIB;
            sys.memory.commit_limit = 40 * GIB;
            sys.memory.committed = 31 * GIB;
            sys.gpus = Arc::from([(DeviceIdx(10), gpu())]);
            sys.disks = Arc::from([
                disk(20, 123_456_789.),
                disk(21, 9_876.),
                disk(22, 54_321_000.),
            ]);
            sys.nets = Arc::from([net(30, 12_345_678.), net(33, 98_765.)]);
            s.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version: s.processes.header.version + 1,
                    ..ComponentHeader::default()
                },
                records: PROCS
                    .map(|pid| super::wired::proc_record(pid, (pid - 10) as f32))
                    .collect(),
                ..ProcessSnapshot::default()
            });
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            init_strings(cx, "en-US");
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let mut failures = Failures::default();
        // How many rows and how many body values (not empty, not in the title row) each box
        // mode drew over the sweep.
        let mut drawn = std::collections::BTreeMap::<String, (usize, usize)>::new();
        for kind in BoxKind::ALL {
            let hub = hub.clone();
            let (host, vcx) = cx.add_window_view(move |_, cx| {
                let boxed = match kind {
                    BoxKind::Cpu => Boxed::Cpu(cx.new(|cx| CpuBox::new(&hub, DeviceIdx(0), cx))),
                    BoxKind::Memory => Boxed::Mem(cx.new(|cx| MemBox::new(&hub, DeviceIdx(1), cx))),
                    BoxKind::Gpu => {
                        Boxed::Gpu(cx.new(|cx| GpuBox::new(&hub, &devices("gpu", &[10]), cx)))
                    }
                    BoxKind::Disk => Boxed::Disk(
                        cx.new(|cx| DiskBox::new(&hub, &devices("disk", &[20, 21, 22]), cx)),
                    ),
                    BoxKind::Net => Boxed::Net(cx.new(|cx| NetBox::new(&hub, cx))),
                    BoxKind::TopProcs => Boxed::Procs(cx.new(|cx| ProcsBox::new(&hub, cx))),
                    BoxKind::Timeline => Boxed::Timeline(cx.new(|cx| TimelineBox::new(&hub, cx))),
                };
                Host {
                    boxed,
                    w: 400.,
                    h: 400.,
                }
            });
            vcx.simulate_scale_factor_change(1.0);
            let driver = vcx.update(|window, cx| {
                let driver = cx.new(AnimationDriver::new);
                AnimationDriver::register(&driver, window, cx);
                driver
            });
            vcx.simulate_resize(size(px(700.), px(400.)));
            draw(vcx);
            let frame_id = leak(format!("perf-overview-box-{}", kind.key()));
            let body_sel = leak(body_id(frame_id));
            let title_id = leak(title_row_id(frame_id));
            let part_ids: Vec<&'static str> = parts(kind).into_iter().map(leak).collect();
            let area_ids: Vec<(&'static str, Vec<&'static str>)> = scroll_areas(kind)
                .into_iter()
                .map(|(id, content)| (id, content.into_iter().map(leak).collect()))
                .collect();
            for mode in kind.modes() {
                vcx.update(|_, cx| {
                    let boxed = host.read(cx).boxed.clone();
                    boxed.set_mode(mode.id, cx);
                });
                // A tall box first, so each mode shows its body values once: at the kind minimum
                // the part minimums can leave a text part no room for a whole row. Then the
                // sweep.
                let sweep = (0..=kind.min_h() as u32 / 2).rev().flat_map(|n| {
                    (200..=600u32)
                        .step_by(20)
                        .map(move |w| (w as f32, n as f32 * 2.))
                });
                for (w, h) in std::iter::once((600., 400.)).chain(sweep) {
                    vcx.update(|_, cx| {
                        host.update(cx, |host, cx| {
                            host.w = w;
                            host.h = h;
                            cx.notify();
                        })
                    });
                    draw(vcx);
                    // Only the settled frame counts: the first frame after a resize may
                    // still use the old size class. A refresh paints every view again, so
                    // each value is logged once.
                    let _ = take_fit_texts();
                    let _ = take_core_labels();
                    vcx.update(|window, cx| {
                        window.refresh();
                        window.draw(cx).clear(cx);
                    });
                    let texts = take_fit_texts();
                    let core_rows = take_core_labels();
                    let what = format!("{} {} at {w} x {h}", kind.key(), mode.id);
                    let Some(frame) = vcx.debug_bounds(frame_id) else {
                        failures.push(&what, "no box", String::new());
                        continue;
                    };
                    // Inside the 1 px border, above the body's bottom.
                    let mut inner = Bounds::new(
                        frame.origin + gpui_kit::point(px(1.), px(1.)),
                        size(frame.size.width - px(2.), frame.size.height - px(2.)),
                    );
                    if let Some(body) = vcx.debug_bounds(body_sel) {
                        let bottom = inner.bottom().min(body.bottom());
                        inner.size.height = (bottom - inner.top()).max(px(0.));
                    }
                    let title = vcx.debug_bounds(title_id);
                    let areas: Vec<Area> = area_ids
                        .iter()
                        .filter_map(|(id, content)| {
                            Some(Area {
                                bounds: vcx.debug_bounds(id)?,
                                content: content.clone(),
                            })
                        })
                        .collect();
                    for area in &areas {
                        let a = area.bounds;
                        if a.size.height > px(0.) && !inside(inner, a) {
                            failures.push(&what, "scroll area", format!("{a:?} outside {inner:?}"));
                        }
                    }
                    let mut rows = 0;
                    for id in &part_ids {
                        let Some(b) = vcx.debug_bounds(id) else {
                            continue;
                        };
                        rows += usize::from(check_part(&what, id, b, inner, &areas, &mut failures));
                    }
                    // A core row runs from the core's name to its percent, one row high.
                    for c in &core_rows {
                        let right = c.pct_right.max(c.name.x);
                        let b = Bounds::from_corners(
                            c.name,
                            gpui_kit::point(right, c.name.y + c.row_h),
                        );
                        rows += usize::from(check_part(
                            &what,
                            CORE_ROWS,
                            b,
                            inner,
                            &areas,
                            &mut failures,
                        ));
                    }
                    let area_bounds: Vec<Bounds<Pixels>> = areas.iter().map(|a| a.bounds).collect();
                    let values = texts
                        .iter()
                        .filter(|t| {
                            !t.text.is_empty()
                                && t.bounds.intersects(&t.mask)
                                && title.is_none_or(|r| t.bounds.top() >= r.bottom())
                        })
                        .count();
                    let e = drawn
                        .entry(what.split(" at ").next().unwrap_or("").to_owned())
                        .or_default();
                    e.0 += rows;
                    e.1 += values;
                    for t in &texts {
                        check_text(&what, t, inner, title, &area_bounds, &mut failures);
                    }
                }
            }
            drop(driver);
        }
        assert!(failures.0.is_empty(), "{failures}");
        // The checks ran: every mode drew rows, and every mode whose body shows values drew
        // body values. The CPU, Top processes and Timeline bodies and the one-chart modes show
        // none.
        for kind in BoxKind::ALL {
            for mode in kind.modes() {
                let with_values = matches!(
                    (kind, mode.id),
                    (BoxKind::Memory, "rows" | "chart_rows" | "charts")
                        | (BoxKind::Gpu | BoxKind::Net, _)
                        | (BoxKind::Disk, "rows" | "disk_charts")
                );
                let key = format!("{} {}", kind.key(), mode.id);
                let (rows, values) = drawn.get(&key).copied().unwrap_or_default();
                assert!(rows > 0, "{key} drew no rows: {drawn:?}");
                assert!(
                    !with_values || values > 0,
                    "{key} drew no body values: {drawn:?}"
                );
            }
        }
    }

    /// One drawn value: it keeps its unit, and it lies inside the box body unless it is in the
    /// title row (checked across only) or clipped by a part that scrolls.
    fn check_text(
        what: &str,
        t: &FitTextPaint,
        inner: Bounds<Pixels>,
        title: Option<Bounds<Pixels>>,
        areas: &[Bounds<Pixels>],
        failures: &mut Failures,
    ) {
        let e = px(0.01);
        // Scrolled out of view: nothing of it shows.
        if !t.bounds.intersects(&t.mask) {
            return;
        }
        if skeleton(&t.text) != skeleton(&t.whole) {
            failures.push(
                what,
                "unit",
                format!("{:?} drawn for {:?}", t.text, t.whole),
            );
        }
        let b = t.bounds;
        // Across, a value is never cut by any clip: it keeps its unit.
        if b.left() < t.mask.left() - e || b.right() > t.mask.right() + e {
            failures.push(
                what,
                "cut",
                format!("{:?} at {b:?} by {:?}", t.text, t.mask),
            );
        }
        if b.left() < inner.left() - e || b.right() > inner.right() + e {
            failures.push(what, "edge", format!("{:?} at {b:?} in {inner:?}", t.text));
        }
        if title.is_some_and(|r| b.top() < r.bottom()) {
            return;
        }
        let clipped = b.top() < t.mask.top() - e || b.bottom() > t.mask.bottom() + e;
        if clipped && areas.iter().any(|a| inside(*a, t.mask)) {
            return;
        }
        if !inside(inner, b) {
            failures.push(
                what,
                "text",
                format!("{:?} at {b:?} outside {inner:?}", t.text),
            );
        }
    }
}

/// UI-PRF-060: the Overview Memory box in its "By app" mode: where the legend goes, the legend
/// click, and the 24 h range from the device metric history.
mod mem_by_app_box {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
        MouseUpEvent, ParentElement as _, Render, StyleRefinement, Styled as _, TestAppContext,
        VisualTestContext, Window, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, OverviewRange, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{ComponentHeader, DeviceHistory, HistPoint, SeriesSet};
    use tl_model::system::MemGroupBand;
    use tl_model::time::now_boot_ns;
    use tl_model::validity::Validity;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::memory_apps::{
        APPS_ID, BandKind, LEGEND_ID, MemoryAppsChart, overview_legend,
    };
    use tl_ui::pages::performance::overview::mem_box::{MEM_APPS_ID, MemBox};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::theme::metrics;

    const MIB: f32 = 1_048_576.;
    const GIB: u64 = 1 << 30;
    const S: u64 = 1_000_000_000;
    const MEMORY: DeviceIdx = DeviceIdx(1);
    const OTHER: DeviceIdx = DeviceIdx(20);
    const SHARED: DeviceIdx = DeviceIdx(21);
    const GROUPS: u16 = 4;

    fn group_dev(i: u16) -> DeviceIdx {
        DeviceIdx(10 + i)
    }

    fn entry(idx: DeviceIdx, identity: DeviceIdentity, name: &str) -> DeviceEntry {
        DeviceEntry {
            idx,
            identity,
            generation: 0,
            display_name: Arc::from(name),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        }
    }

    fn mem_group(idx: DeviceIdx, key: &str, name: &str) -> DeviceEntry {
        entry(
            idx,
            DeviceIdentity::MemGroup {
                key: Arc::from(key),
            },
            name,
        )
    }

    fn sid(device: DeviceIdx) -> SeriesId {
        SeriesId {
            device,
            metric: MetricId::MEM_GROUP,
        }
    }

    /// 60 live samples of `value`, one a second up to now.
    fn flat(device: DeviceIdx, value: f32) -> Arc<SeriesView> {
        let now = now_boot_ns();
        Arc::new(SeriesView {
            id: sid(device),
            version: 1,
            period_ns: S,
            samples: (0..60u64)
                .map(|k| Sample::valid(now - (59 - k) * S, value))
                .collect::<Vec<_>>()
                .into(),
        })
    }

    struct Host {
        mem: Entity<MemBox>,
        w: f32,
        h: f32,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div().w(px(self.w)).h(px(self.h)).child(
                    self.mem
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
    }

    struct Rig<'a> {
        engine: Arc<FakeEngine>,
        host: Entity<Host>,
        apps: Entity<MemoryAppsChart>,
        vcx: &'a mut VisualTestContext,
    }

    impl Rig<'_> {
        fn resize(&mut self, w: f32, h: f32) {
            self.host.update(self.vcx, |host, cx| {
                host.w = w;
                host.h = h;
                cx.notify();
            });
            draw(self.vcx);
        }

        fn bounds(&mut self, id: String) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
            self.vcx.debug_bounds(Box::leak(id.into_boxed_str()))
        }

        fn click(&mut self, id: String) {
            let at = self.bounds(id.clone()).expect(&id).center();
            self.vcx.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            });
            self.vcx.simulate_event(MouseUpEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
            });
            draw(self.vcx);
        }

        fn last_open(&mut self) -> Option<String> {
            self.vcx.read_entity(&self.apps, |a, _| {
                a.last_open.as_ref().map(|o| o.0.to_string())
            })
        }
    }

    /// A Memory box in "By app" mode, `w` × `h`, at the 1 min range. Groups p0 to p3 hold 4 to
    /// 1 MiB live with 3 processes each, "Other apps" 1 MiB and "Shared and kernel" 2 MiB.
    fn open(cx: &mut TestAppContext, w: f32, h: f32) -> Rig<'_> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            let mut entries = vec![entry(MEMORY, DeviceIdentity::Memory, "Memory")];
            let mut views = Vec::new();
            let mut bands = Vec::new();
            for i in 0..GROUPS {
                let key = format!("exe:p{i}");
                entries.push(mem_group(group_dev(i), &key, &format!("p{i}")));
                views.push(flat(group_dev(i), f32::from(GROUPS - i) * MIB));
                bands.push(MemGroupBand {
                    key: Arc::from(key),
                    name: Arc::from(format!("p{i}")),
                    process_count: 3,
                    device: group_dev(i),
                    wine: None,
                });
            }
            entries.push(mem_group(OTHER, "other", "Other apps"));
            entries.push(mem_group(SHARED, "shared", "Shared and kernel"));
            views.push(flat(OTHER, MIB));
            views.push(flat(SHARED, 2. * MIB));
            views.sort_by_key(|v| v.id);
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: entries.into(),
            });
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version: 1,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.memory.validity = Validity::Valid;
            sys.memory.total = 64 * GIB;
            sys.memory.used = 23 * GIB;
            sys.memory.available = 41 * GIB;
            sys.memory.groups = Arc::new(bands);
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            init_strings(cx, "en-US");
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            let mut settings = Settings::default();
            settings.performance.overview_range = OverviewRange::M1;
            cx.set_global(BootSettings {
                settings,
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let (host, vcx) = cx.add_window_view(move |_, cx| {
            let mem = cx.new(|cx| MemBox::new(&hub, MEMORY, cx));
            mem.update(cx, |b, cx| b.set_mode("apps", cx));
            Host { mem, w, h }
        });
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1000.), px(700.)));
        draw(vcx);
        let apps = vcx.update(|_, cx| {
            let mem = host.read(cx).mem.clone();
            mem.read(cx).apps().cloned().expect("the By app chart")
        });
        Rig {
            engine,
            host,
            apps,
            vcx,
        }
    }

    fn kinds(r: &mut Rig) -> Vec<BandKind> {
        r.vcx
            .read_entity(&r.apps, |a, _| a.bands().iter().map(|b| b.kind).collect())
    }

    /// The legend sits at the right of the chart in a body 480 px or wider, and below it in a
    /// narrower one, where it takes at most half the body in whole rows with "+n more".
    #[gpui_kit::test]
    fn overview_memory_by_app_legend_place(cx: &mut TestAppContext) {
        let mut r = open(cx, 700., 400.);
        let m = r.vcx.update(|_, cx| *metrics(cx));
        assert!(overview_legend(size(px(479.), px(300.)), 6, &m).0);
        assert!(!overview_legend(size(px(480.), px(300.)), 6, &m).0);
        assert_eq!(
            overview_legend(size(px(400.), px(100.)), 6, &m),
            (true, 1, 5)
        );

        let groups = vec![BandKind::Group(0); usize::from(GROUPS)];
        let kinds_now = kinds(&mut r);
        assert_eq!(kinds_now.len(), groups.len() + 2);
        assert_eq!(kinds_now[4..], [BandKind::Other, BandKind::Shared]);
        let body = r.bounds(MEM_APPS_ID.into()).expect("the By app part");
        assert!(body.size.width >= px(480.), "{body:?}");
        let chart = r.bounds(APPS_ID.into()).expect("chart");
        let legend = r.bounds(LEGEND_ID.into()).expect("legend");
        assert!(legend.left() >= chart.right(), "{chart:?} {legend:?}");
        assert!(legend.right() <= body.right() + px(0.5));
        for row in 0..6 {
            assert!(
                r.bounds(format!("{LEGEND_ID}-{row}")).is_some(),
                "row {row}"
            );
        }

        r.resize(400., 400.);
        let body = r.bounds(MEM_APPS_ID.into()).expect("the By app part");
        assert!(body.size.width < px(480.), "{body:?}");
        let chart = r.bounds(APPS_ID.into()).expect("chart");
        let legend = r.bounds(LEGEND_ID.into()).expect("legend");
        assert!(legend.top() >= chart.bottom(), "{chart:?} {legend:?}");
        assert!(legend.bottom() <= body.bottom() + px(0.5));
        assert!(legend.size.height <= body.size.height / 2. + px(0.5));

        // A low box: the legend below shows whole rows and "+n more".
        r.resize(400., 180.);
        let body = r.bounds(MEM_APPS_ID.into()).expect("the By app part");
        let legend = r.bounds(LEGEND_ID.into()).expect("legend");
        assert!(legend.size.height <= body.size.height / 2. + px(0.5));
        let more = r.bounds(format!("{LEGEND_ID}-more")).expect("+n more row");
        assert!(more.bottom() <= legend.bottom() + px(0.5));
        assert!(r.bounds(format!("{LEGEND_ID}-5")).is_none());
    }

    /// A group's legend row opens Processes filtered to that group; "Other apps" and "Shared
    /// and kernel" open nothing (UI-PRF-060, leader note 07:15).
    #[gpui_kit::test]
    fn overview_memory_by_app_legend_click(cx: &mut TestAppContext) {
        let mut r = open(cx, 700., 400.);
        // Rows: Other, p3, p2, p1, p0, then the kernel band Shared under the divider.
        r.click(format!("{LEGEND_ID}-0"));
        assert_eq!(r.last_open(), None);
        r.click(format!("{LEGEND_ID}-5"));
        assert_eq!(r.last_open(), None);
        r.click(format!("{LEGEND_ID}-1"));
        assert_eq!(r.last_open().as_deref(), Some("exe:p3"));
        let name = r.vcx.read_entity(&r.apps, |a, _| {
            a.last_open.as_ref().map(|o| o.1.to_string())
        });
        assert_eq!(name.as_deref(), Some("p3"));
        r.click(format!("{LEGEND_ID}-4"));
        assert_eq!(r.last_open().as_deref(), Some("exe:p0"));
    }

    /// At 24 h the chart is a box chart over 24 hours, and the ranking and the legend values
    /// read the device metric history, not the live series.
    #[gpui_kit::test]
    fn overview_memory_by_app_reads_history(cx: &mut TestAppContext) {
        let mut r = open(cx, 700., 400.);
        let bottom = |r: &mut Rig| {
            r.vcx
                .read_entity(&r.apps, |a, _| a.bands()[0].key.to_string())
        };
        assert_eq!(bottom(&mut r), "exe:p0");
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.performance.overview_range = OverviewRange::H24;
            });
        });
        draw(r.vcx);
        let range = r.vcx.update(|_, cx| {
            let apps = r.apps.read(cx);
            apps.chart().read(cx).spec.range_ns
        });
        assert_eq!(range, 24 * 3600 * S);

        // The history has the order the other way round: p3 holds 13 MiB, p0 10 MiB.
        let now = i64::try_from(jiff::Timestamp::now().as_nanosecond()).expect("fits");
        let step = 60 * S;
        let points = |mean: f32| -> Arc<[HistPoint]> {
            (0..30i64)
                .map(|k| HistPoint {
                    t_realtime_ns: now - (30 - k) * step as i64,
                    mean,
                    min: mean,
                    max: mean,
                })
                .collect()
        };
        let mut series: Vec<(SeriesId, Arc<[HistPoint]>)> = (0..GROUPS)
            .map(|i| (sid(group_dev(i)), points(f32::from(10 + i) * MIB)))
            .collect();
        series.push((sid(OTHER), points(3. * MIB)));
        series.push((sid(SHARED), points(5. * MIB)));
        series.sort_by_key(|(id, _)| *id);
        r.engine.publish_with(|s| {
            let version = s.device_history.header.version + 1;
            s.device_history = Arc::new(DeviceHistory {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                request: Some(DeviceHistoryRequest {
                    range: DeviceRange::H24,
                    end_realtime_ns: None,
                }),
                realtime_offset_ns: now - now_boot_ns() as i64,
                since_realtime_ns: Some(now - 30 * step as i64),
                step_ns: step,
                series: series.into(),
                boots: Arc::from([]),
                timeline: None,
            });
        });
        draw(r.vcx);
        assert_eq!(bottom(&mut r), "exe:p3");
        let values: Vec<(String, f64)> = r.vcx.read_entity(&r.apps, |a, _| {
            a.bands()
                .iter()
                .map(|b| (b.key.to_string(), b.value))
                .collect()
        });
        let value = |key: &str| values.iter().find(|v| v.0 == key).map(|v| v.1);
        assert_eq!(value("exe:p3"), Some(f64::from(13. * MIB)));
        assert_eq!(value("exe:p0"), Some(f64::from(10. * MIB)));
        assert_eq!(value("shared"), Some(f64::from(5. * MIB)));
        assert_eq!(value("other"), Some(f64::from(3. * MIB)));
    }
}
