//! The machine status bar (UI-PRF-054): where it shows for each `performance.status_bar`
//! value, its text, and the Overview box state edges. The state rules have unit tests in
//! `pages/performance/status_bar/rules.rs`. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{
    AppearanceSettings, Settings, StatusBarPlacement, ThemeSettings, UiState,
};
use tl_model::snapshot::Snapshot;
use tl_model::system::{CpuSet, CpuTopology, GpuTotals, Metric};
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::components::menu::{close_context_menu, open_context_menu};
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::overview::layout::BoxKind;
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage, edge_id};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::status_bar::rules::{ChipKind, MachineState, Tone};
use tl_ui::pages::performance::status_bar::{BAR_ID, PILL_ID, StatusBar, WARNINGS_ID, chip_id};
use tl_ui::pages::performance::{DeviceKey, PerformancePage};
use tl_ui::settings_global::{SettingKey, SettingValue, TlSettings};
use tl_ui::testing::fake_engine::FakeEngine;

const NS: u64 = 1_000_000_000;

/// A debug selector built at run time, as the `'static` str `debug_bounds` takes.
fn sel(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn entry(idx: u16, identity: DeviceIdentity, hot_c: Option<f32>) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("dev"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
        hot_c,
    }
}

/// CPU package 0, memory 1, logical CPUs 100 to 103, and the k10temp Tctl sensor 20 with a
/// hot threshold of `cpu_hot_c`.
fn catalog(cpu_hot_c: Option<f32>) -> DeviceCatalog {
    let mut entries = vec![
        entry(0, DeviceIdentity::CpuPackage, None),
        entry(1, DeviceIdentity::Memory, None),
        entry(
            20,
            DeviceIdentity::Sensor {
                chip: Arc::from("k10temp"),
                label: Arc::from("Tctl"),
            },
            cpu_hot_c,
        ),
    ];
    entries.extend((0..4).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id }, None)));
    DeviceCatalog {
        version: 1,
        entries: Arc::from(entries),
    }
}

/// A calm machine at `t_s` seconds of boot time.
fn calm(s: &mut Snapshot, t_s: u64) {
    s.published_ns = t_s * NS;
    let sys = Arc::make_mut(&mut s.system);
    sys.header.version += 1;
    sys.cpu.util_pct = Metric::valid(10.);
    sys.cpu.temp_c = Metric::valid(50.);
    sys.cpu.load_avg = Metric::valid([1.5, 1., 1.]);
    sys.cpu.throttling = Metric::valid(false);
    sys.memory.psi_some_pct = Metric::valid(0.);
    sys.memory.psi_full_pct = Metric::valid(0.);
    sys.memory.swap_in_pps = Metric::valid(0.);
    let mut online = CpuSet::empty();
    (0..4).for_each(|c| online.insert(c));
    sys.cpu_topology = Arc::new(CpuTopology {
        online,
        cores: Arc::from(Vec::new()),
    });
}

/// Globals every rig needs, with `settings`.
fn install(cx: &mut TestAppContext, client: &Arc<dyn EngineClient>, settings: Settings) {
    let client = Arc::clone(client);
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
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
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

fn set_placement(vcx: &mut VisualTestContext, placement: StatusBarPlacement) {
    vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.performance.status_bar = placement);
    });
    draw(vcx);
}

mod performance_tab {
    use super::*;

