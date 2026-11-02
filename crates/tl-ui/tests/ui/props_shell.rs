//! The Process Properties tests that need `MainWindow` (E23, UI-PWN-003 to UI-PWN-009): Go to
//! service and Go to connection reach the Services and Connections pages through the main
//! window, and the pin button sends its KWin command through the shell's pin bridge. The other
//! 131 tests of the old `props` module run on the page host in tl-ui-procs. The fixtures here are
//! the ones those four tests use, copied from there.

use std::sync::Arc;

use gpui_kit::{AppContext as _, Entity, Pixels, TestAppContext, VisualTestContext, px};
use tl_model::actions::SchedPolicy;
use tl_model::details::{
    ElfArch, ExeLocation, GeneralDetail, HashState, IdTriple, PackageCheck, PackageInfo, Relro,
    Seccomp, TabPayload,
};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::system::CpuSet;
use tl_model::table::Total;
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::host::HostContext;
use tl_ui::windows::props::general::GeneralTab;

const KEY: ProcessKey = ProcessKey {
    start_ticks: 77,
    boot_id: BootId(1),
    pid: 4321,
};

fn valid<T>(value: T) -> Total<T> {
    Total {
        value,
        validity: Validity::Valid,
    }
}

fn text(s: &str) -> Arc<str> {
    Arc::from(s)
}

fn cpus(ranges: &[(u16, u16)]) -> CpuSet {
    let mut set = CpuSet::empty();
    for &(a, b) in ranges {
        for cpu in a..=b {
            set.insert(cpu);
        }
    }
    set
}

fn detail(cmdline: &str) -> GeneralDetail {
    GeneralDetail {
        started_s: valid(1_760_000_000),
        uids: valid(IdTriple {
            real: 1000,
            effective: 0,
            saved: 0,
            fs: 0,
        }),
        uid_names: valid((text("oystein"), text("root"), text("root"))),
        policy: valid(SchedPolicy::Other),
        rt_prio: 0,
        kernel_prio: valid(120),
        autogroup: valid((text("autogroup-12"), 0)),
        affinity: valid(cpus(&[(0, 7), (16, 23)])),
        affinity_threads_differ: true,
        pie: valid(true),
        nx_stack: valid(true),
        relro: valid(Relro::Full),
        shadow_stack: Total {
            value: false,
            validity: Validity::Unsupported,
        },
        aslr_system: valid(2),
        aslr_disabled_for_process: valid(false),
        no_new_privs: valid(false),
        seccomp: valid(Seccomp::Filter { filters: 3 }),
        architecture: valid(ElfArch::X86_64),
        exe: valid(text("/usr/lib/firefox/firefox")),
        exe_deleted: false,
        location: valid(ExeLocation::System),
        package: valid(Some(PackageInfo {
            name: text("firefox"),
            version: text("131.0-1"),
            packager: Some(text("Arch Linux")),
        })),
        description: valid(text("Web browser")),
        cwd: valid(text("/home/oystein")),
        root: valid(text("/")),
        cgroup: Total {
            value: Some(text("/user.slice/app.slice/app-firefox.scope")),
            validity: Validity::Valid,
        },
        unit: Total {
            value: None,
            validity: Validity::Valid,
        },
        app_id: None,
        cmdline: valid(text(cmdline)),
        hashes: HashState::Pending,
        package_check: PackageCheck::NotVerified,
        handles: valid(412),
        rss_shared_bytes: valid(64 << 20),
        commit_bytes: valid(512 << 20),
        cpu_time_ms: valid(43_424_000),
        io_rchar: valid(18 << 20),
        io_wchar: valid(2 << 20),
        io_syscr: valid(12_345),
        io_syscw: valid(678),
        disk_read_bytes: valid(18 << 20),
        disk_write_bytes: valid(1 << 20),
        cancelled_write_bytes: valid(0),
        gpu_mem_shared_bytes: Total {
            value: 0,
            validity: Validity::Unsupported,
        },
        gpu_engine: valid(None),
    }
}

