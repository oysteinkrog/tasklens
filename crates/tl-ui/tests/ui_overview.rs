//! Performance tab Overview page (E24, UI-PRF-017, D-022): the drawing parts. Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use gpui_kit::px;
use tl_ui::pages::performance::overview::parts::{
    CoreBarsModel, CoreCell, core_bars_layout, core_cell, ghz_bucket, meter_fill_w,
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

#[test]
fn core_bars_layout_32_cores() {
    let l = core_bars_layout(32, px(776.), px(400.), 1.);
    assert_eq!((l.cols, l.rows), (4, 8));
    assert_eq!(l.row_h, px(18.));
    assert!(l.labels && l.meter && !l.squeezed);
    // Too low for 18 px rows: squeezed, labels kept at 14 px and hidden below.
    let l = core_bars_layout(32, px(776.), px(8. * 15.), 1.);
    assert_eq!(l.row_h, px(15.));
    assert!(l.squeezed && l.labels);
    let l = core_bars_layout(32, px(776.), px(50.), 1.);
    assert_eq!(l.row_h, px(12.));
    assert!(!l.labels && !l.meter);
    // Narrower than one cell: one column.
    let l = core_bars_layout(4, px(100.), px(400.), 1.);
    assert_eq!((l.cols, l.rows), (1, 4));
}

/// crw.53: the CPU box at 1200 × 900 is 776 × 516. Its body is 750 × 456 and the side part
/// beside the 40% chart is 750 × 0.6 − 12 = 438 px wide: 2 columns of 215 px, 16 rows of 18 px.
#[test]
fn core_rows_fit_32_threads_at_1200x900() {
    let l = core_bars_layout(32, px(438.), px(456.), 1.);
    assert_eq!((l.cols, l.rows), (2, 16));
    assert_eq!(l.cell_w, px(215.));
    assert_eq!(l.row_h, px(18.));
    assert!(!l.squeezed && l.labels && l.meter);
    assert_eq!(
        core_cell(&l, 1.),
        CoreCell {
            spark_x: px(32.),
            spark_w: px(48.),
            meter_x: px(84.),
            meter_w: px(43.),
            pct_right: px(167.),
        }
    );
}

#[test]
fn core_columns_follow_width() {
    let cols = |w: f32| core_bars_layout(32, px(w), px(900.), 1.).cols;
    assert_eq!(cols(170.), 1);
    assert_eq!(cols(368.), 2);
    assert_eq!(cols(564.), 3);
    assert_eq!(cols(752.), 4);
    // UI scale widens every cell.
    assert_eq!(core_bars_layout(32, px(752.), px(900.), 2.).cols, 2);
    // A cell under 188 px gives the meter's room to the sparkline.
    let l = core_bars_layout(32, px(376.), px(900.), 1.);
    assert_eq!((l.cols, l.cell_w), (2, px(184.)));
    assert!(l.labels && !l.meter);
    let c = core_cell(&l, 1.);
    assert_eq!((c.spark_w, c.meter_w), (px(64.), px(0.)));
}

#[test]
fn core_ghz_bucket_change_only() {
    assert_eq!(ghz_bucket(3949.), 39);
    assert_eq!(ghz_bucket(3950.), 40);
    assert_eq!(ghz_bucket(-5.), 0);
    let mut model = CoreBarsModel::default();
    model.set_cpus((0..2).map(|i| format!("C{i}").into()));
    assert_eq!(model.ghz_labels()[0].as_ref(), "…");
    assert!(model.set_freqs([Some(3900.), Some(4630.)].into_iter()));
    assert_eq!(model.ghz_labels()[0].as_ref(), "3.9 GHz");
    assert_eq!(model.ghz_labels()[1].as_ref(), "4.6 GHz");
    assert!(!model.set_freqs([Some(3920.), Some(4610.)].into_iter()));
    assert_eq!(model.freqs[0], Some(3920.));
    assert!(model.set_freqs([Some(3960.), Some(4610.)].into_iter()));
    assert_eq!(model.ghz_labels()[0].as_ref(), "4.0 GHz");
    assert!(model.set_freqs([Some(3960.), None].into_iter()));
    assert_eq!(model.ghz_labels()[1].as_ref(), "…");
    let mut text = String::new();
    model.copy_text(&mut text);
    assert_eq!(text, "C0\t…\t4.0 GHz\nC1\t…\n");
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
    use tl_ui::pages::performance::overview::parts::{
        CoreBars, CoreBarsModel, METER_H, Meter, take_meter_quads,
    };

    enum Part {
        Meter(Option<f32>),
        Cores(Rc<RefCell<CoreBarsModel>>),
    }

    struct Host(Part);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let body = match &self.0 {
                Part::Meter(fraction) => Meter {
                    fraction: *fraction,
                    color: hsla(0.5, 0.5, 0.5, 1.),
                }
                .into_any_element(),
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
                    .w(px(101.))
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
    use tl_ui::pages::performance::overview::parts::core_spark_id;
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
        // default (384 x 516, a 358 px body) and a 720 px page (224 x 280, a 198 px body).
        for (w, h, spark_w) in [(800., 300., 96.), (384., 516., 96.), (224., 280., 62.)] {
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

    /// crw.53: the GHz column follows each core's `CPU_FREQ` series, and the box notifies only
    /// when a 0.1 GHz bucket changes.
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
                bars.ghz_labels()
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(ghz(&mut r), ["3.9 GHz", "4.0 GHz", "4.6 GHz", "0.8 GHz"]);
        let before = r.counts();

        // Inside every 0.1 GHz bucket: no notify.
        r.publish_core_series(&pct, &[3920., 3980., 4610., 820.]);
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
    /// The column count follows the box width.
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
        assert!(!layout.squeezed && layout.labels && layout.meter);
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
            let (row, col) = (i / 2, i % 2);
            if col == 1 {
                let left = r
                    .vcx
                    .debug_bounds(Box::leak(core_spark_id(i - 1).into_boxed_str()));
                let left = left.expect("left neighbour");
                assert_eq!(b.top(), left.top(), "core {i} beside core {}", i - 1);
                assert!(b.left() > left.right(), "core {i} in the right column");
            } else if row > 0 {
                let above = r
                    .vcx
                    .debug_bounds(Box::leak(core_spark_id(i - 2).into_boxed_str()));
                let above = above.expect("row above");
                assert_eq!(b.top() - above.top(), px(18.), "core {i} one row lower");
            }
        }
        // The sparklines are bound to their core's series.
        r.vcx.update(|_, cx| {
            let chart = sparks[31].read(cx);
            assert!(chart.resolved()[0].is_some(), "C31 CPU_TOTAL resolved");
        });

        // The column count follows the width: 1000 px gives a 572 px side part, 3 columns.
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
        assert_eq!((layout.cols, layout.rows), (3, 11));
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
        ComponentHeader, DeviceDetailsSnapshot, DiskDetails, SeriesSet, Snapshot, SystemSnapshot,
    };
    use tl_model::system::{DiskTotals, GpuTotals, LinkState, Metric, NetTotals};
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
        GPU_BOX_ID, GpuBox, engine_row_id, rows_id,
    };
    use tl_ui::pages::performance::overview::net_box::NetBox;
    use tl_ui::pages::performance::overview::parts::time_axis_id;
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
            gpu: cx.new(|cx| GpuBox::new(&hub, &gpus, cx)),
            disk: cx.new(|cx| DiskBox::new(&hub, &disks, cx)),
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
        // 344 px, the default 1200 × 900 row: a 284 px body is below the 306 px two need.
        r.heights(344., 200.);
        assert_eq!(shown(&mut r), 1, "344 px: one section");
        r.heights(150., 200.);
        assert_eq!(shown(&mut r), 1, "150 px: one section");
        r.heights(450., 200.);
        assert_eq!(shown(&mut r), 2, "back to both");
    }

    /// Two GPUs with 3 engines, VRAM and the temperature and power row: each section is
    /// 48 + 5 × 22 = 158 px, the second adds a 22 px heading, with 12 px between: 350 px, the
    /// body of a 410 px box. Sections take their content height, so neither clips.
    #[gpui_kit::test]
    fn gpu_box_two_full_sections_at_410(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10, 11]), Vec::new(), (410., 200.));
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
        let axis = r.bounds(time_axis_id(GPU_BOX_ID));
        for i in 0..2 {
            let rows = r.bounds(rows_id(i));
            assert_eq!(f32::from(rows.size.height), rows_h(5), "section {i}");
            assert!(
                rows.bottom() <= axis.top(),
                "section {i} rows end at {:?}, the time axis starts at {:?}",
                rows.bottom(),
                axis.top()
            );
        }
        let first = r.bounds(rows_id(0));
        let second = r.bounds(engine_row_id(1, "gpu"));
        assert!(first.bottom() < second.top(), "{first:?} above {second:?}");

        r.heights(409., 200.);
        assert_eq!(
            state(&mut r),
            (1, false),
            "409 px: one full section and \"+1 more\""
        );
        let rows = r.bounds(rows_id(0));
        assert_eq!(f32::from(rows.size.height), rows_h(6));
    }

    /// Two GPUs at the 220 px row minimum (160 px body): the first section in full with the
    /// "+1 more" row needs 48 + 6 × 22 = 180 px, so it shows only its busiest engine and
    /// "+2 more engines": 48 + 5 × 22 = 158 px, above the time axis.
    #[gpui_kit::test]
    fn gpu_box_min_height_shows_busiest_engine(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10, 11]), Vec::new(), (220., 200.));
        r.system(|s| {
            let mut t10 = gpu_engines(false);
            t10.dec_pct = Metric::valid(50.);
            let mut t11 = gpu_engines(false);
            t11.device = DeviceIdx(11);
            s.gpus = Arc::from([(DeviceIdx(10), t10), (DeviceIdx(11), t11)]);
        });
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let (shown, collapsed, more, engines_more) = r.vcx.update(|_, cx| {
            let g = gpu.read(cx);
            (
                g.shown(),
                g.collapsed(),
                g.shown_more_text().to_string(),
                g.sections()[0].engines_more.to_string(),
            )
        });
        assert_eq!((shown, collapsed), (1, true));
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
        let rows = r.bounds(rows_id(0));
        let axis = r.bounds(time_axis_id(GPU_BOX_ID));
        assert!(
            rows.bottom() <= axis.top(),
            "rows end at {:?}, the time axis starts at {:?}",
            rows.bottom(),
            axis.top()
        );
        assert_eq!(
            f32::from(rows.size.height),
            rows_h(5),
            "busiest engine, \"+2 more engines\", VRAM, temperature and power, \"+1 more\""
        );
    }

    /// One GPU at the 220 px row minimum: 3 engines, VRAM and the temperature and power row need
    /// 158 of the 160 px body, so every engine row shows.
    #[gpui_kit::test]
    fn gpu_box_one_gpu_min_height_all_engines(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10]), Vec::new(), (220., 200.));
        r.system(|s| s.gpus = Arc::from([(DeviceIdx(10), gpu_engines(false))]));
        draw(r.vcx);
        let (gpu, _, _) = r.boxes();
        let collapsed = r.vcx.update(|_, cx| gpu.read(cx).collapsed());
        assert!(!collapsed);
        for key in ["gpu", "enc", "dec"] {
            assert!(
                r.vcx.debug_bounds(leak(engine_row_id(0, key))).is_some(),
                "{key}"
            );
        }
        let rows = r.bounds(rows_id(0));
        let axis = r.bounds(time_axis_id(GPU_BOX_ID));
        assert!(rows.bottom() <= axis.top(), "{rows:?} above {axis:?}");
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

    /// crw.55: at the 220 px row minimum (body 160, 7 lines) 2 of 3 sections expand, the
    /// busiest first; at 300 px all 3.
    #[gpui_kit::test]
    fn disk_box_collapse_follows_activity(cx: &mut TestAppContext) {
        let idx = [20, 21, 22];
        let mut r = open(cx, Vec::new(), devices("disk", &idx), (200., 220.));
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
        let mut r = open(cx, Vec::new(), devices("disk", &idx), (200., 600.));
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
        // 600 px: a 540 px body, 24 lines: 8 headers, "+2 more" and 7 expanded sections.
        assert_eq!(state(&mut r), (8, 2, 8, 7, "+2 more".into()));
        // 200 px: a 140 px body, 6 lines: 5 headers and "+5 more".
        r.heights(200., 200.);
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
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
        Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::demand::{CpuDemand, Demand};
    use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{ComponentHeader, SeriesSet};
    use tl_model::time::now_boot_ns;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::chart::ChartView;
    use tl_ui::components::kv_grid;
    use tl_ui::demand::DemandAggregator;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::DeviceKey;
    use tl_ui::pages::performance::overview::layout::{BoxKind, OverviewLayout, place};
    use tl_ui::pages::performance::overview::{BOX_ID_PREFIX, OVERVIEW_KEY, OverviewPage};
    use tl_ui::pages::performance::page::DevicePage as _;
    use tl_ui::pages::performance::sidebar::DeviceSidebar;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;

    const CPUS: u16 = 4;

    struct Host {
        page: Option<Entity<OverviewPage>>,
        sidebar: Entity<DeviceSidebar>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // The sidebar draws out of the way, so its demand part carries this window as in
            // the main window.
            div()
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
        let (host, vcx) = cx.add_window_view(move |_, cx| {
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
            }
        });
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
    fn overview_places_boxes_btop_default(cx: &mut TestAppContext) {
        let r = open(cx, true, 1200., 900.);
        let want = place(
            &OverviewLayout::btop_default(),
            |_| true,
            size(px(1200.), px(900.)),
            1.,
        );
        assert_eq!(want.boxes.len(), 5);
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
        for kind in [BoxKind::Disk, BoxKind::Net] {
            let b = r.vcx.debug_bounds(box_id(kind)).expect("box drawn");
            assert_eq!(b.size.width, px(580.), "{kind:?}");
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
                    r.vcx.simulate_event(gpui_kit::MouseMoveEvent {
                        position: gpui_kit::point(x, y),
                        pressed_button: None,
                        modifiers: gpui_kit::Modifiers::none(),
                    });
                    moves += 1;
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
        assert_eq!(r.merged(), Demand::default());
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
    use tl_ui::pages::performance::overview::cpu_box::CPU_BOX_ID;
    use tl_ui::pages::performance::overview::disk_box::{DISK_BOX_ID, DISK_ROW_ID_PREFIX};
    use tl_ui::pages::performance::overview::gpu_box::GPU_BOX_ID;
    use tl_ui::pages::performance::overview::mem_box::MEM_BOX_ID;
    use tl_ui::pages::performance::overview::net_box::NET_BOX_ID;
    use tl_ui::pages::performance::overview::parts::{
        TIME_AXIS_END, TIME_AXIS_START, box_accent, time_axis_id, time_axis_label_id, title_row_id,
        title_tab_id,
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
            let tab_id: &'static str = Box::leak(title_tab_id(id).into_boxed_str());
            let row = r.vcx.debug_bounds(row_id).expect("title line");
            let tab = r.vcx.debug_bounds(tab_id).expect("inset title tab");
            let row_inset = row.top() - frame.top();
            let tab_inset = tab.top() - frame.top();
            assert!(
                (px(0.)..=px(1.)).contains(&row_inset),
                "{id}: title line inset is {row_inset:?}"
            );
            assert!(
                (px(0.)..=px(1.)).contains(&tab_inset),
                "{id}: title tab inset is {tab_inset:?}"
            );
            assert_eq!(tab.size.height, row.size.height, "{id}: full title height");
            assert_eq!(box_accent(id, &theme), want, "{id}: device accent");
            let axis_id = selector_id(time_axis_id(id));
            let axis_start_id = selector_id(time_axis_label_id(id, "start"));
            let axis_end_id = selector_id(time_axis_label_id(id, "end"));
            assert!(
                r.vcx.debug_bounds(axis_id).is_some(),
                "{id}: one box time axis"
            );
            assert!(
                r.vcx.debug_bounds(axis_start_id).is_some(),
                "{id}: {TIME_AXIS_START}"
            );
            assert!(
                r.vcx.debug_bounds(axis_end_id).is_some(),
                "{id}: {TIME_AXIS_END}"
            );
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

    /// crw.52: Default Dark uses the shared title frame, device accent and one box time axis.
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

    fn proc_record(pid: u32, cpu_pct: f32) -> ProcessRecord {
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
                d.settings.performance.overview = on.then(OverviewLayoutSetting::with_top_procs);
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
        let title_tab = selector_id(title_tab_id(PROCS_BOX_ID));
        let time_axis = selector_id(time_axis_id(PROCS_BOX_ID));
        assert!(r.vcx.debug_bounds(title_row).is_some(), "shared title line");
        assert!(r.vcx.debug_bounds(title_tab).is_some(), "shared title tab");
        let theme = r.vcx.update(|_, cx| current(cx).clone());
        assert_eq!(box_accent(PROCS_BOX_ID, &theme), theme.graph.cpu_line);
        assert!(
            r.vcx.debug_bounds(time_axis).is_none(),
            "a table-only box has no chart axis"
        );
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
