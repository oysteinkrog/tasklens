//! Performance tab Overview page (E24, UI-PRF-017, D-022): the drawing parts and the boxes, moved
//! from the tl-ui `ui` binary (tasklens-zljr5.41). The OverviewPage tests with the device sidebar
//! are `overview::page` in the tl-ui-perf `perf` binary; the page inside the main window is
//! `overview::wired` in tl-ui.

use gpui_kit::px;
use tl_ui_overview::overview::parts::{
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

/// D-029, UI-PRF-059, D-040: 18 px rows in the fewest columns that fit them, at most 4, grown
/// to fill the height, then the rows scroll.
#[test]
fn core_bars_layout_32_cores() {
    let l = core_bars_layout(32, px(776.), px(600.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (1, 32, px(600. / 32.)));
    assert!(!l.scrolls);
    // 32 rows of 18 px need 576 px: a lower part takes the fewest columns that fit.
    let l = core_bars_layout(32, px(776.), px(520.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (2, 16, px(520. / 16.)));
    assert!(!l.scrolls);
    let l = core_bars_layout(32, px(776.), px(400.), 1.);
    assert_eq!((l.cols, l.rows, l.row_h), (2, 16, px(400. / 16.)));
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

/// UI-PRF-059, D-040: core rows are at least 18 px apart at every part height, core count and
/// scale; rows that fit grow to fill the part's height, and scrolling rows keep 18 px.
#[test]
fn core_rows_fill_the_height_and_never_shrink() {
    for scale in [1., 1.5] {
        for n in [1, 4, 8, 12, 16, 24, 32, 64, 128] {
            for h in (0..=1200).step_by(7) {
                let l = core_bars_layout(n, px(776.), px(h as f32), scale);
                let at = format!("{n} cores in {h} px at {scale}x: {l:?}");
                assert!(l.row_h >= px(18. * scale - 1e-3), "{at}");
                if l.scrolls {
                    assert_eq!(l.row_h, px(18. * scale), "{at}");
                } else {
                    assert!((f32::from(l.content_h()) - h as f32).abs() < 1e-2, "{at}");
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
/// beside the 40% chart is 750 × 0.6 − 12 = 438 px wide: 2 columns of 215 px, 16 rows that
/// share the 456 px (D-040).
#[test]
fn core_rows_fit_32_threads_at_1200x900() {
    let l = core_bars_layout(32, px(438.), px(456.), 1.);
    assert_eq!((l.cols, l.rows), (2, 16));
    assert_eq!(l.cell_w, px(215.));
    assert_eq!(l.row_h, px(456. / 16.));
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

mod drawn {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        App, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, Window, div, hsla, px, size,
    };
    use tl_ui_overview::overview::mem_box::{METER_ROW_LABEL_ID, MeterRow, meter_row};
    use tl_ui_overview::overview::parts::{
        CoreBars, CoreBarsModel, METER_H, Meter, take_meter_quads,
    };
    use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};

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
                    let m = *tl_ui_base::theme::metrics(cx);
                    let theme = tl_ui_base::theme::current(cx).clone();
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
        tl_ui_base::theme::store::init(
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
    use tl_model::snapshot::{ComponentHeader, SeriesSet, Snapshot};
    use tl_model::system::Metric;
    use tl_model::time::now_boot_ns;
    use tl_model::validity::Validity;
    use tl_ui_base::anim::AnimationDriver;
    use tl_ui_base::app_env::{BootSettings, EngineGlobal};
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::TlSettings;
    use tl_ui_chart::chart::labels::LabelKind;
    use tl_ui_overview::overview::cpu_box::{CPU_BOX_ID, CpuBox};
    use tl_ui_overview::overview::cpu_flow::cores_of;
    use tl_ui_overview::overview::mem_box::{MEM_BOX_ID, MemBox};
    use tl_ui_overview::overview::parts::{CoreCellFit, core_cell, core_spark_id};
    use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

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
            tl_ui_base::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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

        /// (render, notify) of the CPU box, then of the Memory box. The CPU notify counts the
        /// box and its cores group: a core sample notifies only the group that draws the rows.
        fn counts(&mut self) -> [u32; 4] {
            let (cpu, mem) = (self.cpu(), self.mem());
            self.vcx.update(|_, cx| {
                let (c, m) = (cpu.read(cx), mem.read(cx));
                let cores = cores_of(c.flow())
                    .and_then(|id| c.container().read(cx).group(id).cloned())
                    .map_or(0, |g| g.read(cx).notify_count.get());
                [
                    c.render_count.get(),
                    c.notify_count.get() + cores,
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
        use tl_ui_overview::overview::mem_box::{
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
        assert!(layout.row_h >= px(18.), "{layout:?}");
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
            // The sparkline is 48 px wide and fills its row less a 2 px inset (D-040).
            let row_h = layout.row_h;
            // Layout rounds to whole pixels.
            assert_eq!(b.size.width, px(48.), "core {i} sparkline width");
            let want = f32::from(row_h) - 4.;
            assert!(
                (f32::from(b.size.height) - want).abs() <= 1.,
                "core {i} sparkline height {:?}, want {want}",
                b.size.height
            );
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
                let step = f32::from(b.top() - above.top());
                assert!(
                    (step - f32::from(row_h)).abs() <= 1.,
                    "core {i} one row lower: {step}"
                );
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
        use tl_ui_overview::overview::parts::{body_id, take_meter_quads};
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
        use tl_ui_overview::overview::parts::{
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
        assert_eq!((layout.cols, layout.rows), (2, 16));
        assert!(
            layout.row_h > px(18.),
            "rows fill the part (D-040): {layout:?}"
        );
        let row_h = layout.row_h;
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
                l.name.y + row_h <= frame.bottom() + px(0.01),
                "C{} inside the box",
                l.core
            );
            let x = first.name.x + (layout.cell_w + px(8.)) * col as f32;
            assert_eq!(l.name.x, x, "C{} in column {col}", l.core);
            assert_eq!(l.name.y, first.name.y + row_h * row as f32, "C{}", l.core);
            assert_eq!(l.pct_right, l.name.x + parts.pct_right);
        }

        // Name, sparkline, bar, percent from left to right in every row.
        let quads = take_meter_quads();
        for l in &labels {
            let id: &'static str = Box::leak(core_spark_id(l.core).into_boxed_str());
            let spark = r.vcx.debug_bounds(id).expect("sparkline drawn");
            assert!(spark.top() >= l.name.y && spark.bottom() <= l.name.y + row_h + px(0.01));
            assert_eq!(spark.left(), l.name.x + parts.spark_x, "C{}", l.core);
            let track = quads
                .iter()
                .find(|q| q.left() == l.name.x + parts.meter_x && q.top() > l.name.y)
                .filter(|q| q.bottom() < l.name.y + row_h)
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
                    tl_ui_settings::settings::CoreSmoothing::Off;
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
    use tl_model::snapshot::{
        ComponentHeader, DeviceDetailsSnapshot, DiskDetails, GraphicsAdapter, InvField, SeriesSet,
        Snapshot, SystemSnapshot,
    };
    use tl_model::system::{DiskTotals, GpuEngine, GpuTotals, LinkState, Metric, NetTotals};
    use tl_model::table::Total;
    use tl_model::time::now_boot_ns;
    use tl_model::units;
    use tl_model::validity::Validity;
    use tl_ui_base::anim::AnimationDriver;
    use tl_ui_base::app_env::{BootSettings, EngineGlobal};
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::{SettingKey, SettingValue, TlSettings};
    use tl_ui_overview::overview::disk_box::{DISK_ROW_ID_PREFIX, DiskBox, DiskRow, io_row_id};
    use tl_ui_overview::overview::gpu_box::{
        GPU_BOX_ID, GpuBox, SIDE_ROWS_MIN_W, chart_id, engine_cell_id, engine_row_id, grid_id,
        rows_id, vram_id,
    };
    use tl_ui_overview::overview::layout::Arrange;
    use tl_ui_overview::overview::net_box::NetBox;
    use tl_ui_overview::overview::parts::{BOX_CHROME, GridFit, body_id, fill_grid};
    use tl_ui_perf_core::core::DeviceKey;
    use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

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
            tl_ui_base::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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

    /// The GPU box's widget slots follow the GPUs it shows: the first two visible GPUs, so a
    /// hidden GPU is skipped and a third is never placed (tasklens-lhliq.7).
    #[gpui_kit::test]
    fn overview_gpu_widget_slots_follow_shown_gpus(cx: &mut TestAppContext) {
        use tl_ui_settings::layout::DeviceSelector;

        let shown = |idx: &[u16]| -> Devices {
            idx.iter()
                .map(|i| {
                    let id = gpu_id((*i - 10) as u8);
                    (
                        DeviceKey::from_identity(&id).expect("GPU key"),
                        DeviceIdx(*i),
                        SharedString::from(format!("GPU {i}")),
                    )
                })
                .collect()
        };
        let key = |i: u16| {
            DeviceKey::from_identity(&gpu_id((i - 10) as u8))
                .expect("key")
                .0
        };
        let mut r = open(cx, shown(&[10, 11, 12]), Vec::new(), (450., 200.));
        let (gpu, _, _) = r.boxes();
        let pinned_keys = |r: &mut Rig| {
            r.vcx.update(|window, cx| {
                gpu.update(cx, |g, cx| g.prepare_edit(cx));
                let editor = gpu.read(cx).editor().clone();
                editor.update(cx, |e, cx| e.start(window, cx));
            });
            draw(r.vcx);
            let editor = r.vcx.update(|_, cx| gpu.read(cx).editor().clone());
            let working = r
                .vcx
                .update(|_, cx| editor.read(cx).working_layout(cx))
                .expect("Edit Widgets is on");
            r.vcx
                .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
            draw(r.vcx);
            let mut keys: Vec<std::sync::Arc<str>> = working
                .widgets
                .iter()
                .filter_map(|w| match w.content.device() {
                    Some(DeviceSelector::Key(k)) => Some(k.clone()),
                    _ => None,
                })
                .collect();
            keys.sort();
            keys.dedup();
            keys
        };
        let mut want = vec![key(10), key(11)];
        want.sort();
        assert_eq!(pinned_keys(&mut r), want, "three GPUs: the first two only");

        // GPU 11 hidden: slot 1 is GPU 12.
        r.vcx
            .update(|_, cx| gpu.update(cx, |g, cx| g.set_gpus(&shown(&[10, 12]), cx)));
        draw(r.vcx);
        let mut want = vec![key(10), key(12)];
        want.sort();
        assert_eq!(pinned_keys(&mut r), want, "the hidden GPU is skipped");

        // One GPU: slot 1 is empty, so none of its widgets is pinned.
        r.vcx
            .update(|_, cx| gpu.update(cx, |g, cx| g.set_gpus(&shown(&[12]), cx)));
        draw(r.vcx);
        assert_eq!(pinned_keys(&mut r), vec![key(12)]);
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

    /// tasklens-lhliq.7, ADR-019 "Flow placement": a collapsed section draws only its busiest
    /// engine's widget, so the box places its widgets again when another engine becomes the
    /// busiest, and at no other time. A sample that keeps the busiest engine places nothing
    /// and keeps every widget entity.
    #[gpui_kit::test]
    fn gpu_box_collapsed_places_again_only_for_a_new_busiest(cx: &mut TestAppContext) {
        use tl_ui_overview::overview::gpu_flow::engine_widget;

        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (box_h(152.), 200.));
        let totals = |gpu: f32, dec: f32| {
            let mut t = gpu_engines(false);
            t.util_pct = Metric::valid(gpu);
            t.dec_pct = Metric::valid(dec);
            t
        };
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), totals(25., 50.))]));
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let state = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                let c = g.container().read(cx);
                let mut ids: Vec<(u32, gpui_kit::EntityId)> = c
                    .placed_ids()
                    .iter()
                    .map(|id| (*id, c.widget(*id).expect("entity").entity_id()))
                    .collect();
                ids.sort_by_key(|(id, _)| *id);
                (
                    (g.collapsed(), g.busiest_shown()),
                    c.relayout_count.get(),
                    ids,
                )
            })
        };
        let widget = |r: &mut Rig, key: &str| {
            r.vcx
                .update(|_, cx| engine_widget(gpu.read(cx).flow(), 0, key))
                .expect(key)
        };
        let (dec, gpu_w) = (widget(&mut r, "dec"), widget(&mut r, "gpu"));
        let (fit, count, before) = state(&mut r);
        assert_eq!(fit, (true, true), "152 px body: the busiest engine row");
        assert!(r.vcx.debug_bounds(leak(engine_row_id(0, "dec"))).is_some());
        assert!(before.iter().any(|(id, _)| *id == dec), "{before:?}");
        assert!(!before.iter().any(|(id, _)| *id == gpu_w), "{before:?}");

        // New values, decode still the busiest: nothing is placed again.
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), totals(30., 60.))]));
        draw(r.vcx);
        assert_eq!(engine_rows(&mut r)[2].2, "60.0%", "the box took the sample");
        let (_, same_count, same) = state(&mut r);
        assert_eq!(
            same_count, count,
            "a sample with the same busiest engine places nothing"
        );
        assert_eq!(same, before, "and keeps every widget entity");

        // 3D becomes the busiest: the box places its widgets again, once.
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), totals(90., 60.))]));
        draw(r.vcx);
        let (fit, moved_count, moved) = state(&mut r);
        assert_eq!(fit, (true, true));
        assert_eq!(
            moved_count,
            count + 1,
            "one placement for the new busiest engine"
        );
        assert!(moved.iter().any(|(id, _)| *id == gpu_w), "{moved:?}");
        assert!(!moved.iter().any(|(id, _)| *id == dec), "{moved:?}");
        assert!(r.vcx.debug_bounds(leak(engine_row_id(0, "gpu"))).is_some());
        assert!(r.vcx.debug_bounds(leak(engine_row_id(0, "dec"))).is_none());
    }

    /// tasklens-lhliq.7, ADR-019 "Flow placement", leader decision D: a line value that comes
    /// and goes changes the line's shape, so it places once each time. A sample in which the
    /// power draw has no value yet leaves the power widget placed with no width, with its
    /// entity, and the line draws no value. The next value of the same shape shows in the same
    /// place.
    #[gpui_kit::test]
    fn gpu_box_line_values_place_on_shape(cx: &mut TestAppContext) {
        use tl_ui_overview::overview::gpu_flow::widget_of;

        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (box_h(300.), 200.));
        let totals = |power: Metric<f32>| {
            let mut t = gpu_engines(false);
            t.power_w = power;
            t
        };
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), totals(Metric::valid(120.)))]));
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let power = r
            .vcx
            .update(|_, cx| widget_of(gpu.read(cx).flow(), 0, "power"))
            .expect("power widget");
        let state = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                let c = g.container().read(cx);
                (
                    g.sections()[0].line.to_string(),
                    c.relayout_count.get(),
                    c.widget(power).map(|w| w.entity_id()),
                    g.drawn_widgets().contains(&power),
                )
            })
        };
        let (line, count, entity, drawn) = state(&mut r);
        assert_eq!(line, "120.0 W");
        assert!(
            entity.is_some() && drawn,
            "the power value is placed and drawn"
        );
        let at = r
            .vcx
            .update(|_, cx| gpu.read(cx).container().read(cx).placement().bounds(power));

        r.system(|s| {
            s.gpus = Arc::from([(DeviceIdx(10), totals(Metric::with(Validity::WarmingUp)))])
        });
        draw(r.vcx);
        let (line, gone_count, gone_entity, gone_drawn) = state(&mut r);
        assert_eq!(line, "", "the line shows no value");
        assert_eq!(gone_count, count + 1, "a value going away places once");
        assert_eq!(gone_entity, entity, "the widget keeps its entity");
        assert!(!gone_drawn, "and draws nothing");
        let gone_at = r
            .vcx
            .update(|_, cx| gpu.read(cx).container().read(cx).placement().bounds(power));
        assert_eq!(
            gone_at.map(|b| f32::from(b.size.width)),
            Some(0.),
            "a place with no width"
        );

        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), totals(Metric::valid(130.)))]));
        draw(r.vcx);
        let (line, back_count, back_entity, back_drawn) = state(&mut r);
        assert_eq!(line, "130.0 W");
        assert_eq!(back_count, count + 2, "a value coming back places once");
        assert_eq!(back_entity, entity);
        assert!(back_drawn);
        let back_at = r
            .vcx
            .update(|_, cx| gpu.read(cx).container().read(cx).placement().bounds(power));
        assert_eq!(back_at, at, "in the same place");
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
        tl_ui_chart::chart::format_value(
            tl_ui_chart::chart::ValueFormat::Percent,
            v,
            tl_ui_settings::settings::NetworkUnit::default(),
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
        tl_ui_chart::chart::format_value(
            tl_ui_chart::chart::ValueFormat::RateNetwork,
            1000.,
            tl_ui_settings::settings::NetworkUnit::default(),
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

    fn rate(unit: tl_ui_settings::settings::NetworkUnit, v: f32) -> String {
        let mut out = String::new();
        tl_ui_chart::chart::format_value(
            tl_ui_chart::chart::ValueFormat::RateNetwork,
            v,
            unit,
            &mut out,
        );
        out
    }

    #[gpui_kit::test]
    fn net_box_two_adapters_sum_peak_total(cx: &mut TestAppContext) {
        let mut r = open(cx, Vec::new(), Vec::new(), (200., 200.));
        two_adapters(&mut r);
        let (_, _, net) = r.boxes();
        let unit = tl_ui_settings::settings::NetworkUnit::default();
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
        use tl_ui_kit::components::tooltip::DELAY_CONTROL;
        use tl_ui_overview::overview::fit::take_fit_texts;
        use tl_ui_overview::overview::net_box::DOWN_RATE_ID;
        let mut r = open(cx, Vec::new(), Vec::new(), (200., 200.));
        two_adapters(&mut r);
        r.vcx.update(|window, _| window.activate_window());
        let _ = take_fit_texts();
        draw(r.vcx);
        let unit = tl_ui_settings::settings::NetworkUnit::default();
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
        use tl_ui_settings::settings::NetworkUnit;
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
                let builds =
                    |v: &Entity<tl_ui_chart::chart::ChartView>| v.read(cx).build_count.get();
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

/// The box title line (UI-PRF-017, UI-PRF-058, tasklens-o55.54): icon, title, summary, the range
/// figures and the value on one line. A line too narrow for all of them leaves out the figures
/// first, then the summary; what shows is never cut.
mod title_row {
    use gpui_kit::{
        Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, Window, div, px, size,
    };
    use tl_ui_base::theme::{current, metrics};
    use tl_ui_overview::overview::net_box::NET_BOX_ID;
    use tl_ui_overview::overview::parts::{
        box_frame, figures_id, title_line_id, title_name_id, title_row_id, title_value_id,
    };
    use tl_ui_overview::overview::range::RangeText;
    use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};

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
            tl_ui_base::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
    use tl_model::system::{DiskTotals, GpuTotals, LinkState, Metric, NetTotals};
    use tl_model::validity::Validity;
    use tl_ui_base::anim::AnimationDriver;
    use tl_ui_base::app_env::{BootSettings, EngineGlobal};
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::TlSettings;
    use tl_ui_overview::overview::cpu_box::{
        CORE_CHARTS_ID, CPU_CHART_ID, CpuBox, POWER_STRIP_ID, core_chart_id,
    };
    use tl_ui_overview::overview::disk_box::{self, DiskBox};
    use tl_ui_overview::overview::fit::{
        FitTextPaint, fit_text, round_decimals, shorter_forms, take_fit_texts,
    };
    use tl_ui_overview::overview::fixed_box::FixedBox;
    use tl_ui_overview::overview::gpu_box::{self, GpuBox};
    use tl_ui_overview::overview::layout::BoxKind;
    use tl_ui_overview::overview::mem_box::{self, CHARTS_GRID_ID, MemBox};
    use tl_ui_overview::overview::memory_apps;
    use tl_ui_overview::overview::net_box::{self, NetBox};
    use tl_ui_overview::overview::parts::{
        CORE_ROWS_SCROLL_ID, body_id, core_spark_id, take_core_labels, title_row_id,
    };
    use tl_ui_overview::overview::procs_box::{self, PROC_ROW_ID_PREFIX, ProcsBox};
    use tl_ui_overview::overview::timeline::{
        self, BANDS_ID, MARKER_ROW_ID, TICK_ROW_ID, TimelineBox,
    };
    use tl_ui_perf_core::core::DeviceKey;
    use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

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
            tl_ui_base::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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
        Procs(Entity<FixedBox>),
        Timeline(Entity<FixedBox>),
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
                    .map(|pid| tl_ui_testkit::fixtures::proc_record(pid, (pid - 10) as f32))
                    .collect(),
                ..ProcessSnapshot::default()
            });
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui_base::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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
                    BoxKind::TopProcs => Boxed::Procs(cx.new(|cx| {
                        let content = cx.new(|cx| ProcsBox::new(&hub, cx));
                        FixedBox::new(procs_box::SPEC, content, None, &hub, cx)
                    })),
                    BoxKind::Timeline => Boxed::Timeline(cx.new(|cx| {
                        let content = cx.new(|cx| TimelineBox::new(&hub, cx));
                        FixedBox::new(timeline::SPEC, content, Some(TimelineBox::line), &hub, cx)
                    })),
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
    use tl_model::snapshot::{ComponentHeader, DeviceHistory, HistPoint, SeriesSet};
    use tl_model::system::MemGroupBand;
    use tl_model::time::now_boot_ns;
    use tl_model::validity::Validity;
    use tl_ui_base::app_env::{BootSettings, EngineGlobal};
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::TlSettings;
    use tl_ui_base::theme::metrics;
    use tl_ui_overview::overview::mem_box::{MEM_APPS_ID, MemBox};
    use tl_ui_overview::overview::memory_apps::{
        APPS_ID, AppsChart, BandKind, LEGEND_ID, overview_legend,
    };
    use tl_ui_settings::settings::{
        AppearanceSettings, OverviewRange, Settings, ThemeSettings, UiState,
    };
    use tl_ui_testkit::fake_engine::FakeEngine;

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
        apps: Entity<AppsChart>,
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
            tl_ui_base::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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
