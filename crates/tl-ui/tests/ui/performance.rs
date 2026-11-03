//! Performance tab parts that go through MainWindow: the first visit to the tab, and four graph
//! summary tests (E24-015) that check the main window: the tab switch block, the title bar
//! menus, the main toast queue and the hidden window. The Details grid, the device page frame,
//! the Performance keys, the tab layout and the other summary, page menu and copy tests moved to
//! the tl-ui-perf `perf` binary (tasklens-zljr5.41).

// ---- The Performance tab: the first visit through the main window (E24-A1) ----

mod tab {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, FocusHandle, IntoElement, Render, TestAppContext,
        VisualTestContext, Window, div,
    };
    use tl_model::client::EngineClient;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::DeviceIdx;
    use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology};
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui_base::demand::DemandAggregator;
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::TlSettings;
    use tl_ui_kit::components::kv_grid;
    use tl_ui_perf::performance::{PerformancePage, SIDEBAR_PANEL_ID};
    use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("device"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        }
    }

    /// `n` logical CPUs, two per core.
    fn topology(n: u16) -> CpuTopology {
        let half = n / 2;
        let mut online = CpuSet::empty();
        for id in 0..n {
            online.insert(id);
        }
        CpuTopology {
            online,
            cores: (0..half)
                .map(|c| CoreInfo {
                    id: c,
                    logical: smallvec::smallvec![c, c + half],
                    l3_group: 0,
                    kind: CoreKind::Standard,
                })
                .collect(),
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

    /// Default settings with the Overview card hidden (D-022), so the CPU page is the first page.
    fn hidden_overview() -> Settings {
        let mut s = Settings::default();
        s.performance.device_overrides.insert(
            tl_ui_perf::performance::overview::OVERVIEW_KEY.to_owned(),
            tl_ui_settings::settings::DeviceVisibility::Hide,
        );
        s
    }

    /// Every Global the tab needs over a fake catalog with CPU (`cpus` logical CPUs) and Memory.
    fn install(cx: &mut TestAppContext, state: UiState, cpus: u16) -> Rc<RefCell<Vec<u16>>> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            let mut entries = vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(1, DeviceIdentity::Memory),
            ];
            entries.extend((0..cpus).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id })));
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: Arc::from(entries),
            });
            Arc::make_mut(&mut s.system).cpu_topology = Arc::new(topology(cpus));
        });
        let client: Arc<dyn EngineClient> = engine;
        let persisted: Rc<RefCell<Vec<u16>>> = Rc::default();
        let seen = Rc::clone(&persisted);
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
            cx.set_global(BootSettings {
                settings: hidden_overview(),
                state,
                persist: Some(Box::new(move |_, state| {
                    seen.borrow_mut().push(state.performance.nav_width)
                })),
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        persisted
    }

    struct FakeProcesses(FocusHandle);

    impl gpui_kit::EventEmitter<tl_ui::window::status::StatusChanged> for FakeProcesses {}

    impl Render for FakeProcesses {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::{InteractiveElement as _, Styled as _};
            div().track_focus(&self.0).size_full()
        }
    }

    impl tl_ui::window::main::ShellPage for FakeProcesses {
        fn primary_focus(&self) -> FocusHandle {
            self.0.clone()
        }
    }

    #[gpui_kit::test]
    fn performance_tab_initial_focus_is_sidebar(cx: &mut TestAppContext) {
        use tl_model::command::Tab;
        use tl_ui::app::StartOptions;
        use tl_ui::window::main::{
            self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
        };
        use tl_ui::window::titlebar::TitleBarProbe;
        use tl_ui_base::demand::GlobalDemand;
        use tl_ui_base::hub::GlobalHub;
        use tl_ui_kit::actions::SelectTab;

        install(cx, UiState::default(), 16);
        cx.update(|cx| {
            let client = Arc::clone(&cx.global::<EngineGlobal>().0);
            let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub));
            cx.set_global(GlobalDemand(demand));
            tl_ui_kit::components::toast::init(cx);
            tl_ui_kit::actions::bind_keys_global(cx);
            cx.set_global(StartOptions::default());
            set_test_page(cx, Tab::Processes, |_, cx: &mut Context<MainWindow>| {
                let page = cx.new(|cx| FakeProcesses(cx.focus_handle().tab_stop(true)));
                PageSlot::new(page, cx)
            });
            cx.set_global(TitleBarProbe {
                client: Some(true),
                ..TitleBarProbe::default()
            });
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let vcx = &mut VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        draw(vcx);

        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let built = |vcx: &mut VisualTestContext| {
            vcx.read_entity(&main, |m, _| {
                m.page_entity::<PerformancePage>(Tab::Performance)
            })
        };
        assert!(built(vcx).is_none(), "built on first visit");
        vcx.dispatch_action(SelectTab(Tab::Performance as usize));
        draw(vcx);
        let page = built(vcx).expect("PerformancePage built");
        let focus = vcx.update(|_, cx| page.read(cx).primary_focus());
        assert!(vcx.update(|window, _| focus.is_focused(window)));
        assert!(vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some());
    }
}