fn record(cpu_pct: f32) -> ProcessRecord {
    let name = text("firefox");
    ProcessRecord {
        key: KEY,
        ppid: 1200,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: name.clone(),
            name: name.clone(),
            exe: None,
            cmdline: None,
            user: text("oystein"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: name,
        }),
        group: GroupKey(0),
        state: ProcState::Running,
        flags: ProcFlags::default(),
        nice: 0,
        threads: 97,
        cpu_pct,
        rss_anon_bytes: 800 << 20,
        rss_bytes: 900 << 20,
        pss_bytes: 700 << 20,
        uss_bytes: 600 << 20,
        swap_bytes: 0,
        io_read_bps: 0.,
        io_write_bps: 0.,
        net_rx_bps: 150_000.,
        net_tx_bps: 37_500.,
        gpu_util_pct: 0.,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.0,
        validity: FieldValidity::all(Validity::Valid),
    }
}

fn payload(d: GeneralDetail) -> Arc<TabPayload> {
    Arc::new(TabPayload::General(Arc::new(d)))
}

// ---- App Properties General (E23-026, UI-PWN-019) ----

mod window {

    use gpui_kit::{AnyWindowHandle, WindowHandle};

    use tl_model::client::EngineClient;
    use tl_model::command::{Command, KWinCommand, Tab};

    use tl_model::details::{DetailTab, DetailTarget, TargetDetail};

    use tl_model::ids::NodeId;
    use tl_model::process::{GroupKind, GroupRow, GroupRowKind};

    use tl_ui::app::{AppEnv, TasklensUi, UiInit};

    use tl_ui::components::window_frame::TlSecondaryWindow;

    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui::window::registry::{GlobalRegistry, WindowKey};
    use tl_ui::windows::app_props::{AppPropsWindow, open_app_properties};
    use tl_ui::windows::props::{PropsWindow, open_process_properties};
    use tl_ui_settings::settings::{Settings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::*;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey { pid, ..KEY }
    }

    fn rec(pid: u32, flags: ProcFlags) -> ProcessRecord {
        let mut r = record(1.0);
        r.key = key(pid);
        r.flags = flags;
        r
    }

