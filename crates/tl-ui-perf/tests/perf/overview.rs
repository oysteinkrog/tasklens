//! Performance tab Overview page (crw.40): the OverviewPage with the device sidebar, moved from
//! the tl-ui `ui` binary (tasklens-zljr5.41). The drawing parts and the boxes are in the
//! tl-ui-overview `overview` binary; the page inside the main window stays in tl-ui as
//! `overview::wired`.

// The OverviewPage shell (crw.40).

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
    use tl_model::snapshot::{ComponentHeader, SeriesSet};
    use tl_model::time::now_boot_ns;
    use tl_ui_base::anim::AnimationDriver;
    use tl_ui_base::app_env::{BootSettings, EngineGlobal};
    use tl_ui_base::demand::DemandAggregator;
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::TlSettings;
    use tl_ui_chart::chart::ChartView;
    use tl_ui_kit::components::kv_grid;
    use tl_ui_kit::components::toast::ToastQueue;
    use tl_ui_perf::performance::DeviceKey;
    use tl_ui_perf::performance::actions::{
        PerfOverviewEditLayout, PerfOverviewResetBox, PerfOverviewResetLayout,
        PerfOverviewSetBoxArrange, PerfOverviewSetBoxMode, PerfOverviewSetBoxPower,
        PerfOverviewSetBoxPowerPlacement, bind_keys_performance,
    };
    use tl_ui_perf::performance::overview::box_menu::gear_id;
    use tl_ui_perf::performance::overview::cpu_box::{
        CORE_CHARTS_ID, CPU_BOX_ID, CPU_CHART_ID, CPU_SPLITTER_ID, CPU_SPLITTER_LINE_ID,
        POWER_SPLITTER_ID, POWER_STRIP_ID, core_chart_id,
    };
    use tl_ui_perf::performance::overview::edit::{
        GHOST_ID, TOOLBAR_ID, corner_id, hide_button_id, title_handle_id,
    };
    use tl_ui_perf::performance::overview::inner_split::hit_selector;
    use tl_ui_perf::performance::overview::layout::{
        Arrange, BoxKind, GridBox, OverviewLayout, ROW_UNIT_DEFAULT, place,
    };
    use tl_ui_perf::performance::overview::mem_box::MEM_SPLITTER_ID;
    use tl_ui_perf::performance::overview::net_box::NET_STACK_SPLITTER_ID;
    use tl_ui_perf::performance::overview::{
        BOX_ID_PREFIX, OVERVIEW_KEY, OverviewPage, RANGE_HEADER_ID,
    };
    use tl_ui_perf::performance::page::DevicePage as _;
    use tl_ui_perf::performance::sidebar::DeviceSidebar;
    use tl_ui_settings::settings::{
        AppearanceSettings, OverviewGridSetting, OverviewLayoutSetting, Settings, ThemeSettings,
        UiState,
    };
    use tl_ui_testkit::fake_engine::FakeEngine;

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
                .key_context(tl_ui_kit::actions::GLOBAL_CONTEXT)
                .on_action(move |_: &tl_ui_kit::actions::EscapeChain, _, _| {
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
            tl_ui_base::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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
            cx.update(tl_ui_kit::components::toast::init);
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
                // The disk rows are borrowed from the box's model for as long as `charts` lives.
                let disk_rows = d.rows();
                let mut charts: Vec<&Entity<ChartView>> = vec![c.total_chart(), &m.sparks()[0]];
                charts.extend(g.sections().iter().map(|s| &s.chart));
                if let Some(row) = disk_rows.first() {
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

        fn saved(&mut self) -> Option<tl_ui_settings::settings::OverviewLayoutSetting> {
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

    fn cpu_split_of(
        setting: &Option<tl_ui_settings::settings::OverviewLayoutSetting>,
    ) -> Option<f64> {
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
        use tl_ui_perf::performance::overview::parts::core_spark_id;
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
        use tl_ui_perf::performance::overview::parts::core_spark_id;
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

    /// Publishes 5 samples of the CPU total and both power draw series, 100 ms apart up to now.
    fn publish_cpu_and_power(r: &mut Rig) {
        r.version += 1;
        let version = r.version;
        let newest = now_boot_ns();
        let mut views: Vec<_> = [
            MetricId::CPU_TOTAL,
            MetricId::CPU_POWER_PKG,
            MetricId::CPU_POWER_CORE,
        ]
        .into_iter()
        .map(|metric| {
            Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(0),
                    metric,
                },
                version,
                period_ns: 100_000_000,
                samples: (0..5u64)
                    .map(|k| Sample::valid(newest - (4 - k) * 100_000_000, 20. + k as f32))
                    .collect::<Vec<_>>()
                    .into(),
            })
        })
        .collect();
        views.sort_by_key(|v| v.id);
        r.engine.publish_with(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
        });
        draw(r.vcx);
    }

    /// tasklens-o55.55: with `power_placement = "under_graph"` the power draw strip sits under
    /// the total chart, as wide as it, and the core rows beside them keep the body's full
    /// height. The two charts then share one hover cursor at one time. Full width is the
    /// default, keeps today's layout and has no hover.
    #[gpui_kit::test]
    fn overview_cpu_power_under_graph(cx: &mut TestAppContext) {
        use tl_model::snapshot::RaplState;
        use tl_ui_perf::performance::overview::layout::PowerPlacement;
        use tl_ui_perf::performance::overview::parts::core_spark_id;
        let mut r = open_rooted(cx, true, 1200., 900.);
        r.vcx.update(|window, _| window.activate_window());
        r.set_row_unit(60);
        r.set_rapl(Some(RaplState::Readable));
        r.page_action(&PerfOverviewSetBoxPower {
            kind: "cpu".into(),
            on: true,
        });
        publish_cpu_and_power(&mut r);
        let set_place = |r: &mut Rig, placement: &str| {
            r.page_action(&PerfOverviewSetBoxPowerPlacement {
                kind: "cpu".into(),
                placement: placement.into(),
            });
        };
        let saved = |r: &mut Rig| r.saved().and_then(|s| s.grid[0].power_placement.clone());
        let page = r.page.clone();
        let (total, power) = r.vcx.update(|_, cx| {
            let b = page.read(cx).cpu_box().read(cx);
            (
                b.total_chart().clone(),
                b.power_chart().expect("strip chart").clone(),
            )
        });
        // The marker x and whether a value box was painted, for `chart`.
        let cursor = |r: &mut Rig, chart: &Entity<ChartView>| {
            r.vcx.read_entity(chart, |v, _| {
                let h = v.hover_state();
                (h.last_marker_x, h.last_box.is_some())
            })
        };
        let hover = |r: &mut Rig, at: gpui_kit::Point<gpui_kit::Pixels>| {
            r.vcx.simulate_mouse_move(at, None, Modifiers::none());
            draw(r.vcx);
            draw(r.vcx);
        };
        let frame = r.bounds_of(CPU_BOX_ID.into());

        // Full width by default: the strip spans the box under the core rows, and nothing is
        // written.
        assert_eq!(
            r.layout().config(BoxKind::Cpu).power_placement,
            PowerPlacement::FullWidth
        );
        assert_eq!(saved(&mut r), None);
        let strip = r.bounds_of(POWER_STRIP_ID.into());
        let rows = r.bounds_of(core_spark_id(0));
        assert!(rows.bottom() <= strip.top(), "{rows:?} {strip:?}");
        assert!(f32::from(strip.size.width) > 500., "{strip:?}");
        let chart = r.bounds_of(CPU_CHART_ID.into());
        hover(&mut r, point(chart.right() - px(20.), center(chart).y));
        assert_eq!(cursor(&mut r, &total).0, None, "no hover full width");

        // Under the graph: the strip is the chart's width, under it in the left column, and
        // the core rows' column reaches past the strip's top to the bottom of the body.
        set_place(&mut r, "under_graph");
        assert_eq!(saved(&mut r).as_deref(), Some("under_graph"));
        assert_eq!(
            r.layout().config(BoxKind::Cpu).power_placement,
            PowerPlacement::UnderGraph
        );
        let chart = r.bounds_of(CPU_CHART_ID.into());
        let strip = r.bounds_of(POWER_STRIP_ID.into());
        let rows = r.bounds_of(core_spark_id(0));
        let gap = r.gap(CPU_SPLITTER_ID);
        assert!(chart.bottom() <= strip.top(), "{chart:?} {strip:?}");
        assert!(
            (chart.left() - strip.left()).abs() <= px(1.),
            "{chart:?} {strip:?}"
        );
        assert!(
            (chart.right() - strip.right()).abs() <= px(1.),
            "{chart:?} {strip:?}"
        );
        assert!(strip.size.height >= px(64.), "{strip:?}");
        assert!(strip.right() <= gap.left() + px(1.) && gap.right() <= rows.left());
        assert!(gap.bottom() >= strip.bottom() - px(1.), "{gap:?} {strip:?}");
        assert!(
            rows.top() < chart.bottom(),
            "the rows start at the top of the body"
        );
        assert!(strip.bottom() <= frame.bottom());

        // Hover on the chart: its readout, and the cursor on the strip at the same x.
        hover(&mut r, point(chart.right() - px(20.), center(chart).y));
        let (x_total, box_total) = cursor(&mut r, &total);
        let (x_power, box_power) = cursor(&mut r, &power);
        let x = x_total.expect("cursor on the chart");
        assert!(
            box_total && !box_power,
            "one value box, on the hovered chart"
        );
        let x_power = x_power.expect("cursor on the strip");
        assert!((x - x_power).abs() <= px(1.), "{x:?} {x_power:?}");

        // Hover on the strip: the same the other way round.
        hover(&mut r, point(strip.right() - px(20.), center(strip).y));
        let (x_total, box_total) = cursor(&mut r, &total);
        let (x_power, box_power) = cursor(&mut r, &power);
        let x = x_power.expect("cursor on the strip");
        assert!(box_power && !box_total);
        let x_total = x_total.expect("cursor on the chart");
        assert!((x - x_total).abs() <= px(1.), "{x:?} {x_total:?}");

        // Off both charts: no cursor on either.
        hover(&mut r, center(rows));
        assert_eq!(cursor(&mut r, &total).0, None);
        assert_eq!(cursor(&mut r, &power).0, None);

        // Stacked, the strip stays under the chart and the core rows go under both.
        r.page_action(&PerfOverviewSetBoxArrange {
            kind: "cpu".into(),
            arrange: "vertical".into(),
        });
        let chart = r.bounds_of(CPU_CHART_ID.into());
        let strip = r.bounds_of(POWER_STRIP_ID.into());
        let rows = r.bounds_of(core_spark_id(0));
        assert!(chart.bottom() <= strip.top() && strip.bottom() <= rows.top());

        // Another mode draws the strip full width again; back in chart and cores it is under
        // the graph, because the placement belongs to the box.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: "cores".into(),
        });
        let strip = r.bounds_of(POWER_STRIP_ID.into());
        assert!(r.bounds_of(core_spark_id(0)).bottom() <= strip.top());
        assert_eq!(saved(&mut r).as_deref(), Some("under_graph"));

        // Full width again writes nothing; Reset Box clears it too.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: "chart_cores".into(),
        });
        set_place(&mut r, "full_width");
        assert_eq!(saved(&mut r), None);
        set_place(&mut r, "under_graph");
        r.page_action(&PerfOverviewResetBox { kind: "cpu".into() });
        assert_eq!(saved(&mut r), None);
        // An unknown value and another kind do nothing.
        set_place(&mut r, "left");
        assert_eq!(saved(&mut r), None);
        r.page_action(&PerfOverviewSetBoxPowerPlacement {
            kind: "memory".into(),
            placement: "under_graph".into(),
        });
        assert_eq!(
            r.saved().and_then(|s| s.grid[1].power_placement.clone()),
            None
        );
    }

    /// Publishes 2 physical cores (CPUs 0 and 2, 1 and 3), their maximum and base clocks, and 5
    /// samples 100 ms apart of the CPU total, both power series and each core's
    /// `CPU_CORE_CLOCK` (core 0 at 3000 MHz, core 1 at 4000 MHz, each 500 MHz higher in its
    /// oldest sample).
    fn publish_core_clocks(r: &mut Rig) {
        use smallvec::smallvec;
        use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};
        r.version += 1;
        let version = r.version;
        let newest = now_boot_ns();
        let series = |device: u16, metric, base: f32| {
            Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(device),
                    metric,
                },
                version,
                period_ns: 100_000_000,
                samples: (0..5u64)
                    .map(|k| {
                        let v = if k == 0 { base + 500. } else { base };
                        Sample::valid(newest - (4 - k) * 100_000_000, v)
                    })
                    .collect::<Vec<_>>()
                    .into(),
            })
        };
        let mut views = vec![
            series(0, MetricId::CPU_TOTAL, 20.),
            series(0, MetricId::CPU_POWER_PKG, 40.),
            series(0, MetricId::CPU_POWER_CORE, 30.),
            series(100, MetricId::CPU_CORE_CLOCK, 3000.),
            series(101, MetricId::CPU_CORE_CLOCK, 4000.),
        ];
        views.sort_by_key(|v| v.id);
        r.engine.publish_with(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
            let mut online = CpuSet::empty();
            for id in 0..CPUS {
                online.insert(id);
            }
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.cpu_topology = Arc::new(CpuTopology {
                online,
                cores: (0..2u16)
                    .map(|c| CoreInfo {
                        id: c,
                        logical: smallvec![c, c + 2],
                        l3_group: 0,
                        kind: CoreKind::Standard,
                    })
                    .collect(),
            });
            let inv = Arc::make_mut(&mut s.inventory);
            inv.header.version += 1;
            inv.processor.core_max_mhz = tl_model::snapshot::InvField {
                value: Arc::from([Some(5000), Some(5000)]),
                validity: tl_model::validity::Validity::Valid,
            };
            inv.processor.base_mhz = tl_model::snapshot::InvField {
                value: 3400,
                validity: tl_model::validity::Validity::Valid,
            };
        });
        draw(r.vcx);
    }

    /// tasklens-o55.57: "Show core clocks" puts a Core clocks chart, one line per physical
    /// core, under the CPU graph in the left column. Full width, the power strip stays across
    /// the box; under the graph, the column stacks the CPU graph, Core clocks and Power draw.
    /// The charts of the column share one hover cursor, the readout lists the cores fastest
    /// first, and a hovered core row highlights its core's line.
    #[gpui_kit::test]
    fn overview_cpu_core_clocks(cx: &mut TestAppContext) {
        use tl_model::snapshot::RaplState;
        use tl_ui_perf::performance::actions::PerfOverviewSetBoxClocks;
        use tl_ui_perf::performance::overview::cpu_box::CLOCKS_ID;
        use tl_ui_perf::performance::overview::parts::core_spark_id;
        let mut r = open_rooted(cx, true, 1200., 900.);
        r.vcx.update(|window, _| window.activate_window());
        r.set_row_unit(60);
        r.set_rapl(Some(RaplState::Readable));
        publish_core_clocks(&mut r);
        let set_clocks = |r: &mut Rig, on: bool| {
            r.page_action(&PerfOverviewSetBoxClocks {
                kind: "cpu".into(),
                on,
            });
        };
        assert!(r.vcx.debug_bounds(CLOCKS_ID).is_none(), "off by default");
        set_clocks(&mut r, true);
        assert_eq!(r.saved().map(|s| s.grid[0].clocks), Some(true));
        assert!(r.layout().config(BoxKind::Cpu).clocks);
        let page = r.page.clone();
        let (total, clocks, lines) = r.vcx.update(|_, cx| {
            let b = page.read(cx).cpu_box().read(cx);
            let c = b.core_clocks().expect("core clocks");
            (b.total_chart().clone(), c.view().clone(), c.lines().clone())
        });
        assert_eq!(lines.devices.len(), 2, "one line per physical core");
        publish_core_clocks(&mut r);

        // Under the CPU graph, as wide as it, beside the core rows.
        let chart = r.bounds_of(CPU_CHART_ID.into());
        let under = r.bounds_of(CLOCKS_ID.into());
        let rows = r.bounds_of(core_spark_id(0));
        assert!(chart.bottom() <= under.top(), "{chart:?} {under:?}");
        assert!((chart.left() - under.left()).abs() <= px(1.));
        assert!((chart.right() - under.right()).abs() <= px(1.));
        assert!(under.right() < rows.left(), "{under:?} {rows:?}");

        // Hover on Core clocks: its readout, fastest core first, and the cursor on the CPU
        // graph at the same x without a value box.
        let hover = |r: &mut Rig, at: gpui_kit::Point<gpui_kit::Pixels>| {
            r.vcx.simulate_mouse_move(at, None, Modifiers::none());
            draw(r.vcx);
            draw(r.vcx);
        };
        hover(&mut r, point(under.right() - px(20.), center(under).y));
        let (order, panel, x_clocks) = r.vcx.read_entity(&clocks, |v, _| {
            let l = v.lines();
            let order: Vec<usize> = l.readout_rows().map(|row| row.0).collect();
            (order, l.readout_panel(), v.hover_state().last_marker_x)
        });
        assert_eq!(order, [1, 0], "core 1 is faster");
        assert!(panel.is_some(), "readout painted");
        let (x_total, box_total) = r.vcx.read_entity(&total, |v, _| {
            let h = v.hover_state();
            (h.last_marker_x, h.last_box.is_some())
        });
        assert!(
            x_total.is_some() && !box_total,
            "cursor only on the CPU graph"
        );
        let _ = x_clocks;

        // A hovered core row highlights its core: CPU 3 is core 1's sibling.
        let row = r.bounds_of(core_spark_id(3));
        hover(&mut r, center(row));
        assert_eq!(r.vcx.read_entity(&clocks, |v, _| v.highlight()), Some(1));
        hover(&mut r, center(chart));
        assert_eq!(r.vcx.read_entity(&clocks, |v, _| v.highlight()), None);

        // Power draw full width: the strip spans the box under the rows; Core clocks stays in
        // the column.
        r.page_action(&PerfOverviewSetBoxPower {
            kind: "cpu".into(),
            on: true,
        });
        let under = r.bounds_of(CLOCKS_ID.into());
        let strip = r.bounds_of(POWER_STRIP_ID.into());
        let rows = r.bounds_of(core_spark_id(0));
        assert!(under.bottom() <= strip.top() && rows.bottom() <= strip.top());
        assert!(
            strip.size.width > under.size.width * 1.5,
            "{strip:?} {under:?}"
        );

        // Power draw under the graph: CPU graph, Core clocks and Power draw in one column.
        r.page_action(&PerfOverviewSetBoxPowerPlacement {
            kind: "cpu".into(),
            placement: "under_graph".into(),
        });
        let chart = r.bounds_of(CPU_CHART_ID.into());
        let under = r.bounds_of(CLOCKS_ID.into());
        let strip = r.bounds_of(POWER_STRIP_ID.into());
        assert!(chart.bottom() <= under.top() && under.bottom() <= strip.top());
        assert!((under.left() - strip.left()).abs() <= px(1.));
        assert!((under.right() - strip.right()).abs() <= px(1.));

        // Only in the modes with the CPU graph; off again writes nothing.
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: "cores".into(),
        });
        assert!(r.vcx.debug_bounds(CLOCKS_ID).is_none());
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: "chart".into(),
        });
        assert!(r.vcx.debug_bounds(CLOCKS_ID).is_some());
        set_clocks(&mut r, false);
        assert!(r.vcx.debug_bounds(CLOCKS_ID).is_none());
        assert_eq!(r.saved().map(|s| s.grid[0].clocks), Some(false));
    }

    #[gpui_kit::test]
    fn overview_memory_modes_render(cx: &mut TestAppContext) {
        use tl_ui_perf::performance::overview::mem_box::{
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
        use tl_ui_perf::performance::memory_apps::{APPS_ID, AppsPlace};
        use tl_ui_perf::performance::overview::mem_box::MEM_APPS_ID;
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
                .any(|m| m.id == "apps" && tl_ui_base::i18n::tr(cx, m.label).as_ref() == "By app")
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
        use tl_ui_chart::chart::labels::LabelKind;
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
        use tl_ui_perf::performance::overview::gpu_box::{
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
        use tl_ui_perf::performance::overview::gpu_box::engine_cell_id;
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
        use tl_ui_perf::performance::actions::PerfOverviewSetBoxEngine;
        use tl_ui_perf::performance::overview::gpu_box::{engine_cell_id, vram_id};
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
        use tl_ui_perf::performance::overview::gpu_box::{engine_cell_id, engine_row_id};
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
        use tl_ui_perf::performance::overview::gpu_box::{
            GPU_BOX_ID, GPU_SPLITTER_ID, engine_row_id, rows_id,
        };
        use tl_ui_perf::performance::overview::parts::body_id;
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
        use tl_ui_perf::performance::overview::disk_box::{
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
        use tl_ui_perf::performance::overview::disk_box::{DISK_SPLITTER_ID, GRID_ID, cell_id};
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
        use tl_ui_perf::performance::overview::disk_box::{
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
        use tl_ui_perf::performance::overview::disk_box::{DISK_BOX_ID, cell_id, chart_id};
        use tl_ui_perf::performance::overview::gpu_box::{
            GPU_BOX_ID, engine_cell_id, engine_chart_id,
        };
        use tl_ui_perf::performance::overview::net_box::{
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
            let menu = tl_ui_kit::components::menu::open_context_menu(window, cx)
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
        use tl_ui_perf::performance::overview::disk_box::{
            CELL_MIN_H, CELL_MIN_W, GRID_ID, cell_id,
        };
        use tl_ui_perf::performance::overview::parts::arrange_grid_gaps;
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
            // Cells side by side sit 2 px apart, cells in a column touch.
            let gap = size(px(2.), px(0.));
            let fit = arrange_grid_gaps(arrange, n, grid.size, min, gap, px(0.));
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

        // The gear's Layout items end with the Arrange submenu, the shown choice selected, then
        // "Edit Widgets…" (UI-PRF-061 phase 2).
        r.edit_layout();
        let (labels, items) = gear_menu(&mut r, BoxKind::Disk);
        assert!(
            labels.ends_with(&[
                "Arrange".to_owned(),
                String::new(),
                "Edit Widgets…".to_owned()
            ]),
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
        r.page_action(&tl_ui_perf::performance::actions::PerfOverviewEditDone);
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
        use tl_ui_perf::performance::overview::mem_box::{
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
        use tl_ui_perf::performance::overview::mem_box::{
            CELL_MIN_H, CELL_MIN_W, CHARTS_GRID_ID, cell_chart_id, cell_head_id, cell_id,
        };
        use tl_ui_perf::performance::overview::parts::arrange_grid;
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
        use tl_ui_perf::performance::overview::gpu_box::{
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
                tl_ui_perf::performance::overview::net_box::cell_id(idxs[0]),
                tl_ui_perf::performance::overview::net_box::cell_id(idxs[1]),
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
            let m = tl_ui_base::theme::metrics(cx);
            (m.sp[2], m.ui_scale)
        });
        let (mut wide, mut tall) = (false, false);
        let mut seen = Vec::new();
        for (w, h) in [(1200., 900.), (2400., 700.), (1200., 2400.), (1000., 3000.)] {
            r.vcx.simulate_resize(size(px(w), px(h)));
            draw(r.vcx);
            let grid = r.bounds_of(tl_ui_perf::performance::overview::net_box::GRID_ID.into());
            seen.push((w, h, grid.size));
            let fit = tl_ui_perf::performance::overview::parts::fill_grid(
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
        use tl_ui_perf::performance::overview::disk_box::{HEAD_ONE_LINE_W, cell_id, head_id};
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
        use tl_ui_perf::performance::overview::disk_box::HEAD_ONE_LINE_W;
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
        use tl_ui_perf::performance::overview::disk_box::{CELL_MIN_W, HEAD_LABEL_W};
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

    /// tasklens-6swdr.13: a header value never touches the next cell's name: on one row the gap
    /// from the last header part of a cell to the name of the cell beside it is at least `sp2`.
    #[gpui_kit::test]
    fn overview_disk_cell_head_gap_between_cells(cx: &mut TestAppContext) {
        use tl_ui_perf::performance::overview::disk_box::{cell_id, head_id};
        let mut r = open_rooted(cx, true, 1200., 900.);
        let keys = more_disks(&mut r, 6);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "disk".into(),
            mode: "disk_charts".into(),
        });
        set_disk_arrange(&mut r, "auto");
        widest_rates(&mut r);
        let mut pairs = 0;
        for (pct, (w, h)) in [100u16, 150, 200]
            .into_iter()
            .flat_map(|pct| [(1200., 900.), (900., 560.), (500., 900.)].map(|wh| (pct, wh)))
        {
            let gap = r.vcx.update(|_, cx| {
                let appearance = tl_ui_settings::settings::AppearanceSettings {
                    ui_scale_pct: pct,
                    ..Default::default()
                };
                tl_ui_base::theme::metrics::recompute(&appearance, cx);
                tl_ui_base::theme::metrics(cx).sp[2]
            });
            r.vcx.simulate_resize(size(px(w), px(h)));
            draw(r.vcx);
            let shown: Vec<&DeviceKey> = keys.iter().take(shown_disks(&mut r)).collect();
            for pair in shown.windows(2) {
                let (a, b) = (r.bounds_of(cell_id(pair[0])), r.bounds_of(cell_id(pair[1])));
                if a.top() != b.top() {
                    continue;
                }
                let last = ["active", "read", "write"]
                    .map(|p| r.bounds_of(head_id(pair[0], p)).right())
                    .into_iter()
                    .fold(px(0.), |m, x| if x > m { x } else { m });
                let next = r.bounds_of(head_id(pair[1], "name")).left();
                assert!(
                    next - last >= gap - px(0.5),
                    "{pct}% {w}x{h} {last:?} to {next:?}"
                );
                pairs += 1;
            }
        }
        assert!(pairs > 0, "no two cells on one row");
    }

    /// crw.70: in `rows` mode a long model stays on one line inside its header row, and every
    /// rate cell keeps its whole text, down to a 500 px window.
    #[gpui_kit::test]
    fn overview_disk_rows_text_stays_in_row(cx: &mut TestAppContext) {
        use tl_model::snapshot::{DeviceDetailsSnapshot, DiskDetails};
        use tl_model::table::Total;
        use tl_model::validity::Validity;
        use tl_ui_perf::performance::overview::disk_box::{
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
        use tl_ui_perf::performance::overview::net_box::{DOWN_RATE_ID, NET_BOX_ID, UP_RATE_ID};
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
        use tl_ui_perf::performance::overview::net_box::{GRID_ID, cell_id, name_id};
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
        use tl_ui_perf::performance::overview::net_box::{
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
            .update(|_, cx| f32::from(tl_ui_base::theme::metrics(cx).sp[5]));
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
                    tl_ui_perf::performance::overview::parts::title_value_id(&box_id)
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
        use tl_ui_perf::performance::overview::edit::grip_id;
        use tl_ui_perf::performance::overview::parts::{figures_id, title_name_id, title_row_id};
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
        use tl_ui_perf::performance::overview::disk_box::{DISK_BOX_ID, cell_id};
        use tl_ui_perf::performance::overview::parts::{body_id, title_row_id};
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
        use tl_ui_perf::performance::overview::disk_box::{
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
        use tl_ui_perf::performance::overview::edit::grip_id;
        for (w, h) in [(1920., 1200.), (1280., 800.)] {
            let mut r = open_rooted(cx, true, w, h);
            draw(r.vcx);
            r.edit_layout();
            draw(r.vcx);
            let mut checked = 0;
            for kind in BoxKind::ALL {
                let box_id = format!("perf-overview-box-{}", kind.key());
                let tab_id: &'static str = Box::leak(
                    tl_ui_perf::performance::overview::parts::title_name_id(&box_id)
                        .into_boxed_str(),
                );
                let value_id: &'static str = Box::leak(
                    tl_ui_perf::performance::overview::parts::title_value_id(&box_id)
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
            let menu = tl_ui_kit::components::menu::open_context_menu(window, cx)
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
            // CPU by app (UI-PRF-062).
            ("By app", false),
            // The default snapshot reads as RAPL readable: a separator and the power item.
            ("", false),
            ("Show power draw", false),
            ("Show core clocks", false),
            ("", false),
            ("Arrange", false),
            // The CPU box is on widgets (UI-PRF-061): its gear ends with Edit Widgets.
            ("", false),
            ("Edit Widgets…", false),
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
        use tl_ui_perf::performance::actions::PerfOverviewAddBox;
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
        use tl_ui_perf::performance::actions::PerfOverviewEditDone;
        let mut r = open(cx, true, 1160., 1200.);
        let page = r.page.clone();
        let placement = |r: &mut Rig| r.vcx.update(|_, cx| page.read(cx).placement().clone());
        let before = placement(&mut r);
        assert!(before.grid.row > before.grid.unit, "{before:?}");
        assert!(!before.scrolls);
        let bottom = |p: &tl_ui_perf::performance::overview::layout::Placement| {
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
            tl_ui_kit::actions::bind_keys_global(cx);
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
            tl_ui_kit::actions::bind_keys_global(cx);
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
            tl_ui_kit::actions::bind_keys_global(cx);
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
                tl_ui_kit::actions::bind_keys_global(cx);
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
        use tl_ui_perf::performance::actions::PerfOverviewAddBox;
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
        use tl_ui_perf::performance::actions::PerfOverviewEditDone;
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
            tl_ui_kit::actions::bind_keys_global(cx);
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
        use tl_ui_chart::chart::labels::LabelKind;
        use tl_ui_perf::performance::overview::net_box::COMBINED_CHART_ID;
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

    /// Edit Layout and Edit Widgets are never on together (UI-PRF-061): starting Edit Widgets
    /// ends Edit Layout, and Edit Layout does not start while a box is in Edit Widgets.
    #[gpui_kit::test]
    fn overview_widget_edit_modes_exclusive(cx: &mut TestAppContext) {
        use tl_ui_perf::performance::actions::PerfWidgetEditStart;
        use tl_ui_perf::performance::widget::edit::any_editing;

        let mut r = open(cx, true, 1200., 900.);
        r.edit_layout();
        let hub = r.hub.clone();
        // The GPU box hosts the widget editor (tasklens-lhliq.7).
        let page = r.page.clone();
        let editor = r
            .vcx
            .update(|_, cx| page.read(cx).gpu_box().read(cx).editor().clone());
        r.page_action(&PerfWidgetEditStart);
        assert!(!r.editing(), "Edit Widgets ends Edit Layout");
        assert!(r.vcx.read_entity(&editor, |e, _| e.is_editing()));
        assert!(r.vcx.update(|_, cx| any_editing(&hub, cx)));

        r.page_action(&PerfOverviewEditLayout);
        assert!(!r.editing(), "Edit Layout stays off during Edit Widgets");

        // The editor is not drawn here, so Cancel runs on it directly.
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
        draw(r.vcx);
        assert!(!r.vcx.read_entity(&editor, |e, _| e.is_editing()));
        r.edit_layout();
    }

    /// UI-PRF-061, ADR-019 "Flow placement": the default and each old mode are flow layouts of
    /// the preset's widgets. The container places each widget at the rectangle the fit rules
    /// give it, worked out from the box's plan and the body size, not from a paint; that is
    /// where the box draws the part, as 1.1.0 did. The temperature and power values sit at
    /// their text's width, joined by " · ". Edit Widgets pins the placed widgets there, keyed
    /// to the GPU.
    #[gpui_kit::test]
    fn overview_gpu_flow_matches_modes(cx: &mut TestAppContext) {
        use gpui_kit::Pixels;
        use tl_ui_perf::performance::actions::PerfWidgetEditStart;
        use tl_ui_perf::performance::overview::gpu_box::{chart_id, engine_cell_id, engine_row_id};
        use tl_ui_perf::performance::overview::gpu_flow::{chart_of, engine_widget, widget_of};
        use tl_ui_perf::performance::widget::container::container_id;
        use tl_ui_perf::performance::widget::metric_widget::widget_id;
        use tl_ui_settings::layout::{DeviceSelector, GPU_BOX_CONTAINER};

        let mut r = open_rooted(cx, true, 1200., 900.);
        leave_out_disk_and_net(&mut r);
        let page = r.page.clone();
        let gpu = r.vcx.update(|_, cx| page.read(cx).gpu_box().clone());
        let container = r.vcx.update(|_, cx| gpu.read(cx).container().clone());
        let body = container_id(GPU_BOX_CONTAINER);
        // A record with the temperature and the power draw, so the line draws both values.
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.gpus = Arc::from([(DeviceIdx(2), line_record(2))]);
        });
        draw(r.vcx);
        // The placement of widget `id`, in the body, px.
        let placed = |r: &mut Rig, id: u32| {
            r.vcx
                .update(|_, cx| container.read(cx).placement().bounds(id))
        };
        for mode in ["engine_charts", "chart_engines", "chart"] {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "gpu".into(),
                mode: mode.into(),
            });
            let (flow, engines) = r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                let engines: Vec<&'static str> =
                    g.sections()[0].engines.iter().map(|e| e.key).collect();
                (g.flow().clone(), engines)
            });
            assert_eq!(
                flow.preset.as_ref().map(|p| p.id.as_str()),
                Some(mode),
                "the mode is the preset"
            );
            // The chart preset has no engine widgets, so its sections build no engine rows.
            assert_eq!(engines.len() >= 2, mode != "chart", "{mode}: {engines:?}");
            // (widget, the part the box drew for it)
            let mut parts: Vec<(u32, String)> = Vec::new();
            if mode != "engine_charts" {
                parts.push((chart_of(&flow, 0).expect("section chart"), chart_id(0)));
            }
            for k in &engines {
                let id = engine_widget(&flow, 0, k).expect(k);
                match mode {
                    "engine_charts" => parts.push((id, engine_cell_id(0, k))),
                    "chart_engines" => parts.push((id, engine_row_id(0, k))),
                    _ => {}
                }
            }
            if mode == "chart" {
                for k in ["gpu", "enc", "dec", "memctl"] {
                    let id = engine_widget(&flow, 0, k);
                    assert!(id.is_none(), "chart has no engine widgets: {k}");
                }
            }
            // Each placed rectangle is the painted part, from the body's origin. Painted edges
            // snap to whole device pixels, so they may differ from the worked-out ones by
            // less than one.
            let origin = r.bounds_of(body.clone()).origin;
            let near = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() < 1.;
            let vram = widget_of(&flow, 0, "vram").expect("VRAM widget");
            let vram_part = (vram, String::new());
            // The line's values: `chart_engines` only.
            let line: Vec<(u32, String)> = if mode == "chart_engines" {
                ["temp", "power"]
                    .into_iter()
                    .map(|k| (widget_of(&flow, 0, k).expect(k), String::new()))
                    .collect()
            } else {
                Vec::new()
            };
            for (id, part) in parts.iter().chain([&vram_part]).chain(&line) {
                let at = placed(&mut r, *id).unwrap_or_else(|| panic!("{mode}: {id} placed"));
                // The VRAM row and the line's values draw no selector of their own: the widget's
                // view is the part.
                let painted = if part.is_empty() {
                    r.bounds_of(widget_id(GPU_BOX_CONTAINER, *id))
                } else {
                    r.bounds_of(part.clone())
                };
                let (x, y) = (painted.origin.x - origin.x, painted.origin.y - origin.y);
                assert!(
                    near(at.origin.x, x)
                        && near(at.origin.y, y)
                        && near(at.size.width, painted.size.width)
                        && near(at.size.height, painted.size.height),
                    "{mode}: {id} {part}: placed {at:?}, painted {painted:?} from {origin:?}"
                );
            }

            // Edit Widgets pins every drawn widget, keyed to the GPU, on the grid.
            r.page_action(&PerfWidgetEditStart);
            let editor = r.vcx.update(|_, cx| gpu.read(cx).editor().clone());
            let working = r
                .vcx
                .update(|_, cx| editor.read(cx).working_layout(cx))
                .expect("Edit Widgets is on");
            assert!(working.preset.is_none(), "{mode}: pinned to a grid layout");
            for id in parts
                .iter()
                .map(|p| p.0)
                .chain([vram])
                .chain(line.iter().map(|p| p.0))
            {
                let w = working.widgets.iter().find(|w| w.id == id);
                let w = w.unwrap_or_else(|| panic!("{mode}: widget {id} pinned"));
                assert!(
                    matches!(w.content.device(), Some(DeviceSelector::Key(_))),
                    "{w:?}"
                );
                assert!(w.cell.is_some(), "{w:?}");
            }
            // Pinned in the order drawn: the parts top to bottom, left to right.
            let cell = |id: u32| {
                let c = working
                    .widgets
                    .iter()
                    .find(|w| w.id == id)
                    .and_then(|w| w.cell)
                    .expect("cell");
                (c.row, c.col)
            };
            let mut by_drawn: Vec<u32> = parts.iter().map(|p| p.0).chain([vram]).collect();
            let at: Vec<(u32, (i32, i32))> = by_drawn
                .iter()
                .map(|id| {
                    let b = placed(&mut r, *id).expect("placed");
                    (
                        *id,
                        (f32::from(b.origin.y) as i32, f32::from(b.origin.x) as i32),
                    )
                })
                .collect();
            let drawn_order = |id: u32| at.iter().find(|(i, _)| *i == id).map(|(_, p)| *p);
            let mut by_cell = by_drawn.clone();
            by_drawn.sort_by_key(|id| drawn_order(*id));
            by_cell.sort_by_key(|id| cell(*id));
            assert_eq!(by_cell, by_drawn, "{mode}");

            r.vcx
                .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
            draw(r.vcx);
            assert!(r.vcx.update(|_, cx| gpu.read(cx).grid().is_none()));
            // Back to the flow renderer: the box's own parts show again.
            assert!(r.vcx.debug_bounds(leak_str(parts[0].1.clone())).is_some());
        }
    }

    /// tasklens-lhliq.7, ADR-019 "Flow placement": the default and each old mode draw every
    /// part through a `MetricWidget` entity of the box's container. Each part the box draws has
    /// one entity, in its 1.1.0 look and filling the part's place, with the value its source
    /// formats; a part the box does not draw has none.
    #[gpui_kit::test]
    fn overview_gpu_presets_render_through_widgets(cx: &mut TestAppContext) {
        use tl_ui_perf::performance::overview::gpu_box::{chart_id, engine_cell_id, engine_row_id};
        use tl_ui_perf::performance::overview::gpu_flow::{chart_of, engine_widget, widget_of};
        use tl_ui_perf::performance::widget::metric_widget::{FlowLook, widget_id};
        use tl_ui_settings::layout::{DeviceSelector, GPU_BOX_CONTAINER};

        let mut r = open_rooted(cx, true, 1200., 900.);
        leave_out_disk_and_net(&mut r);
        // A record for the GPU, so the rows show values.
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.gpus = Arc::from([(DeviceIdx(2), gpu_record(2))]);
        });
        draw(r.vcx);
        let page = r.page.clone();
        let gpu = r.vcx.update(|_, cx| page.read(cx).gpu_box().clone());
        let container = r.vcx.update(|_, cx| gpu.read(cx).container().clone());
        for mode in ["engine_charts", "chart_engines", "chart"] {
            r.page_action(&PerfOverviewSetBoxMode {
                kind: "gpu".into(),
                mode: mode.into(),
            });
            draw(r.vcx);
            let (flow, engines, cells, line_parts, collapsed) = r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                let s = &g.sections()[0];
                let keys: Vec<&'static str> = s.engines.iter().map(|e| e.key).collect();
                let line = if s.line.is_empty() {
                    0
                } else {
                    s.line.split(" · ").count()
                };
                (
                    g.flow().clone(),
                    keys,
                    g.shown_cells(0),
                    line,
                    g.collapsed(),
                )
            });
            assert!(!collapsed, "{mode}: room for every part");
            // (widget, the part it draws)
            let mut parts: Vec<(u32, String)> = Vec::new();
            match mode {
                "engine_charts" => {
                    assert!(cells >= 2, "{mode}: {cells} cells");
                    for k in engines.iter().take(cells) {
                        parts.push((engine_widget(&flow, 0, k).expect(k), engine_cell_id(0, k)));
                    }
                }
                "chart_engines" => {
                    parts.push((chart_of(&flow, 0).expect("section chart"), chart_id(0)));
                    for k in &engines {
                        parts.push((engine_widget(&flow, 0, k).expect(k), engine_row_id(0, k)));
                    }
                }
                _ => parts.push((chart_of(&flow, 0).expect("section chart"), chart_id(0))),
            }
            let vram = widget_of(&flow, 0, "vram").expect("VRAM widget");
            let line = if mode == "chart_engines" {
                line_parts
            } else {
                0
            };

            // One entity per drawn part: the parts, the VRAM row and the line's values.
            let placed: Vec<u32> = r
                .vcx
                .update(|_, cx| container.read(cx).placed_ids().to_vec());
            let slot0 = |id: &u32| {
                flow.widgets
                    .iter()
                    .find(|w| w.id == *id)
                    .is_some_and(|w| w.content.device() == Some(&DeviceSelector::Slot(0)))
            };
            let first = placed.iter().filter(|id| slot0(id)).count();
            assert_eq!(first, parts.len() + 1 + line, "{mode}: {placed:?}");
            let drawn = r.vcx.update(|_, cx| gpu.read(cx).drawn_widgets());
            let mut sorted = placed.clone();
            sorted.sort_unstable();
            let mut drawn_sorted = drawn.to_vec();
            drawn_sorted.sort_unstable();
            assert_eq!(sorted, drawn_sorted, "{mode}: placed is what the box drew");

            // Each part is its widget: the widget's view fills the part, in the mode's look.
            for (id, part) in parts.iter().chain([&(vram, String::new())]) {
                assert!(placed.contains(id), "{mode}: widget {id} placed");
                let (look, renders) = r.vcx.update(|_, cx| {
                    let w = container.read(cx).widget(*id).expect("entity").read(cx);
                    let look = match w.flow() {
                        Some(FlowLook::Cell { .. }) => "cell",
                        Some(FlowLook::Row { .. }) => "row",
                        Some(FlowLook::Chart(_)) => "chart",
                        Some(FlowLook::Text) => "text",
                        Some(FlowLook::MemRow { .. }) => "mem row",
                        Some(FlowLook::MemCell { .. }) => "mem cell",
                        Some(FlowLook::NetDirection { .. } | FlowLook::NetCombined { .. }) => "net",
                        None => "none",
                    };
                    (look, w.render_count.get())
                });
                assert!(renders > 0, "{mode}: widget {id} rendered");
                let want = if *id == vram {
                    "row"
                } else if Some(*id) == chart_of(&flow, 0) {
                    "chart"
                } else if mode == "engine_charts" {
                    "cell"
                } else {
                    "row"
                };
                assert_eq!(look, want, "{mode}: widget {id}");
                if !part.is_empty() {
                    let at = r.bounds_of(widget_id(GPU_BOX_CONTAINER, *id));
                    assert_eq!(at, r.bounds_of(part.clone()), "{mode}: {part}");
                }
            }
            // An engine row's value is its source's text, the same as the 1.1.0 row's.
            if mode == "chart_engines" {
                let (shown, want) = r.vcx.update(|_, cx| {
                    let g = gpu.read(cx);
                    let e = &g.sections()[0].engines[0];
                    let id = engine_widget(&flow, 0, e.key).expect("engine");
                    let w = container.read(cx).widget(id).expect("entity").read(cx);
                    (w.texts()[1].to_string(), e.meter.value.to_string())
                });
                assert_eq!(shown, want, "{mode}: engine value");
                assert_eq!(shown, "25.0%", "{mode}: a known value");
            }
        }
    }

    /// A GPU record: 3D, encode and decode at 25, 10 and 5 percent, 4 of 16 GiB VRAM and 120 W.
    fn gpu_record(device: u16) -> tl_model::system::GpuTotals {
        use tl_model::system::Metric;
        use tl_model::validity::Validity;
        const GIB: u64 = 1 << 30;
        tl_model::system::GpuTotals {
            groups: Default::default(),
            vram_groups: Default::default(),
            device: DeviceIdx(device),
            util_pct: Metric::valid(25.),
            mem_util_pct: Metric::default(),
            mem_used: Metric::valid(4 * GIB),
            mem_total: Metric::valid(16 * GIB),
            shared_used: Metric::default(),
            shared_total: Metric::default(),
            mem_reserved: Metric::default(),
            enc_pct: Metric::valid(10.),
            dec_pct: Metric::valid(5.),
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

    /// [`gpu_record`] with the temperature too: 65 °C.
    fn line_record(device: u16) -> tl_model::system::GpuTotals {
        tl_model::system::GpuTotals {
            temp_c: tl_model::system::Metric::valid(65.),
            ..gpu_record(device)
        }
    }

    /// Hides the disks so the Disk box leaves the page, and takes the Network box off the page,
    /// so the GPU box has their room. Widget selectors have the container id
    /// (tasklens-6swdr.10), so other boxes do not shadow them.
    fn leave_out_disk_and_net(r: &mut Rig) {
        use tl_ui_perf::performance::overview::disk_box::DISK_BOX_ID;
        use tl_ui_perf::performance::overview::net_box::NET_BOX_ID;
        use tl_ui_settings::settings::OverviewLayoutSetting;
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let perf = &mut d.settings.performance;
                perf.show_disks = false;
                let mut layout = perf
                    .overview
                    .clone()
                    .unwrap_or_else(OverviewLayoutSetting::grid_default);
                layout.grid.retain(|b| b.kind != "net");
                perf.overview = Some(layout);
            });
        });
        draw(r.vcx);
        assert!(r.vcx.debug_bounds(DISK_BOX_ID).is_none(), "no Disk box");
        assert!(r.vcx.debug_bounds(NET_BOX_ID).is_none(), "no Network box");
    }

    /// Every widget the container places is drawn by the box exactly there: each widget's
    /// view, from the body's origin, within 1 px (painted edges snap to device pixels). The box
    /// draws nothing it does not place.
    fn assert_drawn_is_placed(
        r: &mut Rig,
        gpu: &Entity<tl_ui_perf::performance::overview::gpu_box::GpuBox>,
        what: &str,
    ) {
        use gpui_kit::Pixels;
        use tl_ui_perf::performance::widget::container::container_id;
        use tl_ui_perf::performance::widget::metric_widget::widget_id;
        use tl_ui_settings::layout::GPU_BOX_CONTAINER;

        let (placed, drawn) = r.vcx.update(|_, cx| {
            let g = gpu.read(cx);
            let placed: Vec<(u32, gpui_kit::Bounds<Pixels>)> = g
                .container()
                .read(cx)
                .placement()
                .widgets
                .iter()
                .map(|p| (p.id, p.bounds))
                .collect();
            (placed, g.drawn_widgets().to_vec())
        });
        assert!(!placed.is_empty(), "{what}: widgets placed");
        // A line value with no reading keeps a place with no width and is not drawn.
        let (placed, empty): (Vec<_>, Vec<_>) = placed
            .into_iter()
            .partition(|p| f32::from(p.1.size.width) > 0.);
        for (id, _) in &empty {
            assert!(!drawn.contains(id), "{what}: widget {id} has no width");
        }
        let mut placed_ids: Vec<u32> = placed.iter().map(|p| p.0).collect();
        let mut drawn_ids = drawn.clone();
        placed_ids.sort_unstable();
        drawn_ids.sort_unstable();
        assert_eq!(
            placed_ids, drawn_ids,
            "{what}: the box draws what it places"
        );
        let origin = r.bounds_of(container_id(GPU_BOX_CONTAINER)).origin;
        let near = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() < 1.;
        for (id, at) in placed {
            let painted = r.bounds_of(widget_id(GPU_BOX_CONTAINER, id));
            let (x, y) = (painted.origin.x - origin.x, painted.origin.y - origin.y);
            assert!(
                near(at.origin.x, x)
                    && near(at.origin.y, y)
                    && near(at.size.width, painted.size.width)
                    && near(at.size.height, painted.size.height),
                "{what}: widget {id}: placed {at:?}, drawn {painted:?} from {origin:?}"
            );
        }
    }

    /// tasklens-lhliq.7, ADR-019 "Flow placement": drawing and placement come from one
    /// function. For the default and every old mode, Side by side included, at two window
    /// sizes, every widget's drawn bounds are its placed rectangle: the cells, the section
    /// chart, the engine rows, the VRAM row and the temperature and power values.
    #[gpui_kit::test]
    fn overview_gpu_flow_drawn_is_placed(cx: &mut TestAppContext) {
        use tl_ui_perf::performance::overview::gpu_flow::widget_of;

        let mut r = open_rooted(cx, true, 1200., 900.);
        leave_out_disk_and_net(&mut r);
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.gpus = Arc::from([(DeviceIdx(2), line_record(2))]);
        });
        draw(r.vcx);
        let page = r.page.clone();
        let gpu = r.vcx.update(|_, cx| page.read(cx).gpu_box().clone());
        let modes = [
            ("engine_charts", None),
            ("chart_engines", None),
            ("chart_engines", Some(Arrange::Horizontal)),
            ("chart", None),
        ];
        for (w, h) in [(1200., 900.), (1500., 1100.)] {
            r.vcx.simulate_resize(size(px(w), px(h)));
            draw(r.vcx);
            for (mode, arrange) in modes {
                r.page_action(&PerfOverviewSetBoxMode {
                    kind: "gpu".into(),
                    mode: mode.into(),
                });
                r.page_action(&PerfOverviewSetBoxArrange {
                    kind: "gpu".into(),
                    arrange: arrange.unwrap_or(Arrange::Auto).id().into(),
                });
                let what = format!("{mode} {arrange:?} at {w} x {h}");
                assert_drawn_is_placed(&mut r, &gpu, &what);
                if mode == "chart_engines" {
                    // Both line values are placed and drawn.
                    let (flow, drawn) = r.vcx.update(|_, cx| {
                        (gpu.read(cx).flow().clone(), gpu.read(cx).drawn_widgets())
                    });
                    for k in ["temp", "power"] {
                        let id = widget_of(&flow, 0, k).expect(k);
                        assert!(drawn.contains(&id), "{what}: {k} drawn");
                    }
                }
            }
        }
    }

    /// tasklens-lhliq.7, leader decision D: the temperature and power line draws with tabular
    /// digits, so a value's width depends only on its shape. A sample that keeps the shape
    /// places nothing; a value that gains or loses a character, goes away or comes back places
    /// once. After each sample every widget's drawn bounds are its placed rectangle.
    #[gpui_kit::test]
    fn overview_gpu_line_places_on_shape(cx: &mut TestAppContext) {
        use tl_model::system::Metric;
        use tl_model::validity::Validity;
        use tl_ui_perf::performance::overview::gpu_flow::widget_of;

        let mut r = open_rooted(cx, true, 1200., 900.);
        leave_out_disk_and_net(&mut r);
        let publish = |r: &mut Rig, temp: Metric<f32>, power: f32| {
            r.engine.publish_with(|s| {
                let sys = Arc::make_mut(&mut s.system);
                sys.header.version += 1;
                let mut t = line_record(2);
                t.temp_c = temp;
                t.power_w = Metric::valid(power);
                sys.gpus = Arc::from([(DeviceIdx(2), t)]);
            });
            draw(r.vcx);
        };
        publish(&mut r, Metric::valid(65.), 120.);
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "gpu".into(),
            mode: "chart_engines".into(),
        });
        let page = r.page.clone();
        let gpu = r.vcx.update(|_, cx| page.read(cx).gpu_box().clone());
        let (temp, power) = r.vcx.update(|_, cx| {
            let flow = gpu.read(cx).flow().clone();
            (
                widget_of(&flow, 0, "temp").expect("temp"),
                widget_of(&flow, 0, "power").expect("power"),
            )
        });
        let state = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                let g = gpu.read(cx);
                let drawn = g.drawn_widgets();
                (
                    g.sections()[0].line.to_string(),
                    g.container().read(cx).relayout_count.get(),
                    (drawn.contains(&temp), drawn.contains(&power)),
                )
            })
        };
        let (line, count, drawn) = state(&mut r);
        assert!(line.ends_with("120.0 W"), "{line}");
        assert_eq!(drawn, (true, true), "both values drawn");
        assert_drawn_is_placed(&mut r, &gpu, "start");

        // New values of the same shape: nothing is placed.
        publish(&mut r, Metric::valid(66.), 121.5);
        let (line, same, _) = state(&mut r);
        assert!(line.ends_with("121.5 W"), "{line}");
        assert_eq!(same, count, "a sample that keeps the shape places nothing");
        assert_drawn_is_placed(&mut r, &gpu, "same shape");

        // The power value loses a character: one placement.
        publish(&mut r, Metric::valid(66.), 99.5);
        let (line, fewer, _) = state(&mut r);
        assert!(line.ends_with("99.5 W"), "{line}");
        assert_eq!(fewer, count + 1, "a new character count places once");
        assert_drawn_is_placed(&mut r, &gpu, "fewer characters");

        // The temperature goes away: one placement, and power starts the line.
        publish(&mut r, Metric::with(Validity::WarmingUp), 99.5);
        let (line, gone, drawn) = state(&mut r);
        assert_eq!(line, "99.5 W");
        assert_eq!(gone, count + 2, "a value going away places once");
        assert_eq!(drawn, (false, true));
        assert_drawn_is_placed(&mut r, &gpu, "temperature gone");

        // It comes back: one placement.
        publish(&mut r, Metric::valid(70.), 99.5);
        let (line, back, drawn) = state(&mut r);
        assert!(
            line.ends_with("99.5 W") && line.len() > "99.5 W".len(),
            "{line}"
        );
        assert_eq!(back, count + 3, "a value coming back places once");
        assert_eq!(drawn, (true, true));
        assert_drawn_is_placed(&mut r, &gpu, "temperature back");
    }

    /// tasklens-lhliq.7: with the stacked `chart_engines` split moved, the first chart's
    /// height follows the body height at every layout. After each resize the drawn chart is
    /// the placed rectangle, and the splitter sits under it.
    #[gpui_kit::test]
    fn gpu_box_split_resize_draws_the_placed_chart(cx: &mut TestAppContext) {
        use gpui_kit::Pixels;
        use tl_ui_perf::performance::overview::gpu_box::{GPU_SPLITTER_ID, chart_id};
        use tl_ui_perf::performance::overview::gpu_flow::chart_of;
        use tl_ui_perf::performance::widget::container::container_id;
        use tl_ui_settings::layout::GPU_BOX_CONTAINER;

        let mut r = open_rooted(cx, true, 1200., 900.);
        leave_out_disk_and_net(&mut r);
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.gpus = Arc::from([(DeviceIdx(2), line_record(2))]);
        });
        draw(r.vcx);
        let page = r.page.clone();
        let gpu = r.vcx.update(|_, cx| page.read(cx).gpu_box().clone());
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "gpu".into(),
            mode: "chart_engines".into(),
        });
        r.vcx
            .update(|_, cx| gpu.update(cx, |g, cx| g.set_split(0.6, cx)));
        draw(r.vcx);
        let near = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() < 1.;
        let mut heights = Vec::new();
        for (w, h) in [(1200., 900.), (1200., 1150.), (1400., 800.), (1200., 900.)] {
            r.vcx.simulate_resize(size(px(w), px(h)));
            draw(r.vcx);
            let (flow, plan) = r
                .vcx
                .update(|_, cx| (gpu.read(cx).flow().clone(), gpu.read(cx).flow_plan()));
            assert_eq!(
                plan.and_then(|p| p.stacked_share),
                Some(0.6),
                "the split is moved"
            );
            let chart = chart_of(&flow, 0).expect("section chart");
            let at = r
                .vcx
                .update(|_, cx| gpu.read(cx).container().read(cx).placement().bounds(chart))
                .expect("chart placed");
            let origin = r.bounds_of(container_id(GPU_BOX_CONTAINER)).origin;
            let drawn = r.bounds_of(chart_id(0));
            let (x, y) = (drawn.origin.x - origin.x, drawn.origin.y - origin.y);
            assert!(
                near(at.origin.x, x)
                    && near(at.origin.y, y)
                    && near(at.size.width, drawn.size.width)
                    && near(at.size.height, drawn.size.height),
                "{w} x {h}: placed {at:?}, drawn {drawn:?} from {origin:?}"
            );
            let gap = r.bounds_of(GPU_SPLITTER_ID.to_owned());
            assert!(
                near(gap.origin.y, drawn.bottom()),
                "{w} x {h}: the splitter {gap:?} under the chart {drawn:?}"
            );
            assert_drawn_is_placed(&mut r, &gpu, &format!("split at {w} x {h}"));
            heights.push(f32::from(drawn.size.height));
        }
        assert!(
            heights[1] > heights[0] && heights[2] < heights[0],
            "the chart follows the body height: {heights:?}"
        );
        assert!(
            (heights[3] - heights[0]).abs() < 1.,
            "back to the first size: {heights:?}"
        );
    }

    fn leak_str(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    /// A saved grid layout with VRAM as a chart: the box draws it through its widget
    /// container, and the chart reads the VRAM history.
    #[gpui_kit::test]
    fn overview_gpu_vram_chart_widget_draws_history(cx: &mut TestAppContext) {
        use tl_ui_perf::performance::overview::gpu_box::EMPTY_ID;
        use tl_ui_perf::performance::widget::metric_widget::widget_id;
        use tl_ui_settings::layout::{
            ContainerLayout, DeviceSelector, FormSet, GPU_BOX_CONTAINER, LAYOUT_VERSION_GPU_ONLY,
            MetricKey, SourceSelector, WidgetCell, WidgetContent, WidgetOptions, WidgetSetting,
        };

        let mut r = open_rooted(cx, true, 1200., 900.);
        let page = r.page.clone();
        let gpu = r.vcx.update(|_, cx| page.read(cx).gpu_box().clone());
        let (key, idx) = r.vcx.update(|_, cx| {
            let s = &gpu.read(cx).sections()[0];
            (s.key.0.clone(), s.idx)
        });
        let layout = |widgets: Vec<WidgetSetting>| ContainerLayout {
            container: GPU_BOX_CONTAINER.to_owned(),
            version: LAYOUT_VERSION_GPU_ONLY,
            preset: None,
            widgets,
            next_id: 2,
        };
        let vram = WidgetSetting {
            id: 1,
            content: WidgetContent::Metric(SourceSelector {
                device: DeviceSelector::Key(key),
                metric: MetricKey::new("vram"),
            }),
            forms: FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
            cell: Some(WidgetCell {
                col: 0,
                row: 0,
                cols: 6,
                rows: 6,
            }),
            options: WidgetOptions::default(),
            device_name: None,
        };
        let saved = layout(vec![vram]);
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.performance.layouts.set(saved));
        });
        draw(r.vcx);
        assert!(r.vcx.update(|_, cx| gpu.read(cx).grid().is_some()));
        assert!(
            r.vcx
                .debug_bounds(leak_str(widget_id(GPU_BOX_CONTAINER, 1)))
                .is_some(),
            "the widget is drawn"
        );
        // The engine rows of the flow layout are gone.
        assert!(
            r.vcx
                .debug_bounds(leak_str(
                    tl_ui_perf::performance::overview::gpu_box::engine_cell_id(0, "gpu")
                ))
                .is_none()
        );

        // A VRAM history: the widget's chart reads it.
        r.version += 1;
        let version = r.version;
        let newest = now_boot_ns();
        let view = Arc::new(SeriesView {
            id: SeriesId {
                device: idx,
                metric: MetricId::GPU_MEM,
            },
            version,
            period_ns: 1_000_000_000,
            samples: (0..10u64)
                .map(|k| Sample::valid(newest - (9 - k) * 1_000_000_000, (k as f32 + 1.) * 1e9))
                .collect::<Vec<_>>()
                .into(),
        });
        r.engine.publish_with(|s| {
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
        let container = r.vcx.update(|_, cx| gpu.read(cx).container().clone());
        let (series, points) = r.vcx.update(|_, cx| {
            let w = container.read(cx).widget(1).expect("widget entity").clone();
            let chart = w.read(cx).chart().expect("a chart").clone();
            let chart = chart.read(cx);
            let series: Vec<SeriesId> = chart.series_ids().collect();
            let points = chart
                .resolved()
                .first()
                .and_then(|v| v.as_ref())
                .map_or(0, |v| v.samples.len());
            (series, points)
        });
        assert_eq!(
            series,
            [SeriesId {
                device: idx,
                metric: MetricId::GPU_MEM,
            }]
        );
        assert_eq!(points, 10, "the chart draws the VRAM history");

        // An empty grid layout shows the empty text.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.performance.layouts.set(layout(Vec::new()))
            });
        });
        draw(r.vcx);
        assert!(r.vcx.debug_bounds(EMPTY_ID).is_some());
    }

    /// Phase 1 gate (tasklens-lhliq.8): the only GPU goes and comes back. The box leaves the page
    /// and returns; the saved layout is never touched, and the widget is drawn again.
    #[gpui_kit::test]
    fn overview_gpu_hotplug_keeps_the_saved_layout(cx: &mut TestAppContext) {
        use tl_ui_perf::performance::widget::metric_widget::widget_id;
        use tl_ui_settings::layout::{
            ContainerLayout, DeviceSelector, FormSet, GPU_BOX_CONTAINER, LAYOUT_VERSION_GPU_ONLY,
            MetricKey, SourceSelector, WidgetCell, WidgetContent, WidgetOptions, WidgetSetting,
        };

        let mut r = open_rooted(cx, true, 1200., 900.);
        let page = r.page.clone();
        let gpu = r.vcx.update(|_, cx| page.read(cx).gpu_box().clone());
        let key = r
            .vcx
            .update(|_, cx| gpu.read(cx).sections()[0].key.0.clone());
        let saved = ContainerLayout {
            container: GPU_BOX_CONTAINER.to_owned(),
            version: LAYOUT_VERSION_GPU_ONLY,
            preset: None,
            // An id the CPU box's preset does not use: widget selectors are not per box.
            widgets: vec![WidgetSetting {
                id: 40,
                content: WidgetContent::Metric(SourceSelector {
                    device: DeviceSelector::Key(key),
                    metric: MetricKey::new("vram"),
                }),
                forms: FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
                cell: Some(WidgetCell {
                    col: 0,
                    row: 0,
                    cols: 6,
                    rows: 6,
                }),
                options: WidgetOptions::default(),
                device_name: None,
            }],
            next_id: 41,
        };
        let stored = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                TlSettings::settings(cx)
                    .performance
                    .layouts
                    .known(GPU_BOX_CONTAINER)
                    .cloned()
            })
        };
        let want = saved.clone();
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.performance.layouts.set(saved));
        });
        draw(r.vcx);
        let widget = leak_str(widget_id(GPU_BOX_CONTAINER, 40));
        assert!(r.vcx.debug_bounds(widget).is_some(), "drawn before");

        // The GPU is unplugged: the box is gone, the layout is not.
        r.engine.publish_with(|s| {
            let mut catalog = (*s.devices).clone();
            catalog.version += 1;
            catalog.entries = catalog
                .entries
                .iter()
                .filter(|e| !matches!(e.identity, DeviceIdentity::Gpu { .. }))
                .cloned()
                .collect::<Vec<_>>()
                .into();
            s.devices = Arc::new(catalog);
        });
        draw(r.vcx);
        assert!(r.vcx.debug_bounds(box_id(BoxKind::Gpu)).is_none(), "no box");
        assert!(r.vcx.debug_bounds(widget).is_none(), "no widget");
        assert_eq!(stored(&mut r), Some(want.clone()), "gone: layout kept");

        // The GPU is plugged in again: the same layout is drawn.
        r.engine.publish_with(|s| {
            let mut catalog = catalog(true);
            catalog.version = s.devices.version + 1;
            s.devices = Arc::new(catalog);
        });
        draw(r.vcx);
        assert!(
            r.vcx.debug_bounds(box_id(BoxKind::Gpu)).is_some(),
            "box back"
        );
        assert!(r.vcx.debug_bounds(widget).is_some(), "drawn after");
        assert_eq!(stored(&mut r), Some(want), "back: layout kept");
        let gpu = r.vcx.update(|_, cx| page.read(cx).gpu_box().clone());
        assert!(
            r.vcx.update(|_, cx| gpu.read(cx).grid().is_some()),
            "the box shows the grid layout"
        );
    }

    /// Each box's widget container and editor, Top processes only while the layout places it.
    fn widget_boxes(
        r: &mut Rig,
    ) -> Vec<(
        BoxKind,
        Entity<tl_ui_perf::performance::widget::container::WidgetContainer>,
        Entity<tl_ui_perf::performance::widget::edit::WidgetEditor>,
    )> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let p = page.read(cx);
            let mut out = vec![
                (
                    BoxKind::Cpu,
                    p.cpu_box().read(cx).container().clone(),
                    p.cpu_box().read(cx).editor().clone(),
                ),
                (
                    BoxKind::Memory,
                    p.mem_box().read(cx).container().clone(),
                    p.mem_box().read(cx).editor().clone(),
                ),
                (
                    BoxKind::Gpu,
                    p.gpu_box().read(cx).container().clone(),
                    p.gpu_box().read(cx).editor().clone(),
                ),
                (
                    BoxKind::Disk,
                    p.disk_box().read(cx).container().clone(),
                    p.disk_box().read(cx).editor().clone(),
                ),
                (
                    BoxKind::Net,
                    p.net_box().read(cx).container().clone(),
                    p.net_box().read(cx).editor().clone(),
                ),
                (
                    BoxKind::Timeline,
                    p.timeline_frame().read(cx).container().clone(),
                    p.timeline_frame().read(cx).editor().clone(),
                ),
            ];
            if let Some(f) = p.procs_frame() {
                let f = f.read(cx);
                out.push((BoxKind::TopProcs, f.container().clone(), f.editor().clone()));
            }
            out
        })
    }

    /// The six boxes the default layout places. Each draws its widget container.
    const SIX_BOXES: [BoxKind; 6] = [
        BoxKind::Cpu,
        BoxKind::Memory,
        BoxKind::Gpu,
        BoxKind::Disk,
        BoxKind::Net,
        BoxKind::Timeline,
    ];

    fn drawn(r: &mut Rig, name: &'static str) -> bool {
        r.vcx.debug_bounds(name).is_some()
    }

    /// The whole page check: the six boxes are drawn and no selector is drawn twice
    /// (tasklens-6swdr.11, [`tl_ui_pagehost::selectors::assert_none_drawn_twice`]).
    fn assert_page_selectors(r: &mut Rig, what: &str) {
        for kind in SIX_BOXES {
            assert!(drawn(r, box_id(kind)), "{what}: the {kind:?} box is drawn");
        }
        tl_ui_pagehost::selectors::assert_none_drawn_twice(r.vcx, what);
    }

    /// In Edit Widgets of `kind`'s box: no selector is drawn twice, every placed widget draws
    /// its frame, grip, gear and corner, a selected one its outline, and Remove Widget takes
    /// the widget and its edit parts off the page; Undo brings them back.
    fn assert_edit_selectors(
        r: &mut Rig,
        kind: BoxKind,
        container: &Entity<tl_ui_perf::performance::widget::container::WidgetContainer>,
        editor: &Entity<tl_ui_perf::performance::widget::edit::WidgetEditor>,
    ) {
        use tl_ui_perf::performance::widget::edit::{
            corner_id as widget_corner_id, frame_id, gear_id as widget_gear_id,
            grip_id as widget_grip_id, outline_id,
        };
        use tl_ui_perf::performance::widget::metric_widget::widget_id;
        let what = format!("{kind:?} Edit Widgets");
        assert_page_selectors(r, &what);
        let (c, placed) = r.vcx.read_entity(container, |c, _| {
            (c.id().to_string(), c.placed_ids().to_vec())
        });
        assert!(!placed.is_empty(), "{what}: widgets placed");
        for &id in &placed {
            let parts = [
                frame_id(&c, id),
                widget_grip_id(&c, id),
                widget_gear_id(&c, id),
                widget_corner_id(&c, id),
            ]
            .map(leak_str);
            for name in parts {
                assert!(drawn(r, name), "{what}: {name} drawn");
            }
            r.vcx
                .update(|_, cx| editor.update(cx, |e, cx| e.select(id, cx)));
            draw(r.vcx);
            let outline = leak_str(outline_id(&c, id));
            assert!(drawn(r, outline), "{what}: {outline} drawn");
            let root = leak_str(widget_id(&c, id));
            let mine: Vec<&'static str> = parts
                .into_iter()
                .chain([outline, root])
                .filter(|&name| drawn(r, name))
                .collect();
            assert!(
                r.vcx
                    .update(|_, cx| editor.update(cx, |e, cx| e.remove_selected(cx))),
                "{what}: widget {id} removed"
            );
            draw(r.vcx);
            for &name in &mine {
                assert!(
                    !drawn(r, name),
                    "{what}: {name} is still drawn without widget {id}"
                );
            }
            assert!(
                r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.undo(cx))),
                "{what}: widget {id} back"
            );
            draw(r.vcx);
            // Undo puts the widget back but not the selection, so its outline stays off.
            for &name in mine.iter().filter(|&&name| name != outline) {
                assert!(drawn(r, name), "{what}: {name} back after Undo");
            }
        }
    }

    /// tasklens-6swdr.10 and 6swdr.11, ADR-019: widget ids repeat across containers, so every
    /// widget-level debug selector has the container id. The whole Overview at 1200 × 900,
    /// every box on its default widgets, in Edit Widgets, with one added widget each, and in
    /// Edit Layout: all six boxes are drawn and no selector of the window is drawn twice.
    #[gpui_kit::test]
    fn overview_widget_selectors_unique_on_the_page(cx: &mut TestAppContext) {
        use tl_ui_perf::performance::actions::{PerfWidgetEditBox, WidgetSource};
        use tl_ui_perf::performance::widget::metric_widget::widget_id;

        let mut r = open_rooted(cx, true, 1200., 900.);
        // The rig's catalog with the persistent keys the engine gives, and a GPU record, so
        // each box resolves its devices and Add Widget offers sources in every box.
        r.engine.publish_with(|s| {
            let mut catalog = catalog(true);
            catalog.version = s.devices.version + 1;
            let entries: Vec<DeviceEntry> = catalog
                .entries
                .iter()
                .cloned()
                .map(|mut e| {
                    e.persistent_key = match &e.identity {
                        DeviceIdentity::CpuPackage => Arc::from("cpu"),
                        DeviceIdentity::Memory => Arc::from("memory"),
                        DeviceIdentity::Gpu { .. } => Arc::from("gpu:0101"),
                        DeviceIdentity::Disk { name, .. } => Arc::from(format!("disk:{name}")),
                        DeviceIdentity::Net { name, .. } => Arc::from(format!("net:{name}")),
                        DeviceIdentity::Aggregate(AggregateKind::Disks) => {
                            Arc::from("aggregate:disk")
                        }
                        DeviceIdentity::Aggregate(AggregateKind::Nets) => {
                            Arc::from("aggregate:net")
                        }
                        DeviceIdentity::CpuLogical { id } => Arc::from(format!("cpu:{id}")),
                        _ => Arc::clone(&e.persistent_key),
                    };
                    e
                })
                .collect();
            catalog.entries = Arc::from(entries);
            s.devices = Arc::new(catalog);
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.gpus = Arc::from([(DeviceIdx(2), gpu_record(2))]);
        });
        draw(r.vcx);
        assert_page_selectors(&mut r, "default");

        // Every one of the six boxes gets Edit Widgets and one added widget.
        let boxes = widget_boxes(&mut r);
        let kinds: Vec<BoxKind> = boxes.iter().map(|(kind, _, _)| *kind).collect();
        assert_eq!(kinds, SIX_BOXES, "the boxes on widgets");
        let mut added = Vec::new();
        for (kind, container, editor) in boxes {
            r.page_action(&PerfWidgetEditBox {
                kind: kind.key().into(),
            });
            assert!(
                r.vcx.read_entity(&editor, |e, _| e.is_editing()),
                "{kind:?}: Edit Widgets"
            );
            assert_edit_selectors(&mut r, kind, &container, &editor);
            let source = r.vcx.update(|_, cx| {
                let groups = editor.read(cx).source_groups(cx);
                let (group, item) = groups
                    .iter()
                    .find_map(|g| {
                        let sub = g.subs.iter().flat_map(|s| s.items.iter());
                        g.items.iter().chain(sub).next().map(|i| (g, i))
                    })
                    .unwrap_or_else(|| panic!("{kind:?}: a source in {groups:?}"));
                WidgetSource {
                    device: item.device.clone().unwrap_or_else(|| group.key.clone()),
                    metric: item.metric.0.clone(),
                }
            });
            let id = r
                .vcx
                .update(|_, cx| editor.update(cx, |e, cx| e.add(&source, cx)))
                .unwrap_or_else(|| panic!("{kind:?}: a widget added"));
            assert!(
                r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))),
                "{kind:?}: applied"
            );
            draw(r.vcx);
            let c = r.vcx.read_entity(&container, |c, _| c.id().to_string());
            assert!(
                r.vcx.read_entity(&container, |c, _| c
                    .layout()
                    .widgets
                    .iter()
                    .any(|w| w.id == id)),
                "{kind:?}: widget {id} in the layout"
            );
            added.push((kind, leak_str(widget_id(&c, id))));
        }
        assert_page_selectors(&mut r, "one added widget per box");
        for (kind, name) in added {
            assert!(
                drawn(&mut r, name),
                "{kind:?}: the added widget {name} drawn"
            );
        }

        // Edit Layout adds the box handles; they too have one owner each.
        r.edit_layout();
        for kind in SIX_BOXES {
            for name in [title_handle_id(kind), hide_button_id(kind)] {
                let name = leak_str(name);
                assert!(drawn(&mut r, name), "Edit Layout: {name} drawn");
            }
        }
        assert_page_selectors(&mut r, "Edit Layout");
    }
}