// ---- Graph summary mode (E24-015), the parts that check the main window ----

mod summary {
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::{
        AppContext as _, Context, Entity, FocusHandle, IntoElement, Modifiers, MouseButton,
        MouseDownEvent, MouseUpEvent, Render, TestAppContext, VisualTestContext, Window, div, px,
        size,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::demand::Demand;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
    use tl_model::ids::DeviceIdx;
    use tl_model::system::Metric;
    use tl_model::validity::Validity;
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
    use tl_ui::window::main::{
        self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
    };
    use tl_ui::window::titlebar::TitleBarProbe;
    use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
    use tl_ui_base::hub::{GlobalHub, SnapshotHub};
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::TlSettings;
    use tl_ui_kit::components::kv_grid;
    use tl_ui_kit::components::motion::M_FAST;
    use tl_ui_perf::performance::memory::MemoryPage;
    use tl_ui_perf::performance::overview::OVERVIEW_KEY;
    use tl_ui_perf::performance::summary::SummaryView;
    use tl_ui_perf::performance::{DeviceKey, PerformancePage};
    use tl_ui_settings::settings::{
        AppearanceSettings, DeviceVisibility, Settings, ThemeSettings, UiState,
    };
    use tl_ui_testkit::fake_engine::FakeEngine;

    fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("device"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind,
            hot_c: None,
        }
    }

    fn disk(idx: u16, name: &str) -> DeviceEntry {
        let identity = DeviceIdentity::Disk {
            major: 8,
            minor: idx.into(),
            name: Arc::from(name),
            wwn_or_serial: None,
        };
        entry(idx, identity, None)
    }