    struct Rig<'a> {
        engine: Arc<FakeEngine>,
        page: Entity<PerformancePage>,
        vcx: &'a mut VisualTestContext,
    }

    fn open(cx: &mut TestAppContext) -> Rig<'_> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(catalog(None));
            calm(s, 100);
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        install(cx, &client, Settings::default());
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let (page, vcx) = cx.add_window_view(move |_, cx| PerformancePage::new(hub, demand, cx));
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1280.), px(800.)));
        draw(vcx);
        let mut r = Rig { engine, page, vcx };
        r.select(OVERVIEW_KEY);
        r
    }

    impl Rig<'_> {
        fn bar(&mut self) -> Entity<StatusBar> {
            let page = self.page.clone();
            self.vcx.read_entity(&page, |p, _| p.status_bar().clone())
        }

        fn shown(&mut self) -> bool {
            let bar = self.bar();
            self.vcx.read_entity(&bar, |b, _| b.is_shown())
        }

        fn select(&mut self, key: &str) {
            let page = self.page.clone();
            let key = DeviceKey(Arc::from(key));
            self.vcx.update(|_, cx| {
                let sidebar = page.read(cx).sidebar().clone();
                sidebar.update(cx, |s, cx| s.select(key, cx));
            });
            draw(self.vcx);
        }

        fn selected(&mut self) -> String {
            let page = self.page.clone();
            self.vcx
                .read_entity(&page, |p, _| p.selected().map(|k| k.0.to_string()))
                .unwrap_or_default()
        }
    }

    #[gpui_kit::test]
    fn status_bar_placement_setting(cx: &mut TestAppContext) {
        let mut r = open(cx);
        assert_eq!(r.selected(), OVERVIEW_KEY);
        // `overview` (the default): above the Overview page only, 64 px high.
        let bar = r.vcx.debug_bounds(BAR_ID).expect("bar on the Overview");
        assert_eq!(bar.size.height, px(64.));
        let pill = r.vcx.debug_bounds(PILL_ID).expect("pill");
        assert_eq!(pill.size.height, px(40.));
        assert!(r.shown(), "the bar sends its demand while shown");
        r.select(DeviceKey::CPU);
        assert!(r.vcx.debug_bounds(BAR_ID).is_none(), "not on the CPU page");
        assert!(!r.shown());

        // `performance`: above every page.
        set_placement(r.vcx, StatusBarPlacement::Performance);
        assert!(r.vcx.debug_bounds(BAR_ID).is_some(), "on the CPU page");
        assert!(r.shown());
        r.select(OVERVIEW_KEY);
        assert!(r.vcx.debug_bounds(BAR_ID).is_some(), "on the Overview");

        // `all_tabs` is the main window's slot; `off` draws it nowhere.
        // Written as the Settings window's "Machine status bar" dropdown writes them.
        for (value, placement) in [
            ("all_tabs", StatusBarPlacement::AllTabs),
            ("off", StatusBarPlacement::Off),
        ] {
            r.vcx.update(|_, cx| {
                TlSettings::set(
                    cx,
                    SettingKey::PerformanceStatusBar,
                    SettingValue::Str(value.into()),
                );
                assert_eq!(TlSettings::settings(cx).performance.status_bar, placement);
            });
            draw(r.vcx);
            assert!(r.vcx.debug_bounds(BAR_ID).is_none(), "{placement:?}");
            assert!(!r.shown(), "{placement:?}");
        }

        // The Overview page menu's "Status bar" radio items write the setting. The menu
        // dispatches to the focus inside the tab.
        let page = r.page.clone();
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).primary_focus();
            focus.focus(window, cx);
        });
        r.vcx
            .dispatch_action(tl_ui::pages::performance::actions::PerfSetStatusBar {
                placement: StatusBarPlacement::Overview,
            });
        draw(r.vcx);
        let placement = r
            .vcx
            .update(|_, cx| TlSettings::settings(cx).performance.status_bar);
        assert_eq!(placement, StatusBarPlacement::Overview);
        assert!(r.vcx.debug_bounds(BAR_ID).is_some());
        let _ = &r.engine;
    }

    /// UI-PRF-059: the pill, the bar's first item, starts at the x of the Overview boxes' left
    /// border, at both smoke sizes.
    #[gpui_kit::test]
    fn status_row_aligned_with_boxes(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::overview::cpu_box::CPU_BOX_ID;
        let r = &mut open(cx);
        for (w, h) in [(1920., 1200.), (1280., 800.)] {
            r.vcx.simulate_resize(size(px(w), px(h)));
            draw(r.vcx);
            let pill = r.vcx.debug_bounds(PILL_ID).expect("pill");
            let cpu = r.vcx.debug_bounds(CPU_BOX_ID).expect("CPU box");
            assert_eq!(pill.left(), cpu.left(), "{w}x{h}: {pill:?} over {cpu:?}");
        }
    }

    /// One row at 900 px and wider, never wrapping; below 900 px the chips take a second row.
    #[gpui_kit::test]
    fn status_bar_wraps_below_900(cx: &mut TestAppContext) {
        let mut r = open(cx);
        let chip_tops = |r: &mut Rig<'_>| -> Vec<_> {
            [ChipKind::Uptime, ChipKind::Load, ChipKind::CpuTemp]
                .into_iter()
                .map(|k| r.vcx.debug_bounds(sel(chip_id(k))).expect("chip").top())
                .collect()
        };
        let bar = r.vcx.debug_bounds(BAR_ID).expect("bar");
        assert!(bar.size.width >= px(900.), "{:?}", bar.size.width);
        assert_eq!(bar.size.height, px(64.));
        let pill = r.vcx.debug_bounds(PILL_ID).expect("pill");
        for top in chip_tops(&mut r) {
            assert!(
                top > pill.top() && top < pill.bottom(),
                "chips beside the pill"
            );
        }

        r.vcx.simulate_resize(size(px(800.), px(800.)));
        draw(r.vcx);
        let bar = r.vcx.debug_bounds(BAR_ID).expect("bar");
        assert!(bar.size.width < px(900.), "{:?}", bar.size.width);
        assert!(bar.size.height > px(64.));
        let pill = r.vcx.debug_bounds(PILL_ID).expect("pill");
        let tops = chip_tops(&mut r);
        assert!(tops[0] >= pill.bottom(), "chips under the pill");
        assert!(tops.iter().all(|t| *t == tops[0]), "one chip row: {tops:?}");
    }

    #[gpui_kit::test]
    fn status_bar_pill_and_warnings(cx: &mut TestAppContext) {
        let mut r = open(cx);
        // A CPU at its threshold: the chip is red at once, the state Hot after 30 s.
        r.engine
            .publish_with(|s| s.devices = Arc::new(catalog(Some(95.))));
        for t in 101..=131 {
            r.engine.publish_with(|s| {
                calm(s, t);
                Arc::make_mut(&mut s.system).cpu.temp_c = Metric::valid(97.);
            });
            r.vcx.run_until_parked();
        }
        draw(r.vcx);
        let bar = r.bar();
        let text = r.vcx.read_entity(&bar, |b, _| b.text().clone());
        assert_eq!(text.state, MachineState::Hot);
        assert_eq!(&*text.pill, "Hot: CPU 97 °C for 30 s");
        assert_eq!(
            text.pill_target.as_ref().map(|k| &*k.0),
            Some(DeviceKey::CPU)
        );
        let chip = text
            .chips
            .iter()
            .find(|c| c.kind == ChipKind::CpuTemp)
            .expect("CPU temperature chip");
        assert_eq!((&*chip.value, chip.tone), ("97 °C", Tone::Danger));
        assert_eq!(chip.warning.as_deref(), Some("97 °C (threshold 95 °C)"));
        assert_eq!(&*text.warnings, "1 warning");
        assert!(r.vcx.debug_bounds(WARNINGS_ID).is_some());
        assert!(
            r.vcx
                .debug_bounds(sel(chip_id(ChipKind::CpuTemp)))
                .is_some()
        );
        let load = text
            .chips
            .iter()
            .find(|c| c.kind == ChipKind::Load)
            .unwrap();
        assert_eq!(&*load.value, "1.50 / 4");

        // Cool again: no red chip, no button; the state holds 10 s, then Calm.
        for t in 132..=150 {
            r.engine.publish_with(|s| calm(s, t));
            r.vcx.run_until_parked();
        }
        draw(r.vcx);
        let text = r.vcx.read_entity(&bar, |b, _| b.text().clone());
        assert_eq!(text.state, MachineState::Calm);
        assert_eq!(&*text.pill, "Calm");
        assert!(text.warnings.is_empty());
        assert!(r.vcx.debug_bounds(WARNINGS_ID).is_none());
    }

    /// A GPU at 50 °C.
    fn gpu(idx: u16) -> (DeviceIdx, GpuTotals) {
        let totals = GpuTotals {
            device: DeviceIdx(idx),
            util_pct: Metric::valid(5.),
            mem_util_pct: Metric::default(),
            mem_used: Metric::default(),
            mem_total: Metric::default(),
            shared_used: Metric::default(),
            shared_total: Metric::default(),
            mem_reserved: Metric::default(),
            enc_pct: Metric::default(),
            dec_pct: Metric::default(),
            enc_sessions: Metric::default(),
            enc_dec_shared: false,
            temp_c: Metric::valid(50.),
            temp_edge_c: Metric::default(),
            temp_junction_c: Metric::default(),
            temp_mem_c: Metric::default(),
            power_w: Metric::default(),
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
        };
        (DeviceIdx(idx), totals)
    }

    /// With more chips than fit, the warning button keeps its place at the right end of the
    /// bar, fully visible, and a click on it opens the popup. Checked with the bar 900 px and
    /// 1280 px wide.
    #[gpui_kit::test]
    fn status_bar_warning_button_never_pushed_out(cx: &mut TestAppContext) {
        let mut r = open(cx);
        // A red CPU chip and 12 GPU temperature chips.
        r.engine.publish_with(|s| {
            s.devices = Arc::new(catalog(Some(95.)));
            calm(s, 101);
            let sys = Arc::make_mut(&mut s.system);
            sys.cpu.temp_c = Metric::valid(97.);
            sys.gpus = Arc::from((200..212).map(gpu).collect::<Vec<_>>());
        });
        draw(r.vcx);
        let bar = r.bar();
        let text = r.vcx.read_entity(&bar, |b, _| b.text().clone());
        assert_eq!(&*text.warnings, "1 warning");
        let last = sel(chip_id(text.chips.last().expect("chips").kind));

        for width in [900., 1280.] {
            // Size the window so the bar is `width` wide.
            let window = r.vcx.update(|window, _| window.viewport_size());
            let bar = r.vcx.debug_bounds(BAR_ID).expect("bar");
            let w = window.width + px(width) - bar.size.width;
            r.vcx.simulate_resize(size(w, window.height));
            draw(r.vcx);
            let bar = r.vcx.debug_bounds(BAR_ID).expect("bar");
            assert_eq!(bar.size.width, px(width));
            assert_eq!(bar.size.height, px(64.), "{width}: one row");

            let button = r.vcx.debug_bounds(WARNINGS_ID).expect("warning button");
            assert!(
                button.left() >= bar.left()
                    && button.right() <= bar.right()
                    && button.top() >= bar.top()
                    && button.bottom() <= bar.bottom(),
                "{width}: button {button:?} inside bar {bar:?}"
            );
            let last = r.vcx.debug_bounds(last).expect("last chip");
            assert!(
                last.right() > button.left(),
                "{width}: more chips than fit, last chip {last:?}"
            );
            let first = r
                .vcx
                .debug_bounds(sel(chip_id(ChipKind::Uptime)))
                .expect("first chip");
            assert!(first.right() <= button.left(), "{width}: first chip shown");

            r.vcx.simulate_click(button.center(), Modifiers::none());
            draw(r.vcx);
            let rows = r.vcx.update(|window, cx| {
                let menu = open_context_menu(window, cx).expect("the popup is open");
                menu.read(cx)
                    .entries_for_test(window)
                    .into_iter()
                    .filter_map(|e| e.label.map(|l| l.to_string()))
                    .collect::<Vec<_>>()
            });
            assert_eq!(rows, ["CPU"], "{width}");
            r.vcx.update(close_context_menu);
            draw(r.vcx);
            let open = r
                .vcx
                .update(|window, cx| open_context_menu(window, cx).is_some());
            assert!(!open, "{width}: popup closed");
        }
    }
}