    /// App globals and the main window on Processes.
    fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let init = UiInit {
            settings: Settings::default(),
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: Vec::new(),
            env: AppEnv::test(),
        };
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Processes, cx))
            .expect("the main window opens");
        let mut main = VisualTestContext::from_window(handle.into(), cx);
        main.update(|window, _| window.activate_window());
        main.run_until_parked();
        (engine, main)
    }

    /// Publishes `records` (any order) as the process list.
    fn publish_records(
        cx: &mut TestAppContext,
        engine: &FakeEngine,
        mut records: Vec<ProcessRecord>,
    ) {
        records.sort_by_key(|r| r.key);
        engine.publish_with(|s| {
            let p = Arc::make_mut(&mut s.processes);
            p.records = Arc::from(records);
            p.header.version += 1;
        });
        cx.run_until_parked();
    }

    fn app_group(name: &str, kind: GroupRowKind, members: u32) -> GroupRow {
        let class = GroupKind::App {
            app_id: Arc::from(name.to_lowercase()),
        };
        let key = class.key();
        GroupRow {
            node: NodeId::Group(key),
            key,
            kind,
            class,
            name: Arc::from(name),
            icon: None,
            members,
            pids: members,
            note: None,
            memory_bytes: 0,
            disk_bps: 0.0,
            net_bps: 0.0,
            gpu_pct: 0.0,
            cpu_pct: 1.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            threads: 0,
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            commit_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            handles: None,
            desktop_path: None,
            description: None,
            user: None,
            frozen: false,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn publish_groups(cx: &mut TestAppContext, engine: &FakeEngine, groups: Vec<GroupRow>) {
        engine.publish_with(|s| {
            let table = Arc::make_mut(&mut s.table);
            table.groups = Arc::from(groups);
            table.header.version += 1;
        });
        cx.run_until_parked();
    }

    /// Edits the detail of `pid` and publishes it.
    fn publish_detail(
        cx: &mut TestAppContext,
        engine: &FakeEngine,
        pid: u32,
        f: impl FnOnce(&mut TargetDetail),
    ) {
        let target = DetailTarget::Process(key(pid));
        engine.publish_with(|s| {
            let d = Arc::make_mut(&mut s.details);
            let mut detail = d
                .targets
                .get(&target)
                .map(|t| TargetDetail::clone(t))
                .unwrap_or_default();
            f(&mut detail);
            d.targets.insert(target, Arc::new(detail));
            d.header.version += 1;
        });
        cx.run_until_parked();
    }

    fn open(cx: &mut TestAppContext, pid: u32) -> Option<AnyWindowHandle> {
        cx.update(|cx| open_process_properties(&HostContext::local(cx), key(pid), cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.read(cx).get(&WindowKey::Properties(key(pid)))
        })
    }

    fn open_app(cx: &mut TestAppContext, group: GroupKey) -> Option<AnyWindowHandle> {
        cx.update(|cx| open_app_properties(&HostContext::local(cx), group, cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.read(cx).get(&WindowKey::AppProperties(group))
        })
    }

    fn props(cx: &mut TestAppContext, handle: AnyWindowHandle) -> Entity<PropsWindow> {
        cx.update(|cx| PropsWindow::find(handle, cx))
            .expect("props window")
    }

    fn app_props(cx: &mut TestAppContext, handle: AnyWindowHandle) -> Entity<AppPropsWindow> {
        cx.update(|cx| AppPropsWindow::find(handle, cx))
            .expect("app props window")
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn main_toasts(main: &mut VisualTestContext) -> Vec<String> {
        main.update(|_, cx| {
            let view = main_window_view(cx).expect("main window");
            let texts = view.read(cx).toasts().read(cx).visible_texts();
            texts.iter().map(|t| t.to_string()).collect()
        })
    }

    /// Pinning Process or App Properties sends `KeepAbove` for that window's caption. Their
    /// frames carry the registry key: without it the pin only changed its look, which the old
    /// `Floating` kind hid (tasklens-fut.25, R-073, D-031).
    #[gpui_kit::test]
    fn props_pin_sends_keep_above(cx: &mut TestAppContext) {
        let (engine, _main) = setup(cx);
        engine.publish_with(|s| {
            let windows = Arc::make_mut(&mut s.windows);
            windows.header.validity = Validity::Valid;
            windows.header.version += 1;
        });
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let app = app_group("Firefox", GroupRowKind::App, 34);
        publish_groups(cx, &engine, vec![app.clone()]);
        let handle = open(cx, 4321).expect("opened");
        let app_handle = open_app(cx, app.key).expect("app window opened");

        let frame = cx
            .update(|cx| {
                TlSecondaryWindow::<PropsWindow>::find(WindowHandle::new(handle.window_id()), cx)
            })
            .expect("frame");
        let app_frame = cx
            .update(|cx| {
                TlSecondaryWindow::<AppPropsWindow>::find(
                    WindowHandle::new(app_handle.window_id()),
                    cx,
                )
            })
            .expect("app frame");
        assert_eq!(
            cx.update(|cx| frame.read(cx).key().cloned()),
            Some(WindowKey::Properties(key(4321)))
        );
        assert_eq!(
            cx.update(|cx| app_frame.read(cx).key().cloned()),
            Some(WindowKey::AppProperties(app.key))
        );

        cx.update_window(handle, |_, window, cx| {
            frame.update(cx, |f, cx| f.set_pinned(true, window, cx))
        })
        .expect("props window");
        cx.update_window(app_handle, |_, window, cx| {
            app_frame.update(cx, |f, cx| f.set_pinned(true, window, cx))
        })
        .expect("app props window");
        cx.run_until_parked();

        let keep_above: Vec<KWinCommand> = engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::KWin(k @ KWinCommand::KeepAbove { .. }) => Some(k),
                _ => None,
            })
            .collect();
        let pinned = |caption: &str| KWinCommand::KeepAbove {
            pid: std::process::id(),
            caption: Some(Arc::from(caption)),
            on: true,
        };
        assert_eq!(
            keep_above,
            [
                pinned("Process Properties - firefox - PID 4321"),
                pinned("App Properties - Firefox"),
            ]
        );
    }

    // ---- General copy, links, field menu and keyboard (E23-011) ----

    struct GeneralRig {
        engine: Arc<FakeEngine>,
        main: VisualTestContext,
        vcx: VisualTestContext,
        general: Entity<GeneralTab>,
    }

    /// Parent 1200 and the process 4321 with a General payload from `edit`.
    fn general_rig(cx: &mut TestAppContext, edit: impl FnOnce(&mut GeneralDetail)) -> GeneralRig {
        let (engine, main) = setup(cx);
        publish_records(
            cx,
            &engine,
            vec![
                rec(1200, ProcFlags::default()),
                rec(4321, ProcFlags::default()),
            ],
        );
        let handle = open(cx, 4321).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        let mut d = detail("firefox --x");
        edit(&mut d);
        publish_detail(cx, &engine, 4321, move |t| {
            t.tabs[DetailTab::General.index()].payload = Some(payload(d));
            t.tabs[DetailTab::General.index()].validity = Validity::Valid;
        });
        draw(&mut vcx);
        draw(&mut vcx);
        let general = cx
            .read(|cx| view.read(cx).tabs().general.clone())
            .expect("general tab");
        GeneralRig {
            engine,
            main,
            vcx,
            general,
        }
    }

    /// The row index of `label` in column `col`.
    fn general_row(rig: &mut GeneralRig, col: usize, label: &str) -> usize {
        rig.vcx
            .read(|cx| rig.general.read(cx).rows(col))
            .iter()
            .position(|r| r.label.as_ref() == label)
            .unwrap_or_else(|| panic!("{label} shown"))
    }

    /// A point just inside the value of row `row` in column `col` (rows in the first grid).
    fn general_value_point(
        rig: &mut GeneralRig,
        col: usize,
        row: usize,
    ) -> gpui_kit::Point<Pixels> {
        draw(&mut rig.vcx);
        let panel = rig
            .vcx
            .debug_bounds(
                [
                    "GeneralTab-panel-0",
                    "GeneralTab-panel-1",
                    "GeneralTab-panel-2",
                ][col],
            )
            .expect("panel drawn");
        let (sp3, sp4, row_h) = rig.vcx.update(|_, cx| {
            let m = tl_ui::theme::metrics(cx);
            (m.sp[3], m.sp[4], px(26. * m.ui_scale))
        });
        let label = rig
            .vcx
            .read(|cx| rig.general.read(cx).label_width(col))
            .expect("measured");
        gpui_kit::point(
            panel.left() + sp4 + label + px(3.),
            panel.top() + sp3 + row_h * (row as f32 + 0.5),
        )
    }

    fn general_click(rig: &mut GeneralRig, col: usize, row: usize) {
        let at = general_value_point(rig, col, row);
        rig.vcx.simulate_click(at, Default::default());
        rig.vcx.run_until_parked();
    }

    fn clipboard(vcx: &mut VisualTestContext) -> String {
        vcx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    }

    fn focus_fields(rig: &mut GeneralRig) {
        let focus = rig.vcx.read(|cx| rig.general.read(cx).grid_focus().clone());
        rig.vcx.update(|window, cx| focus.focus(window, cx));
        draw(&mut rig.vcx);
    }

    use tl_model::details::{AppGeneralDetail, AppKind};
    use tl_model::snapshot::{ServiceStatus, ServicesSnapshot, StartupType, UnitRecord};
    use tl_model::table::{Manager, SysNodeId, SysTable, UnitRef, UnitType};

    fn app_detail_for_unit() -> AppGeneralDetail {
        fn valid<T>(value: T) -> Total<T> {
            Total {
                value,
                validity: Validity::Valid,
            }
        }
        AppGeneralDetail {
            name: text("Firefox"),
            kind: AppKind::App,
            icon_path: None,
            app_id: Some(text("org.mozilla.firefox")),
            desktop_file: None,
            unit: None,
            cgroup: Some(text("/user.slice/app-firefox.scope")),
            processes: 34,
            memory_anon_bytes: valid(1_310_720_000),
            swap_bytes: valid(0),
            disk_read_bps: valid(0.0),
            disk_write_bps: valid(0.0),
            primary_cmdline: valid(text("/usr/lib/firefox/firefox")),
        }
    }

    /// The Unit row sits deep in column 0: make the window tall enough to click it.
    fn grow_tall(rig: &mut GeneralRig) {
        let width = rig.vcx.update(|window, _| window.viewport_size().width);
        rig.vcx.simulate_resize(gpui_kit::size(width, px(2400.)));
        draw(&mut rig.vcx);
    }

    fn firefox_unit() -> UnitRef {
        UnitRef {
            manager: Manager::User,
            name: "app-firefox@1.service".into(),
        }
    }

    /// Publishes the Firefox unit as the only service and collapses both Services groups, so
    /// the query a jump sends shows which manager group it expanded.
    fn publish_firefox_unit(cx: &mut TestAppContext, engine: &FakeEngine) {
        cx.update(|cx| {
            TlSettings::update(cx, |data| {
                data.state.services.group_expanded.system = false;
                data.state.services.group_expanded.user = false;
            });
        });
        let record = UnitRecord {
            unit: firefox_unit(),
            kind: UnitType::Service,
            description: Arc::from("Firefox"),
            status: ServiceStatus::Running,
            sub_state: Arc::from("running"),
            load_state: Arc::from("loaded"),
            startup: StartupType::Static,
            main_pid: 4321,
            user: Arc::from("user"),
            path: Arc::from("/usr/lib/firefox/firefox"),
            unit_file: None,
            active_since_us: None,
            triggered_by: Arc::from([]),
            control_group: None,
            can_reload: false,
            has_cgroup_procs: false,
            cgroup_pids: Arc::from([]),
            cpu_pct: Total::default(),
            memory_bytes: Total::default(),
            disk_bps: Total::default(),
            tasks: Total::default(),
        };
        engine.publish_with(|snapshot| {
            snapshot.services = Arc::new(ServicesSnapshot {
                units: vec![record].into(),
                ..ServicesSnapshot::default()
            });
        });
        cx.run_until_parked();
    }

    /// The expanded rows of the last Services query the page sent.
    fn services_expanded(engine: &FakeEngine) -> Vec<SysNodeId> {
        engine
            .commands()
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetSysTableQuery(query) if query.table == SysTable::Services => {
                    Some(query.expanded.to_vec())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    /// How many times the Services page found the unit and said so. The toast queue merges
    /// repeats into one toast that ends in "(×n)".
    fn switched_toasts(main: &mut VisualTestContext) -> usize {
        const TEXT: &str = "Switched to Services tab";
        main_toasts(main)
            .iter()
            .filter_map(|t| match t.strip_prefix(TEXT)? {
                "" => Some(1),
                rest => rest.strip_prefix(" (×")?.strip_suffix(')')?.parse().ok(),
            })
            .sum()
    }

    /// The main window's active tab. The main window now handles `GoToService` itself.
    fn main_tab(cx: &mut TestAppContext) -> Tab {
        cx.update(|cx| {
            main_window_view(cx)
                .expect("main window")
                .read(cx)
                .active_tab()
        })
    }

    fn main_is_active(cx: &mut TestAppContext, main: &mut VisualTestContext) -> bool {
        let active = cx.update(|cx| cx.active_window().map(|w| w.window_id()));
        let main_id = main.update(|window, _| window.window_handle().window_id());
        active == Some(main_id)
    }

    #[gpui_kit::test]
    fn general_unit_link_dispatches_go_to_service(cx: &mut TestAppContext) {
        let mut rig = general_rig(cx, |d| {
            d.unit = Total {
                value: Some(firefox_unit()),
                validity: Validity::Valid,
            };
        });
        publish_firefox_unit(cx, &rig.engine);
        grow_tall(&mut rig);
        let row = general_row(&mut rig, 0, "Unit:");
        let value = rig
            .vcx
            .read(|cx| rig.general.read(cx).rows(0)[row].value.clone());
        assert!(value.link.is_some(), "Unit is a link");
        assert_eq!(value.tooltip.as_deref(), Some("Go to service"));
        general_click(&mut rig, 0, row);
        assert_eq!(
            main_tab(cx),
            Tab::Services,
            "the main window shows Services"
        );
        assert_eq!(
            switched_toasts(&mut rig.main),
            1,
            "the page found User app-firefox@1.service: {:?}",
            main_toasts(&mut rig.main)
        );
        assert_eq!(
            services_expanded(&rig.engine),
            [SysNodeId::ServiceGroup(Manager::User)],
            "the jump expanded only the User group"
        );
        assert!(
            main_is_active(cx, &mut rig.main),
            "the main window is activated"
        );
        assert!(
            clipboard(&mut rig.vcx).is_empty(),
            "a link click copies nothing"
        );

        // Enter on the grid cursor runs the same link. Go back to Processes first, so the
        // check fails when Enter does nothing.
        rig.main.dispatch_action(tl_ui::actions::SelectTab(0));
        rig.main.run_until_parked();
        assert_eq!(main_tab(cx), Tab::Processes, "the tab was reset");
        focus_fields(&mut rig);
        rig.vcx
            .update(|_, cx| rig.general.update(cx, |g, cx| g.set_cursor(0, row, cx)));
        rig.vcx.simulate_keystrokes("enter");
        rig.vcx.run_until_parked();
        assert_eq!(main_tab(cx), Tab::Services);
        assert_eq!(
            switched_toasts(&mut rig.main),
            2,
            "Enter found the same unit: {:?}",
            main_toasts(&mut rig.main)
        );
    }

    #[gpui_kit::test]
    fn app_props_unit_link_dispatches_go_to_service(cx: &mut TestAppContext) {
        let (engine, mut main) = setup(cx);
        let app = app_group("Firefox", GroupRowKind::App, 34);
        publish_groups(cx, &engine, vec![app.clone()]);
        publish_firefox_unit(cx, &engine);
        let handle = open_app(cx, app.key).expect("app window opened");
        let general = app_props(cx, handle).read_with(cx, |w, _| w.general().clone());
        let mut detail = app_detail_for_unit();
        detail.unit = Some(firefox_unit());
        let p = Arc::new(TabPayload::AppGeneral(Arc::new(detail)));
        cx.update(|cx| general.update(cx, |t, cx| t.set_payload(Some(&p), cx)));
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        let row = vcx
            .read(|cx| general.read(cx).rows(0).to_vec())
            .iter()
            .position(|r| r.label.as_ref() == "Unit:")
            .expect("Unit row");
        let value = vcx.read(|cx| general.read(cx).rows(0)[row].value.clone());
        assert!(value.link.is_some(), "Unit is a link");
        assert_eq!(value.tooltip.as_deref(), Some("Go to service"));
        let focus = vcx.read(|cx| general.read(cx).grid_focus().clone());
        vcx.update(|window, cx| focus.focus(window, cx));
        vcx.update(|_, cx| general.update(cx, |g, cx| g.set_cursor(0, row, cx)));
        draw(&mut vcx);
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert_eq!(
            main_tab(cx),
            Tab::Services,
            "the main window shows Services"
        );
        assert!(
            main_is_active(cx, &mut main),
            "the main window is activated"
        );
        assert_eq!(
            switched_toasts(&mut main),
            1,
            "the page found User app-firefox@1.service: {:?}",
            main_toasts(&mut main)
        );
        assert_eq!(
            services_expanded(&engine),
            [SysNodeId::ServiceGroup(Manager::User)],
            "the jump expanded only the User group"
        );
    }

    // ---- File descriptors tab (E23-021, UI-PWN-012) ----

    use tl_model::details::{FdDetail, FdKind, FdRow};
    use tl_ui::components::menu::{MenuEntryKind, open_context_menu};
    use tl_ui::table::actions::OpenContextMenu;
    use tl_ui::windows::props::fds::FdsTab;

    fn fd(fd: i32, kind: FdKind, name: &str, flags: Option<u32>, pos: Option<u64>) -> FdRow {
        FdRow {
            fd,
            kind,
            name: text(name),
            unnamed: name.is_empty(),
            pos,
            flags,
            mnt_id: Some(25),
            inode: Some(1_048_577),
            eventfd_count: None,
            drm_client_id: None,
            drm_driver: None,
            socket_inode: None,
        }
    }

    fn fd_rows() -> Vec<FdRow> {
        vec![
            fd(
                3,
                FdKind::File,
                "/home/u/notes.txt",
                Some(0o2104002),
                Some(4096),
            ),
            fd(4, FdKind::Pipe, "pipe:[90211]", Some(0o4000), None),
            fd(7, FdKind::AnonInode, "", Some(0o2), None),
        ]
    }

    struct FdsRig {
        _main: VisualTestContext,
        vcx: VisualTestContext,
        tab: Entity<FdsTab>,
    }

    /// Opens Properties for `pid` on the File descriptors tab with `rows` as its payload.
    fn fds_window(
        cx: &mut TestAppContext,
        engine: &Arc<FakeEngine>,
        pid: u32,
        rows: Vec<FdRow>,
        validity: Validity,
    ) -> (
        AnyWindowHandle,
        Entity<PropsWindow>,
        VisualTestContext,
        Entity<FdsTab>,
    ) {
        let handle = open(cx, pid).expect("opened");
        let view = props(cx, handle);
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        let payload = Arc::new(TabPayload::Fds(Arc::new(FdDetail {
            rows: Arc::from(rows),
            fdinfo_available: true,
        })));
        publish_detail(cx, engine, pid, move |d| {
            let slot = &mut d.tabs[DetailTab::Fds.index()];
            slot.payload = Some(payload);
            slot.validity = validity;
        });
        vcx.update(|window, cx| view.update(cx, |p, cx| p.select_tab(DetailTab::Fds, window, cx)));
        draw(&mut vcx);
        let tab = cx
            .read(|cx| view.read(cx).tabs().fds.clone())
            .expect("fds tab");
        (handle, view, vcx, tab)
    }

    /// Selects `fds` with the first as the focus row and focuses the table.
    fn select_fds(rig: &mut FdsRig, fds: &[i32]) {
        let table = rig.vcx.read(|cx| rig.tab.read(cx).table().clone());
        let fds = fds.to_vec();
        rig.vcx.update(|window, cx| {
            table.update(cx, |t, cx| {
                for (i, fd) in fds.iter().enumerate() {
                    if i == 0 {
                        t.selection.click(*fd);
                    } else {
                        t.selection.ctrl_click(*fd);
                    }
                }
                cx.notify();
            });
            let focus = table.read(cx).focus_handle().clone();
            focus.focus(window, cx);
        });
        draw(&mut rig.vcx);
    }

    /// The keyboard row menu's entries as (label, enabled).
    fn fds_menu(rig: &mut FdsRig) -> Vec<(String, bool)> {
        rig.vcx.dispatch_action(OpenContextMenu);
        draw(&mut rig.vcx);
        rig.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("the row menu is open");
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| {
                    (
                        e.label.map(|l| l.to_string()).unwrap_or_default(),
                        e.kind == MenuEntryKind::Item,
                    )
                })
                .collect()
        })
    }

    fn socket_rows() -> Vec<FdRow> {
        let mut tcp = fd(9, FdKind::Tcp, "TCP 10.0.0.2:443", Some(0o2), None);
        tcp.socket_inode = Some(1_234_567);
        let mut fds = fd_rows();
        fds.push(tcp);
        fds
    }

    fn socket_rig(cx: &mut TestAppContext) -> FdsRig {
        let (engine, main) = setup(cx);
        publish_records(cx, &engine, vec![rec(4321, ProcFlags::default())]);
        let (_handle, _view, vcx, tab) =
            fds_window(cx, &engine, 4321, socket_rows(), Validity::Valid);
        FdsRig {
            _main: main,
            vcx,
            tab,
        }
    }

    #[gpui_kit::test]
    fn fds_go_to_connection_not_found_toast(cx: &mut TestAppContext) {
        let mut rig = socket_rig(cx);
        select_fds(&mut rig, &[9]);
        fds_menu(&mut rig);
        rig.vcx.simulate_keystrokes("down down enter");
        draw(&mut rig.vcx);
        rig.vcx.run_until_parked();
        assert_eq!(
            main_toasts(&mut rig._main),
            ["Connection not found in current view"]
        );
    }

    // ---- Environment tab (E23-022, UI-PWN-016, R-099) ----

    use tl_ui::settings_global::TlSettings;

    // ---- Strings tab (E23-021, UI-PWN-017) ----

    // ---- Grant access and Enable (E23-025, UI-PWN-018 P3) ----
}

// ---- Threads tab (E23-016, UI-PWN-010) ----
