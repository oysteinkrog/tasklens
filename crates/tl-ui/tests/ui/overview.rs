//! Performance tab Overview page inside the main window (crw.42). The drawing parts and the
//! boxes moved to the tl-ui-overview `overview` binary and the OverviewPage tests to the
//! tl-ui-perf `perf` binary (tasklens-zljr5.41); these need MainWindow, its title bar and the
//! tab switch.

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
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::process::{ProcFlags, ProcessRecord};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, SeriesSet};
    use tl_model::time::now_boot_ns;
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
    use tl_ui::window::main::{
        self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
    };
    use tl_ui::window::titlebar::TitleBarProbe;
    use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
    use tl_ui_base::hub::{GlobalHub, SnapshotHub};
    use tl_ui_base::i18n::init_strings;
    use tl_ui_base::settings_global::TlSettings;
    use tl_ui_base::theme::current;
    use tl_ui_chart::chart::labels::LabelKind;
    use tl_ui_kit::components::kv_grid;
    use tl_ui_kit::components::menu::{MenuEntryKind, open_context_menu};
    use tl_ui_perf::performance::actions::PerfOverviewEditLayout;
    use tl_ui_perf::performance::overview::cpu_box::CPU_BOX_ID;
    use tl_ui_perf::performance::overview::disk_box::{DISK_BOX_ID, DISK_ROW_ID_PREFIX};
    use tl_ui_perf::performance::overview::edit::title_handle_id;
    use tl_ui_perf::performance::overview::gpu_box::GPU_BOX_ID;
    use tl_ui_perf::performance::overview::mem_box::MEM_BOX_ID;
    use tl_ui_perf::performance::overview::net_box::NET_BOX_ID;
    use tl_ui_perf::performance::overview::parts::{
        body_id, box_accent, title_name_id, title_row_id,
    };
    use tl_ui_perf::performance::overview::procs_box::{
        PROC_ROW_ID_PREFIX, PROCS_BOX_ID, ProcsBox,
    };
    use tl_ui_perf::performance::overview::{OVERVIEW_KEY, OverviewPage};
    use tl_ui_perf::performance::page::DevicePage as _;
    use tl_ui_perf::performance::{
        DeviceKey, DeviceKind, PAGE_PANEL_ID, PerformancePage, SIDEBAR_PANEL_ID,
    };
    use tl_ui_settings::settings::{
        AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
    };
    use tl_ui_testkit::fake_engine::FakeEngine;
    use tl_ui_testkit::fixtures::{proc_key, proc_record};

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
            tl_ui_base::theme::store::init(
                cx,
                &theme_settings,
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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
            tl_ui_kit::components::toast::init(cx);
            tl_ui_kit::actions::bind_keys_global(cx);
            tl_ui_perf::performance::actions::bind_keys_performance(cx);
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
        vcx.dispatch_action(tl_ui_kit::actions::SelectTab(Tab::Performance as usize));
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
        let card = r.vcx.debug_bounds("perf-card:cpu").expect("a card");
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
            .dispatch_action(tl_ui_kit::actions::SelectTab(Tab::Processes as usize));
        draw(r.vcx);
        let away = merged(&mut r);
        assert!(!away.disks && !away.cpu_fast, "{away:?}");
        r.vcx
            .dispatch_action(tl_ui_kit::actions::SelectTab(Tab::Performance as usize));
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
        r.vcx
            .update(|_, cx| tl_ui_kit::components::menu::bind_keys(cx));
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
            // The five mode radios come first.
            entries[5..].to_vec()
        };
        // The Arrange submenu closes the list, after a separator.
        let arrange = || (MenuEntryKind::Submenu, "Arrange".to_owned(), None, false);
        let sep = || (MenuEntryKind::Separator, String::new(), None, false);
        // The CPU box also has the "Show core clocks" check item (tasklens-o55.57), right
        // after the power items, or after the mode radios when there are none.
        let clocks = || {
            (
                MenuEntryKind::Item,
                "Show core clocks".to_owned(),
                None,
                false,
            )
        };
        assert_eq!(
            tail(&mut r, Some(RaplState::NoZone)),
            [clocks(), sep(), arrange()]
        );
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
                clocks(),
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
                clocks(),
                sep(),
                arrange(),
            ]
        );
    }

    #[gpui_kit::test]
    fn overview_box_menu_switches_mode_and_saves(cx: &mut TestAppContext) {
        let mut r = open(cx);
        select(&mut r, OVERVIEW_KEY);
        r.vcx
            .update(|_, cx| tl_ui_kit::components::menu::bind_keys(cx));
        let page = overview(&mut r).expect("an OverviewPage");
        let saved = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| TlSettings::settings(cx).performance.overview.clone())
        };
        let cpu_split = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                page.read(cx)
                    .layout()
                    .split(tl_ui_perf::performance::overview::layout::BoxKind::Cpu)
            })
        };
        let title = r
            .vcx
            .debug_bounds(Box::leak(title_row_id(CPU_BOX_ID).into_boxed_str()))
            .expect("the CPU title line");

        // Right-click on the title line: Layout, Edit Widgets (the CPU box is on widgets),
        // Reset Box, a separator, then the page menu.
        mouse(r.vcx, MouseButton::Right, title.center());
        let (labels, modes) = open_menu(&mut r);
        assert_eq!(
            labels[..5],
            [
                (MenuEntryKind::Submenu, "Layout".to_owned()),
                (MenuEntryKind::Item, "Edit Widgets…".to_owned()),
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
                ("By app".to_owned(), false),
                // The default snapshot reads as RAPL readable.
                (String::new(), false),
                ("Show power draw".to_owned(), false),
                ("Show core clocks".to_owned(), false),
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
        r.vcx.simulate_keystrokes("down down down enter");
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
                title_handle_id(tl_ui_perf::performance::overview::layout::BoxKind::Cpu)
                    .into_boxed_str(),
            ))
            .expect("the CPU title handle");
        mouse(r.vcx, MouseButton::Right, handle.center());
        let (labels, modes) = open_menu(&mut r);
        assert_eq!(
            labels[..4],
            [
                (MenuEntryKind::Submenu, "Layout".to_owned()),
                (MenuEntryKind::Item, "Edit Widgets…".to_owned()),
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
                ("By app".to_owned(), false),
                // The default snapshot reads as RAPL readable.
                (String::new(), false),
                ("Show power draw".to_owned(), false),
                ("Show core clocks".to_owned(), false),
                (String::new(), false),
                ("Arrange".to_owned(), false),
            ]
        );
    }

    /// While the GPU box is in Edit Widgets the rest of the page takes no pointer input but the
    /// wheel (UI-PRF-061): no box menu, no page menu, and the page still scrolls. Ending Edit
    /// Widgets takes the covers away.
    #[gpui_kit::test]
    fn overview_widget_edit_page_takes_only_scroll(cx: &mut TestAppContext) {
        use gpui_kit::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};
        use tl_ui_perf::performance::actions::PerfWidgetEditStart;
        use tl_ui_perf::performance::overview::box_menu::widget_edit_cover_id;
        use tl_ui_perf::performance::overview::layout::BoxKind;

        // Every card on, so the Overview shows the GPU box.
        let mut r = open_with(cx, true);
        select(&mut r, OVERVIEW_KEY);
        let page = overview(&mut r).expect("an OverviewPage");
        // The GPU box hosts the widget editor (tasklens-lhliq.7).
        let editor = r
            .vcx
            .update(|_, cx| page.read(cx).gpu_box().read(cx).editor().clone());
        draw(r.vcx);
        let cover =
            |kind| -> &'static str { Box::leak(widget_edit_cover_id(kind).into_boxed_str()) };
        assert!(r.vcx.debug_bounds(cover(BoxKind::Cpu)).is_none());
        r.vcx.update(|window, cx| {
            let focus = gpui_kit::Focusable::focus_handle(page.read(cx), cx);
            focus.dispatch_action(&PerfWidgetEditStart, window, cx);
        });
        draw(r.vcx);
        assert!(r.vcx.read_entity(&editor, |e, _| e.is_editing()));
        let cpu = r.vcx.debug_bounds(CPU_BOX_ID).expect("the CPU box");
        let cpu_cover = r
            .vcx
            .debug_bounds(cover(BoxKind::Cpu))
            .expect("the CPU box cover");
        assert!(
            cpu_cover.contains(&cpu.center())
                && cpu_cover.size.width >= cpu.size.width - px(1.)
                && cpu_cover.size.height >= cpu.size.height - px(1.),
            "the cover is the whole box: {cpu_cover:?} {cpu:?}"
        );
        let gpu_title = r
            .vcx
            .debug_bounds(Box::leak(title_row_id(GPU_BOX_ID).into_boxed_str()))
            .expect("the GPU title line");
        let gpu_cover = r
            .vcx
            .debug_bounds(cover(BoxKind::Gpu))
            .expect("the GPU title cover");
        assert!(
            gpu_cover.contains(&gpu_title.center()),
            "{gpu_cover:?} {gpu_title:?}"
        );
        let gpu = r.vcx.debug_bounds(GPU_BOX_ID).expect("the GPU box");
        assert!(
            gpu_cover.size.height < gpu.size.height / 2.,
            "the body stays open"
        );

        // No box menu, no page menu.
        let cpu_title = r
            .vcx
            .debug_bounds(Box::leak(title_row_id(CPU_BOX_ID).into_boxed_str()))
            .expect("the CPU title line");
        for at in [cpu_title.center(), cpu.center(), gpu_title.center()] {
            mouse(r.vcx, MouseButton::Right, at);
            draw(r.vcx);
            let open = r
                .vcx
                .update(|window, cx| open_context_menu(window, cx).is_some());
            assert!(!open, "a right-click at {at:?} opens no menu");
        }

        // The wheel over a covered box still scrolls the page. A short window, so it scrolls.
        r.vcx.simulate_resize(size(px(1600.), px(420.)));
        draw(r.vcx);
        let cpu = r.vcx.debug_bounds(CPU_BOX_ID).expect("the CPU box");
        assert!(r.vcx.debug_bounds(cover(BoxKind::Cpu)).is_some());
        let top = cpu.origin.y;
        r.vcx.simulate_event(ScrollWheelEvent {
            position: cpu.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        draw(r.vcx);
        let scrolled = r
            .vcx
            .debug_bounds(CPU_BOX_ID)
            .expect("the CPU box")
            .origin
            .y;
        assert!(scrolled < top, "the page scrolled: {top:?} to {scrolled:?}");

        // Cancel ends Edit Widgets; the covers go and the box menu opens again. The full
        // height puts the CPU title line back in view.
        r.vcx.simulate_resize(size(px(1600.), px(1000.)));
        draw(r.vcx);
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
        draw(r.vcx);
        assert!(r.vcx.debug_bounds(cover(BoxKind::Cpu)).is_none());
        assert!(r.vcx.debug_bounds(cover(BoxKind::Gpu)).is_none());
        let cpu_title = r
            .vcx
            .debug_bounds(Box::leak(title_row_id(CPU_BOX_ID).into_boxed_str()))
            .expect("the CPU title line");
        mouse(r.vcx, MouseButton::Right, cpu_title.center());
        let (labels, _) = open_menu(&mut r);
        assert_eq!(labels[0], (MenuEntryKind::Submenu, "Layout".to_owned()));
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
        r.vcx
            .update(|_, cx| tl_ui_kit::components::menu::bind_keys(cx));
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
            .dispatch_action(tl_ui_kit::actions::SelectTab(Tab::Processes as usize));
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
            .dispatch_action(tl_ui_kit::actions::SelectTab(Tab::Performance as usize));
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