mod overview {
    use super::*;

    struct Host {
        page: Entity<OverviewPage>,
        sidebar: Entity<DeviceSidebar>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .child(
                    self.page
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
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

    #[gpui_kit::test]
    fn overview_box_state_edge(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(catalog(None));
            calm(s, 100);
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        install(cx, &client, Settings::default());
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let (host, vcx) = cx.add_window_view(move |_, cx| {
            let sidebar = cx.new(|cx| DeviceSidebar::new(hub.clone(), demand.clone(), cx));
            let page = cx.new(|cx| {
                OverviewPage::new(
                    DeviceKey(Arc::from(OVERVIEW_KEY)),
                    hub,
                    demand,
                    &sidebar,
                    cx,
                )
            });
            Host { page, sidebar }
        });
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1200.), px(900.)));
        draw(vcx);
        let page = vcx.read_entity(&host, |h, _| h.page.clone());
        let theme = vcx.update(|_, cx| tl_ui::theme::current(cx).clone());
        let edge = |vcx: &mut VisualTestContext, kind| {
            vcx.read_entity(&page, |p, cx| p.edge_color(kind, cx))
        };
        // Calm: no edge on any box.
        for kind in [BoxKind::Cpu, BoxKind::Memory, BoxKind::Disk, BoxKind::Net] {
            assert_eq!(edge(vcx, kind), None, "{kind:?}");
            assert!(vcx.debug_bounds(sel(edge_id(kind))).is_none(), "{kind:?}");
        }

