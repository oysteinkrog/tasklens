//! Performance tab Overview page (E24, UI-PRF-017, D-022): the drawing parts. Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use gpui_kit::px;
use tl_ui::pages::performance::overview::parts::{CoreBarsModel, core_bars_layout, meter_fill_w};

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
    assert_eq!((l.cols, l.rows), (5, 7));
    assert_eq!(l.row_h, px(18.));
    assert!(l.labels);
    // Too low for 18 px rows: squeezed, labels kept at 14 px and hidden below.
    let l = core_bars_layout(32, px(776.), px(7. * 15.), 1.);
    assert_eq!(l.row_h, px(15.));
    assert!(l.labels);
    let l = core_bars_layout(32, px(776.), px(50.), 1.);
    assert_eq!(l.row_h, px(12.));
    assert!(!l.labels);
    // Narrower than one cell: one column.
    let l = core_bars_layout(4, px(100.), px(400.), 1.);
    assert_eq!((l.cols, l.rows), (1, 4));
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
        assert_eq!(shaped, 8, "4 names and 4 percents shaped once");

        assert!(
            model
                .borrow_mut()
                .set_values([Some(10.), Some(25.), Some(30.), Some(40.)].into_iter())
        );
        vcx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        draw(vcx);
        let m = model.borrow();
        assert!(m.paint_count.get() > paints, "painted again");
        assert_eq!(
            m.shape_count.get(),
            shaped + 1,
            "only C1's percent re-shaped"
        );
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
    use tl_ui::chart::ChartView;
    use tl_ui::chart::labels::LabelKind;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::overview::cpu_box::CpuBox;
    use tl_ui::pages::performance::overview::mem_box::MemBox;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;

    const GIB: u64 = 1 << 30;
    const CPUS: u16 = 4;

    struct Host {
        cpu: Entity<CpuBox>,
        mem: Entity<MemBox>,
        /// The CPU box width.
        cpu_w: f32,
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
                        .h(px(300.))
                        .child(self.cpu.clone().cached(cached())),
                )
                .child(
                    div()
                        .w(px(800.))
                        .h(px(300.))
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
        let mut entries = vec![
            entry(0, DeviceIdentity::CpuPackage),
            entry(1, DeviceIdentity::Memory),
        ];
        entries.extend((0..CPUS).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id })));
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
            cpu: cx.new(|cx| CpuBox::new(&hub, DeviceIdx(0), cx)),
            mem: cx.new(|cx| MemBox::new(&hub, DeviceIdx(1), cx)),
            cpu_w: 800.,
        });
        vcx.simulate_scale_factor_change(1.0);
        let driver = vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            driver
        });
        vcx.simulate_resize(size(px(800.), px(600.)));
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
            self.version += 1;
            let version = self.version;
            let mut views: Vec<_> = (0..CPUS)
                .map(|id| {
                    series(
                        100 + id,
                        MetricId::CPU_TOTAL,
                        values[usize::from(id)],
                        version,
                    )
                })
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
        });
        let mem = r.mem();
        let (rows, value) = r.vcx.update(|_, cx| {
            let m = mem.read(cx);
            (m.rows().clone(), m.text().1.to_string())
        });
        assert_eq!(value, "50.0%");
        assert_eq!(rows[0].label.as_ref(), "Used");
        assert_eq!(rows[0].fraction, Some(0.5));
        assert_eq!(rows[0].value.as_ref(), "32.00 GiB");
        assert_eq!(rows[2].fraction, Some(0.25));
        assert_eq!(rows[3].value.as_ref(), "No swap");
        assert_eq!(rows[3].fraction, None);

        r.system(|s| {
            s.memory.swap_total = 8 * GIB;
            s.memory.swap_free = 6 * GIB;
        });
        let swap = r.vcx.update(|_, cx| mem.read(cx).rows()[3].clone());
        assert_eq!(swap.fraction, Some(0.25));
        assert_eq!(swap.value.as_ref(), "2.00 GiB");
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
        let chart = r.vcx.update(|_, cx| mem.read(cx).chart().clone());
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
    fn scale_label_shapes_on_ceiling_change(cx: &mut TestAppContext) {
        let mut r = open(cx);
        r.system(|s| {
            s.memory.validity = Validity::Valid;
            s.memory.total = 16 * GIB;
            s.memory.used = 4 * GIB;
        });
        draw(r.vcx);
        let (cpu, mem) = (r.cpu(), r.mem());
        let cpu_chart = r.vcx.update(|_, cx| cpu.read(cx).total_chart().clone());
        let mem_chart = r.vcx.update(|_, cx| mem.read(cx).chart().clone());
        let label = |r: &mut Rig, chart: &Entity<ChartView>| {
            r.vcx.update(|_, cx| {
                let c = chart.read(cx).label_cache();
                assert!(
                    c.painted(LabelKind::AxisMax).is_some(),
                    "scale label painted"
                );
                (c.text(LabelKind::AxisMax).map(str::to_owned), c.shape_count)
            })
        };
        assert_eq!(label(&mut r, &cpu_chart).0.as_deref(), Some("100.0%"));
        let (text, shapes) = label(&mut r, &mem_chart);
        assert_eq!(text.as_deref(), Some("16.00 GiB"));
        // Memory changes below the total keep the label shaped.
        r.system(|s| s.memory.used = 5 * GIB);
        draw(r.vcx);
        assert_eq!(label(&mut r, &mem_chart).1, shapes);
        // A new total is a new ceiling.
        r.system(|s| s.memory.total = 32 * GIB);
        draw(r.vcx);
        let (text, after) = label(&mut r, &mem_chart);
        assert_eq!(text.as_deref(), Some("32.00 GiB"));
        assert_eq!(after, shapes + 1);
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
    use tl_model::snapshot::{ComponentHeader, SeriesSet, Snapshot, SystemSnapshot};
    use tl_model::system::{DiskTotals, GpuTotals, LinkState, Metric, NetTotals};
    use tl_model::time::now_boot_ns;
    use tl_model::units;
    use tl_model::validity::Validity;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::performance::DeviceKey;
    use tl_ui::pages::performance::overview::disk_box::DiskBox;
    use tl_ui::pages::performance::overview::gpu_box::GpuBox;
    use tl_ui::pages::performance::overview::net_box::NetBox;
    use tl_ui::settings_global::TlSettings;
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
        /// GPU and Disk box heights.
        gpu_h: f32,
        disk_h: f32,
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
                        .w(px(500.))
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

    /// GPUs at 10..13, disks sda and sdb at 20 and 21, aggregates, eth0 (wired) at 30, docker0
    /// (virtual) at 31 and lo at 32.
    fn catalog() -> DeviceCatalog {
        let mut entries: Vec<_> = (0..3)
            .map(|n| entry(10 + n, gpu_id(n as u8), None))
            .collect();
        entries.push(entry(20, disk_id(0, "sda"), None));
        entries.push(entry(21, disk_id(16, "sdb"), None));
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
        NetTotals {
            device: DeviceIdx(device),
            rx_bps: Metric::valid(rx_bps),
            tx_bps: Metric::valid(0.),
            rx_total,
            tx_total: 0,
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
    }

    fn rate_iec(bps: f64) -> String {
        let mut s = String::new();
        units::rate_iec_auto(&mut s, bps);
        s
    }

    #[gpui_kit::test]
    fn gpu_box_caps_at_two_with_more(cx: &mut TestAppContext) {
        let r = open(cx, devices("gpu", &[10, 11, 12]), Vec::new(), (400., 200.));
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
        assert_eq!(shown, 2, "400 px fits both sections");
        let mut copy = String::new();
        r.vcx.update(|_, cx| gpu.read(cx).copy_text(&mut copy));
        assert!(copy.ends_with("+1 more\n"), "{copy}");
    }

    #[gpui_kit::test]
    fn gpu_box_second_section_only_when_both_fit(cx: &mut TestAppContext) {
        let mut r = open(cx, devices("gpu", &[10, 11]), Vec::new(), (400., 200.));
        let (gpu, _, _) = r.boxes();
        let shown = |r: &mut Rig| r.vcx.update(|_, cx| gpu.read(cx).shown());
        assert_eq!(shown(&mut r), 2);
        r.heights(150., 200.);
        assert_eq!(shown(&mut r), 1, "150 px: one section");
        r.heights(400., 200.);
        assert_eq!(shown(&mut r), 2, "back to both");
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
        assert_eq!(rows[1].name.as_ref(), "disk 1");
    }

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
                    d.shown_more_text().to_string(),
                )
            })
        };
        assert_eq!(
            state(&mut r),
            (8, 2, 8, "+2 more".into()),
            "600 px: all 8 rows"
        );
        // 200 px: 200 - 2·12 padding - 2 border - 24 title = 150 body; minus the 48 px chart,
        // a 12 px gap and the 18 px header leaves 72 px: 4 rows, one goes to "+n more".
        r.heights(200., 200.);
        assert_eq!(state(&mut r), (8, 2, 3, "+7 more".into()));
    }

    #[gpui_kit::test]
    fn disk_box_io_chart_on_aggregate(cx: &mut TestAppContext) {
        let mut r = open(cx, Vec::new(), devices("disk", &[20]), (200., 300.));
        let (_, disk, _) = r.boxes();
        let ids: Vec<SeriesId> = r
            .vcx
            .update(|_, cx| disk.read(cx).io_chart().read(cx).series_ids().collect());
        let id = |metric| SeriesId {
            device: DeviceIdx(DISK_AGG),
            metric,
        };
        assert_eq!(ids, [id(MetricId::DISK_READ), id(MetricId::DISK_WRITE)]);

        // With data, the auto scale's ceiling shows as the compact scale label.
        let newest = now_boot_ns();
        let views: Vec<_> = [MetricId::DISK_READ, MetricId::DISK_WRITE]
            .into_iter()
            .map(|metric| {
                Arc::new(SeriesView {
                    id: id(metric),
                    version: 1,
                    period_ns: 1_000_000_000,
                    samples: (0..5u64)
                        .map(|k| Sample::valid(newest - (4 - k) * 1_000_000_000, 3. * MIB as f32))
                        .collect::<Vec<_>>()
                        .into(),
                })
            })
            .collect();
        r.publish(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version: 1,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
        });
        draw(r.vcx);
        let text = r.vcx.update(|_, cx| {
            let chart = disk.read(cx).io_chart().read(cx);
            chart
                .label_cache()
                .text(tl_ui::chart::labels::LabelKind::AxisMax)
                .map(str::to_owned)
        });
        assert_eq!(
            text.as_deref(),
            Some("4 MiB/s"),
            "the power-of-two ceiling over 3 MiB/s"
        );
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
        assert_eq!(texts[2], "1.00 GiB since boot", "eth0 only");
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
                [
                    g.render_count.get(),
                    g.notify_count.get(),
                    builds(&g.sections()[0].chart),
                    d.render_count.get(),
                    d.notify_count.get(),
                    builds(d.io_chart()),
                    n.render_count.get(),
                    n.notify_count.get(),
                    builds(n.down_chart()),
                    builds(n.up_chart()),
                ]
            })
        };
        let before = counts(&mut r);
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
                let mut charts: Vec<&Entity<ChartView>> = vec![c.total_chart(), m.chart()];
                charts.extend(g.sections().iter().map(|s| &s.chart));
                charts.extend([d.io_chart(), n.down_chart(), n.up_chart()]);
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
    use tl_model::ids::DeviceIdx;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
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
    use tl_ui::pages::performance::overview::parts::title_row_id;
    use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
    use tl_ui::pages::performance::page::DevicePage as _;
    use tl_ui::pages::performance::{DeviceKey, PAGE_PANEL_ID, PerformancePage, SIDEBAR_PANEL_ID};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
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

    struct FakeProcesses(FocusHandle);

    impl gpui_kit::EventEmitter<tl_ui::window::status::StatusChanged> for FakeProcesses {}

    impl Render for FakeProcesses {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::{InteractiveElement as _, Styled as _};
            div().track_focus(&self.0).size_full()
        }
    }

    impl shell::ShellPage for FakeProcesses {
        fn primary_focus(&self) -> FocusHandle {
            self.0.clone()
        }
    }

    struct Rig<'a> {
        page: Entity<PerformancePage>,
        demand: Entity<DemandAggregator>,
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
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: Arc::from(catalog()),
            });
        });
        let client: Arc<dyn EngineClient> = engine;
        let demand = cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
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
                let page = cx.new(|cx| FakeProcesses(cx.focus_handle().tab_stop(true)));
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
        Rig { page, demand, vcx }
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
    }

    fn center_of(r: &mut Rig, id: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
        let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
        r.vcx.debug_bounds(id).expect(id).center()
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
                    p.render_count.get(),
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
}