    fn net(idx: u16, name: &str, kind: NetKind) -> DeviceEntry {
        let identity = DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: idx.into(),
            name: Arc::from(name),
        };
        entry(idx, identity, Some(kind))
    }

    /// Nine visible cards: CPU, Memory, a GPU, four disks, a wired interface and a VPN tunnel.
    fn catalog() -> Vec<DeviceEntry> {
        let gpu = DeviceIdentity::Gpu {
            uuid: [0; 16],
            pci_bus_id: Arc::from("0000:2d:00.0"),
        };
        vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            entry(2, gpu, None),
            disk(3, "sda"),
            disk(4, "sdb"),
            disk(5, "sdc"),
            disk(6, "sdd"),
            net(7, "enp38s0", NetKind::Wired),
            net(8, "tailscale0", NetKind::VpnTunnel),
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
        main: Entity<MainWindow>,
        page: Entity<PerformancePage>,
        demand: Entity<DemandAggregator>,
        hub: Entity<SnapshotHub>,
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

    /// The main window, `width` × `height`, on the Performance tab with the sidebar focused.
    fn open(cx: &mut TestAppContext, width: f32, height: f32, reduce_motion: bool) -> Rig<'_> {
        open_with(cx, width, height, reduce_motion, Settings::default())
    }

    /// [`open`] with `settings`. The Overview card is hidden unless `settings` has an entry
    /// for it (D-022).
    fn open_with(
        cx: &mut TestAppContext,
        width: f32,
        height: f32,
        reduce_motion: bool,
        settings: Settings,
    ) -> Rig<'_> {
        open_catalog(cx, width, height, reduce_motion, settings, catalog())
    }

    /// [`open_with`] on a catalog of `devices`.
    fn open_catalog(
        cx: &mut TestAppContext,
        width: f32,
        height: f32,
        reduce_motion: bool,
        mut settings: Settings,
        devices: Vec<DeviceEntry>,
    ) -> Rig<'_> {
        // Instant tab switches (E20-021): frame counts here must not see the switch fade.
        settings.motion.tab_switch_ms = 0;
        settings
            .performance
            .device_overrides
            .entry(OVERVIEW_KEY.to_owned())
            .or_insert(DeviceVisibility::Hide);
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: Arc::from(devices),
            });
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        let (hub, demand) = cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(reduce_motion);
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
            cx.set_global(BootSettings {
                settings,
                state: UiState::default(),
                persist: Some(Box::new(|_, _| {})),
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub.clone()));
            cx.set_global(GlobalDemand(demand.clone()));
            tl_ui_kit::components::toast::init(cx);
            tl_ui_kit::actions::bind_keys_global(cx);
            tl_ui_kit::components::menu::bind_keys(cx);
            tl_ui_perf::performance::actions::bind_keys_performance(cx);
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
            (hub, demand)
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let vcx = VisualTestContext::from_window(handle.into(), cx).into_mut();
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(width), px(height)));
        vcx.update(|window, _| window.activate_window());
        draw(vcx);
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        vcx.dispatch_action(tl_ui_kit::actions::SelectTab(Tab::Performance as usize));
        draw(vcx);
        // Let the tab switch fade (`motion.tab_switch_ms`, timed on the test clock) end, or it
        // asks for a frame on every later draw.
        let tab_ms = vcx.update(|_, cx| TlSettings::settings(cx).motion.tab_switch_ms);
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(u64::from(tab_ms)));
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
            hub,
            engine,
            vcx,
        }
    }

    fn key(s: &str) -> DeviceKey {
        DeviceKey(Arc::from(s))
    }

    fn summary(r: &mut Rig) -> Option<Entity<SummaryView>> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).summary().cloned())
    }

    /// Ctrl+M from the sidebar, then the `m_fast` fade-out when animations are on.
    fn enter(r: &mut Rig) -> Entity<SummaryView> {
        r.vcx.simulate_keystrokes("ctrl-m");
        draw(r.vcx);
        if summary(r).is_none() {
            r.vcx.executor().advance_clock(M_FAST);
            draw(r.vcx);
        }
        summary(r).expect("summary mode on")
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

    fn merged(r: &mut Rig) -> Demand {
        let demand = r.demand.clone();
        r.vcx.update(|_, cx| demand.read(cx).merged())
    }

    #[gpui_kit::test]
    fn summary_blocks_tab_switch(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., false);
        enter(&mut r);
        for keys in ["ctrl-2", "ctrl-1", "ctrl-3", "alt-1", "ctrl-tab"] {
            r.vcx.simulate_keystrokes(keys);
            draw(r.vcx);
            let tab = r.vcx.read_entity(&r.main, |m, _| m.active_tab());
            assert_eq!(tab, Tab::Performance, "{keys}");
            assert!(summary(&mut r).is_some(), "{keys}: summary still on");
        }
    }

    #[gpui_kit::test]
    fn summary_hides_title_bar_menus(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        assert!(r.vcx.debug_bounds("TitleBarMenu").is_some());
        enter(&mut r);
        assert!(
            r.vcx.debug_bounds("TitleBarMenu").is_none(),
            "hidden in summary mode"
        );
        r.vcx.simulate_keystrokes("alt-f");
        draw(r.vcx);
        assert!(summary(&mut r).is_some(), "alt-f opens no menu");
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        assert!(summary(&mut r).is_none());
        assert!(r.vcx.debug_bounds("TitleBarMenu").is_some(), "shown again");
    }

    fn right_click(cx: &mut VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        cx.run_until_parked();
    }

    /// A point inside the page panel's top-left padding, away from every Details value, so the
    /// page menu opens without "Copy value" whatever the values are.
    fn page_panel(r: &mut Rig) -> gpui_kit::Point<gpui_kit::Pixels> {
        let b = r
            .vcx
            .debug_bounds(tl_ui_perf::performance::PAGE_PANEL_ID)
            .expect("the page panel");
        b.origin + gpui_kit::point(gpui_kit::px(6.), gpui_kit::px(6.))
    }

    /// The summary sends its own part (CPU at the fast rate); the hidden device page withdraws
    /// its part; the sidebar keeps its own; a hidden window drops them all.
    #[gpui_kit::test]
    fn summary_demand_on_off_and_hidden(cx: &mut TestAppContext) {
        let mut r = open(cx, 1600., 1000., true);
        select(&mut r, "memory");
        let memory = {
            let page = r.page.clone();
            r.vcx.update(|_, cx| {
                page.read(cx)
                    .page(&key("memory"))
                    .expect("memory page built")
                    .clone()
                    .downcast::<MemoryPage>()
                    .expect("a MemoryPage")
            })
        };
        let off = merged(&mut r);
        assert!(off.any_surface_visible && off.memory);
        assert!(!off.cpu_fast, "the CPU page is not shown");

        enter(&mut r);
        let on = merged(&mut r);
        assert!(on.any_surface_visible);
        assert!(on.cpu_fast, "the CPU tile runs at 10 Hz");
        assert!(
            on.disks && on.nets && on.gpus,
            "the sidebar keeps its part: {on:?}"
        );
        assert!(
            !r.vcx.read_entity(&memory, |p, _| p.is_active()),
            "page part withdrawn"
        );

        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        let back = merged(&mut r);
        assert!(!back.cpu_fast, "the summary part is gone");
        assert!(r.vcx.read_entity(&memory, |p, _| p.is_active()));
        assert_eq!(back, off);

        // Hidden: a change waits over 1 s for a render, and every main-window part drops out.
        enter(&mut r);
        assert!(merged(&mut r).cpu_fast);
        r.engine
            .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
        r.vcx.run_until_parked();
        r.vcx.executor().advance_clock(Duration::from_millis(1600));
        r.vcx.run_until_parked();
        let hub = r.hub.clone();
        assert!(r.vcx.read_entity(&hub, |h, _| h.is_hidden()));
        assert_eq!(
            merged(&mut r),
            Demand {
                device_history: true,
                ..Demand::default()
            }
        );
    }

    fn clipboard(r: &mut Rig) -> Option<String> {
        r.vcx.read_from_clipboard().and_then(|item| item.text())
    }

    /// Publishes the fake CPU values of the copy tests: a model name, every Details value
    /// Valid, and the caches.
    fn publish_cpu(r: &mut Rig, temp: Validity) {
        use tl_model::snapshot::{
            CacheInfo, CacheLevelInfo, CpuStaticInfo, InvField, SystemSnapshot, VirtState,
        };

        fn inv<T>(value: T) -> InvField<T> {
            InvField {
                value,
                validity: Validity::Valid,
            }
        }
        fn level(instances: u16, instance_kib: u64) -> Option<CacheLevelInfo> {
            Some(CacheLevelInfo {
                instances,
                instance_kib,
                total_kib: u64::from(instances) * instance_kib,
            })
        }
        r.engine.publish_with(|s| {
            let sys: &mut SystemSnapshot = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.header.validity = Validity::Valid;
            let c = &mut sys.cpu;
            c.util_pct = Metric::valid(18.6);
            c.kernel_pct = Metric::valid(2.3);
            c.freq_mhz = Metric::valid(4631);
            c.base_mhz = Metric::valid(4200);
            c.temp_c = if temp == Validity::Valid {
                Metric::valid(52.0)
            } else {
                Metric::with(temp)
            };
            c.processes = Metric::valid(295);
            c.threads = Metric::valid(5596);
            c.handles = Metric::valid(253_248);
            c.load_avg = Metric::valid([1.24, 0.98, 0.87]);
            c.sockets = Metric::valid(1);
            let dd = Arc::make_mut(&mut s.device_details);
            dd.header.version += 1;
            dd.cpu = Arc::new(CpuStaticInfo {
                cores: 8,
                logical: 16,
                caches: CacheInfo {
                    l1d: level(8, 32),
                    l1i: level(8, 32),
                    l2: level(8, 1024),
                    l3: level(1, 98_304),
                    ..CacheInfo::default()
                },
                driver: inv(Arc::from("amd-pstate-epp")),
                governor: inv(Arc::from("powersave")),
                epp: inv(Arc::from("balance_performance")),
                platform_profile: inv(Arc::from("balanced")),
                virtualization: inv(VirtState::EnabledKvm),
                running_in_vm: inv(Arc::from("")),
                microcode: inv(Arc::from("0xa601206")),
                rapl: None,
                ..CpuStaticInfo::default()
            });
            let invt = Arc::make_mut(&mut s.inventory);
            invt.header.version += 1;
            invt.processor.name = inv(Arc::from("AMD Ryzen 7 7800X3D 8-Core Processor"));
        });
        draw(r.vcx);
    }

    /// Ctrl+C and the menu's Copy put the same text on the clipboard; each pushes the toast
    /// once.
    #[gpui_kit::test]
    fn copy_pushes_toast_once(cx: &mut TestAppContext) {
        use tl_ui_kit::components::toast::ToastQueue;
        use tl_ui_perf::performance::actions::PerfCopyDeviceInfo;

        fn toasts(r: &mut Rig) -> Vec<String> {
            r.vcx.update(|window, cx| {
                let queue = ToastQueue::of_window(window, cx).expect("a toast queue");
                queue
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .map(|t| t.to_string())
                    .collect()
            })
        }

        let mut r = open(cx, 1600., 1000., true);
        publish_cpu(&mut r, Validity::Valid);
        assert!(toasts(&mut r).is_empty());
        r.vcx.dispatch_action(PerfCopyDeviceInfo);
        r.vcx.run_until_parked();
        assert_eq!(toasts(&mut r), ["Copied metrics to clipboard"]);
        let by_action = clipboard(&mut r).expect("copied");

        // The menu's Copy (the first item off the values) copies the same text.
        r.vcx.update(|_, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()))
        });
        r.vcx
            .update(|_, cx| tl_ui_kit::components::menu::bind_keys(cx));
        let at = page_panel(&mut r);
        right_click(r.vcx, at);
        draw(r.vcx);
        r.vcx.simulate_keystrokes("down enter");
        r.vcx.run_until_parked();
        assert_eq!(clipboard(&mut r).as_deref(), Some(by_action.as_str()));
        assert_eq!(
            toasts(&mut r),
            ["Copied metrics to clipboard"],
            "one toast at a time"
        );
    }
}