        // CPU busy for 30 s and memory pressure: both warning.
        for t in 101..=131 {
            engine.publish_with(|s| {
                calm(s, t);
                let sys = Arc::make_mut(&mut s.system);
                sys.cpu.util_pct = Metric::valid(90.);
                sys.memory.psi_some_pct = Metric::valid(6.);
            });
            vcx.run_until_parked();
        }
        draw(vcx);
        assert_eq!(edge(vcx, BoxKind::Cpu), Some(theme.status.warning));
        assert_eq!(edge(vcx, BoxKind::Memory), Some(theme.status.warning));
        assert_eq!(edge(vcx, BoxKind::Net), None);
        let cpu_box = vcx.debug_bounds("perf-overview-box-cpu").expect("CPU box");
        let cpu_edge = vcx
            .debug_bounds(sel(edge_id(BoxKind::Cpu)))
            .expect("CPU box edge");
        assert_eq!(cpu_edge.size.height, px(2.));
        assert_eq!(cpu_edge.top(), cpu_box.top());
        assert_eq!(cpu_edge.size.width, cpu_box.size.width);

        // CPU throttling (Hot) and memory PSI full (Saturated): both danger.
        engine.publish_with(|s| {
            calm(s, 132);
            let sys = Arc::make_mut(&mut s.system);
            sys.cpu.throttling = Metric::valid(true);
            sys.memory.psi_full_pct = Metric::valid(3.);
        });
        draw(vcx);
        assert_eq!(edge(vcx, BoxKind::Cpu), Some(theme.status.danger));
        assert_eq!(edge(vcx, BoxKind::Memory), Some(theme.status.danger));

        // Calm again: the edges go after the 10 s hold.
        for t in 133..=145 {
            engine.publish_with(|s| calm(s, t));
            vcx.run_until_parked();
        }
        draw(vcx);
        assert_eq!(edge(vcx, BoxKind::Cpu), None);
        assert_eq!(edge(vcx, BoxKind::Memory), None);
        assert!(vcx.debug_bounds(sel(edge_id(BoxKind::Cpu))).is_none());
    }
}

mod main_window {
    use super::*;
    use gpui_kit::{EventEmitter, FocusHandle, InteractiveElement as _};
    use tl_model::command::Tab;
    use tl_ui::demand::GlobalDemand;
    use tl_ui::hub::GlobalHub;
    use tl_ui::window::main::{
        self as shell, MainWindow, PageSlot, ShellPage, open_main_window, set_test_page,
    };
    use tl_ui::window::status::StatusChanged;

    struct FakePage {
        focus: FocusHandle,
    }

    impl EventEmitter<StatusChanged> for FakePage {}

    impl Render for FakePage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus).size_full()
        }
    }

    impl ShellPage for FakePage {
        fn primary_focus(&self) -> FocusHandle {
            self.focus.clone()
        }
    }

    fn fake(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let page = cx.new(|cx| FakePage {
            focus: cx.focus_handle().tab_stop(true),
        });
        PageSlot::new(page, cx)
    }

    /// `all_tabs`: the main window draws the bar under the tab strip, above the page.
    #[gpui_kit::test]
    fn status_bar_all_tabs_under_tab_strip(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(catalog(None));
            calm(s, 100);
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        let mut settings = Settings::default();
        settings.performance.status_bar = StatusBarPlacement::AllTabs;
        settings.motion.tab_switch_ms = 0;
        settings.general.start_tab = tl_model::settings::StartTab::Processes;
        install(cx, &client, settings);
        cx.update(|cx| {
            let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub));
            cx.set_global(GlobalDemand(demand));
            tl_ui::components::toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            for tab in [Tab::Processes, Tab::Services] {
                set_test_page(cx, tab, fake);
            }
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        let bar = vcx.debug_bounds(BAR_ID).expect("bar under the tab strip");
        let page = vcx.debug_bounds("MainWindowPage").expect("page");
        assert_eq!(bar.bottom(), page.top());
        assert_eq!(bar.size.width, page.size.width);
        // With no Performance view open, the bar alone asks for what its rules read, the
        // network adapters for the network Busy rule among them.
        let merged = vcx.update(|_, cx| cx.global::<GlobalDemand>().0.read(cx).merged());
        assert!(merged.nets, "network Busy needs the adapters");
        assert!(merged.memory && merged.gpus && merged.sensors && merged.disks);

        set_placement(&mut vcx, StatusBarPlacement::Overview);
        assert!(
            vcx.debug_bounds(BAR_ID).is_none(),
            "not on the Processes tab"
        );
    }
}
