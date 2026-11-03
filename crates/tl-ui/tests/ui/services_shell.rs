//! Services page tests that need `MainWindow`: lazy page creation, the toolbar slots, the status
//! chip, the filter box, tab switches from the page, and the keymap at app init. The other
//! Services tests moved to tl-ui-sys (tasklens-zljr5.43).

use std::sync::Arc;

use gpui_kit::base::Root;

use gpui_kit::{
    Action, AppContext as _, Context, Entity, IntoElement, KeyContext, Render, TestAppContext,
    VisualTestContext, Window, div,
};

use tl_model::client::EngineClient;

use tl_model::command::Command;

use tl_model::ids::ProcessKey;

use tl_model::table::{Manager, UnitRef, UnitType};

use tl_model::time::BootId;

use tl_ui_sys::pages::services::actions::*;

use tl_ui_settings::settings::UiState;

use tl_ui_testkit::fake_engine::FakeEngine;

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn assert_window_binding(
    vcx: &mut VisualTestContext,
    action: &dyn Action,
    context: &str,
    keys: &str,
) {
    let parsed = KeyContext::parse(context).expect("key context");
    let bindings = vcx.update(|window, _| window.bindings_for_action_in_context(action, parsed));
    assert!(
        bindings.iter().any(|binding| {
            binding.keystrokes().len() == 1 && binding.keystrokes()[0].unparse() == keys
        }),
        "{keys} in {context:?} is not bound to {action:?}; got {bindings:?}"
    );
}

#[gpui_kit::test]
fn init_binds_services_keys(cx: &mut TestAppContext) {
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui_settings::settings::Settings;

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
    cx.update(|cx| TasklensUi::init(cx, client, init));
    cx.run_until_parked();

    let handle = cx.add_window(|window, cx| Root::new(cx.new(|_| Host), window, cx));
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    assert_window_binding(&mut vcx, &ServiceStart, "Table ServiceTable", "ctrl-s");
    assert_window_binding(
        &mut vcx,
        &ServicePropsSelectTab { index: 2 },
        "ServiceProps",
        "ctrl-3",
    );
}

/// Services Start, Stop, Restart, Reload and Reset failed flows (tasklens-hk6.15).
mod flows {
    use std::sync::Arc;

    use gpui_kit::{TestAppContext, VisualTestContext};

    use tl_model::command::Tab;

    use tl_model::ids::NodeId;

    use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};

    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, StartupType, UnitRecord};

    use tl_model::table::{Manager, UnitRef};

    use tl_model::table::{ProcessTableView, TableRow, Total};

    use tl_model::validity::{FieldValidity, Validity};

    use tl_ui::app::{AppEnv, TasklensUi, UiInit};

    use tl_ui_sys::pages::services::flows::run_jump_to_process;

    use tl_ui::window::main::{main_window_view, open_main_window_on};

    use tl_ui_settings::settings::Settings;

    use super::*;

    fn unit(name: &str) -> UnitRef {
        UnitRef {
            manager: Manager::System,
            name: Arc::from(name),
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        main: VisualTestContext,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
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
            tl_ui_base::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Services, cx))
            .expect("the main window opens");
        let mut main = VisualTestContext::from_window(handle.into(), cx);
        main.update(|window, _| window.activate_window());
        main.run_until_parked();
        Rig { engine, main }
    }

    impl Rig {
        fn toasts(&mut self) -> Vec<String> {
            self.main.update(|_, cx| {
                let view = main_window_view(cx).expect("main window");
                let texts = view.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(|t| t.to_string()).collect()
            })
        }

        fn draw(&mut self) {
            self.main.run_until_parked();
            self.main.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn active_tab(&mut self) -> Tab {
            self.main.update(|_, cx| {
                main_window_view(cx)
                    .expect("main window")
                    .read(cx)
                    .active_tab()
            })
        }

        fn queries(&self) -> usize {
            self.engine
                .commands()
                .iter()
                .filter(|c| matches!(c, Command::SetTableQuery(_)))
                .count()
        }
    }

    fn unit_rec(name: &str, path: &str, unit_file: Option<&str>, pid: u32) -> UnitRecord {
        fn missing<T: Default>() -> Total<T> {
            Total {
                value: T::default(),
                validity: Validity::Missing,
            }
        }
        UnitRecord {
            unit: unit(name),
            kind: UnitType::Service,
            description: Arc::from("Unit"),
            status: tl_model::snapshot::ServiceStatus::Running,
            sub_state: Arc::from("running"),
            load_state: Arc::from("loaded"),
            startup: StartupType::Enabled,
            main_pid: pid,
            user: Arc::from("root"),
            path: Arc::from(path),
            unit_file: unit_file.map(Arc::from),
            active_since_us: None,
            triggered_by: Arc::from([]),
            control_group: None,
            can_reload: false,
            has_cgroup_procs: false,
            cgroup_pids: Arc::from([]),
            cpu_pct: missing(),
            memory_bytes: missing(),
            disk_bps: missing(),
            tasks: missing(),
        }
    }

    #[gpui_kit::test]
    fn jump_to_process_main_pid_zero_toast(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let u = unit_rec("a.service", "/usr/bin/a", None, 0);
        let snap = (*rig.engine.latest()).clone();
        let tab = rig.active_tab();
        rig.main
            .update(|window, cx| run_jump_to_process(&u, &snap, window, cx));
        rig.main.run_until_parked();
        assert_eq!(rig.toasts(), ["Service not running"]);
        assert_eq!(rig.active_tab(), tab, "the tab does not change");
    }

    fn proc_record(pid: u32) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessRecord {
            key: ProcessKey {
                start_ticks: u64::from(pid),
                boot_id: tl_model::time::BootId(0),
                pid,
            },
            ppid: 1,
            uid: 0,
            meta: Arc::new(ProcessMeta {
                comm: a("daemon"),
                name: a("daemon"),
                exe: None,
                cmdline: None,
                user: a("root"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a("daemon"),
            }),
            group: GroupKind::System.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
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
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    #[gpui_kit::test]
    fn jump_to_process_calls_entry_point(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let record = proc_record(812);
        let key = record.key;
        rig.engine.publish_with(|s| {
            let mut processes = ProcessSnapshot {
                records: vec![record].into(),
                ..ProcessSnapshot::default()
            };
            processes.header.version = s.sequence;
            s.processes = Arc::new(processes);
        });
        rig.draw();
        let u = unit_rec("a.service", "/usr/bin/a", None, 812);
        let snap = (*rig.engine.latest()).clone();
        let before = rig.queries();
        rig.main
            .update(|window, cx| run_jump_to_process(&u, &snap, window, cx));
        rig.main.run_until_parked();
        assert_eq!(rig.active_tab(), Tab::Processes);
        // The window starts on Services, so the jump is the first visit to Processes: that page
        // sends its first query (generation 1), then the jump sends one more (generation 2).
        assert_eq!(
            rig.queries(),
            before + 2,
            "the first visit and the jump each sent a query"
        );
        let generation = rig
            .engine
            .commands()
            .iter()
            .rev()
            .find_map(|c| match c {
                Command::SetTableQuery(q) => Some(q.generation),
                _ => None,
            })
            .expect("query");
        rig.engine.publish_with(|s| {
            let header = ComponentHeader {
                version: s.sequence,
                ..ComponentHeader::default()
            };
            s.table = Arc::new(ProcessTableView {
                header,
                query_generation: generation,
                rows: vec![TableRow {
                    node: NodeId::Process(key),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: Some(0),
                    group: None,
                    descendants: 0,
                    subtree: None,
                }]
                .into(),
                ..ProcessTableView::default()
            });
        });
        rig.draw();
        assert_eq!(rig.toasts(), ["Switched to service process."]);
    }
}

use tl_model::snapshot::{ServiceStatus, StartupType, UnitRecord};

use tl_model::table::Total;

use tl_model::validity::Validity;

fn record(name: &str, kind: UnitType, status: ServiceStatus) -> UnitRecord {
    UnitRecord {
        unit: UnitRef {
            manager: Manager::System,
            name: Arc::from(name),
        },
        kind,
        description: Arc::from("Secure Shell"),
        status,
        sub_state: Arc::from("running"),
        load_state: Arc::from("loaded"),
        startup: StartupType::Enabled,
        main_pid: 0,
        user: Arc::from("root"),
        path: Arc::from("/usr/bin/sshd"),
        unit_file: Some(Arc::from("/usr/lib/systemd/system/sshd.service")),
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
    }
}

use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};

use tl_model::validity::FieldValidity;

fn child_key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid),
        boot_id: BootId(1),
        pid,
    }
}

fn child_process(pid: u32, flags: ProcFlags) -> ProcessRecord {
    let text = |value: &str| Arc::<str>::from(value);
    ProcessRecord {
        key: child_key(pid),
        ppid: 1,
        uid: 0,
        meta: Arc::new(ProcessMeta {
            comm: text("sshd"),
            name: text("sshd"),
            exe: Some(text("/usr/bin/sshd")),
            cmdline: None,
            user: text("root"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: Some(text("OpenSSH daemon")),
            desktop_path: None,
            search_lower: text("sshd"),
        }),
        group: GroupKind::System.key(),
        state: ProcState::Sleeping,
        flags,
        nice: 0,
        threads: 3,
        cpu_pct: 0.5,
        rss_anon_bytes: 5_452_595,
        rss_bytes: 9_000_000,
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

/// The Services page (tasklens-hk6.2, UI-SYS-010, UI-SYS-022, UI-SYS-023, UI-SYS-004).
mod page {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Context, Entity, TestAppContext, VisualTestContext, Window};

    use tl_model::client::EngineClient;

    use tl_model::command::{Command, Tab};

    use tl_model::health::ManagerState;

    use tl_model::ids::ProcessKey;

    use tl_model::process::{ProcFlags, ProcessRecord};

    use tl_model::snapshot::{
        ComponentHeader, ProcessSnapshot, ServiceCounts, ServiceStatus, ServicesSnapshot,
        UnitRecord,
    };

    use tl_model::table::{Manager, SysNodeId, SysRow, SysTable, SysTableQuery, UnitType};

    use tl_model::validity::Validity;

    use tl_ui::app::{AppEnv, TasklensUi, UiInit};

    use tl_ui_base::host::HostContext;

    use tl_ui_sys::pages::services::actions::{
        GoToService, JumpOrigin, ManagerArg, ServicesClearAllFilters, ServicesShowHidden,
        ServicesToggleStateFilter, StateFilterArg,
    };

    use tl_ui_sys::pages::services::ServicesPage;

    use tl_ui_table::table::sys_table::SysBody;

    use tl_ui_table::table::tree_table::TreeTableDelegate as _;

    use tl_ui::window::filter::FILTER_DEBOUNCE;

    use tl_ui::window::main::{
        MainWindow, PageSlot, main_window_view, open_main_window_on, set_test_page,
    };

    use tl_ui_settings::settings::{Settings, UiState};

    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::{child_key, child_process, record};

    /// System units in the fixture; the other five are User units.
    const SYSTEM_UNITS: usize = 15;

    /// The fake engine's 20-unit services fixture: 15 System units, then 5 User units.
    fn fixture() -> Vec<UnitRecord> {
        (0..20)
            .map(|i| {
                let status = if i % 4 == 3 {
                    ServiceStatus::Stopped
                } else {
                    ServiceStatus::Running
                };
                let mut unit = record(&format!("unit{i:02}.service"), UnitType::Service, status);
                if i >= SYSTEM_UNITS {
                    unit.unit.manager = Manager::User;
                }
                unit
            })
            .collect()
    }

    /// Group rows and unit rows for `units`; `children` adds child rows under unit indexes. No
    /// units means no rows, as the engine sends when nothing matches the filter.
    fn rows(units: &[UnitRecord], children: &[(usize, ProcessKey)]) -> Vec<SysRow> {
        let mut rows = Vec::new();
        if units.is_empty() {
            return rows;
        }
        for manager in [Manager::System, Manager::User] {
            rows.push(SysRow {
                node: SysNodeId::ServiceGroup(manager),
                depth: 0,
                expanded: true,
                has_children: true,
                record: None,
            });
            for (i, unit) in units.iter().enumerate() {
                if unit.unit.manager != manager {
                    continue;
                }
                let kids: Vec<ProcessKey> = children
                    .iter()
                    .filter(|(ix, _)| *ix == i)
                    .map(|(_, key)| *key)
                    .collect();
                rows.push(SysRow {
                    node: SysNodeId::Unit(unit.unit.clone()),
                    depth: 1,
                    expanded: !kids.is_empty(),
                    has_children: !kids.is_empty(),
                    record: Some(i as u32),
                });
                for key in kids {
                    rows.push(SysRow {
                        node: SysNodeId::UnitProcess(unit.unit.clone(), key),
                        depth: 2,
                        expanded: false,
                        has_children: false,
                        record: Some(0),
                    });
                }
            }
        }
        rows
    }

    fn init(cx: &mut TestAppContext, client: Arc<dyn EngineClient>) {
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
            cx.set_reduce_motion(true);
        });
    }

    fn last_services_query(engine: &FakeEngine) -> Option<SysTableQuery> {
        engine
            .commands()
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetSysTableQuery(query) if query.table == SysTable::Services => {
                    Some(query)
                }
                _ => None,
            })
    }

    /// Publishes a services view for the last query the page sent.
    fn publish_services(
        engine: &FakeEngine,
        version: u64,
        units: Vec<UnitRecord>,
        children: &[(usize, ProcessKey)],
        mut records: Vec<ProcessRecord>,
        counts: ServiceCounts,
    ) {
        let generation = last_services_query(engine).map_or(0, |q| q.generation);
        let rows = rows(&units, children);
        records.sort_by_key(|r| r.key);
        let system = units
            .iter()
            .filter(|u| u.unit.manager == Manager::System)
            .count() as u32;
        let user = units.len() as u32 - system;
        engine.publish_with(|snapshot| {
            snapshot.services = Arc::new(ServicesSnapshot {
                header: ComponentHeader {
                    version,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                rows: rows.into(),
                query_generation: generation,
                units: units.into(),
                counts,
                managers: [ManagerState::Connected; 2],
                manager_counts: [system, user],
                ..ServicesSnapshot::default()
            });
            snapshot.processes = Arc::new(ProcessSnapshot {
                records: records.into(),
                ..ProcessSnapshot::default()
            });
        });
    }

    fn services_page(window: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let page = cx.new(|cx| ServicesPage::new(&HostContext::local(cx), window, cx));
        PageSlot::new(page, cx)
    }

    /// A main window on Processes with a Services page and a published sshd.service.
    fn go_to_service_rig(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
        let engine = FakeEngine::new();
        init(cx, engine.clone());
        let handle = cx.update(|cx| {
            set_test_page(cx, Tab::Services, services_page);
            open_main_window_on(Tab::Processes, cx).expect("main window opens")
        });
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let units = units_with_sshd();
        publish_view(&engine, 1, &units, units.len(), 0);
        vcx.run_until_parked();
        (engine, vcx)
    }

    fn dispatch_go_to_sshd(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            window.dispatch_action(
                Box::new(GoToService {
                    manager: ManagerArg::System,
                    name: "sshd.service".into(),
                    origin: JumpOrigin::Processes,
                }),
                cx,
            );
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn active_tab(vcx: &mut VisualTestContext) -> Tab {
        vcx.update(|_, cx| {
            main_window_view(cx)
                .expect("main window")
                .read(cx)
                .active_tab()
        })
    }

    #[gpui_kit::test]
    fn go_to_service_action_switches_tab_and_selects(cx: &mut TestAppContext) {
        let (engine, mut vcx) = go_to_service_rig(cx);
        assert_eq!(active_tab(&mut vcx), Tab::Processes);

        dispatch_go_to_sshd(&mut vcx);

        assert_eq!(active_tab(&mut vcx), Tab::Services);
        let toasts: Vec<String> = vcx.update(|_, cx| {
            let view = main_window_view(cx).expect("main window");
            let texts = view.read(cx).toasts().read(cx).visible_texts();
            texts.iter().map(|t| t.to_string()).collect()
        });
        assert_eq!(
            toasts
                .iter()
                .filter(|t| t.as_str() == "Switched to Services tab")
                .count(),
            1,
            "select_unit ran once: {toasts:?}"
        );
        let query = last_services_query(&engine).expect("a query");
        assert!(
            query
                .expanded
                .contains(&SysNodeId::ServiceGroup(Manager::System)),
            "System group expanded for the jump"
        );
    }

    #[gpui_kit::test]
    fn go_to_service_activates_main_window(cx: &mut TestAppContext) {
        let (_engine, mut vcx) = go_to_service_rig(cx);
        vcx.deactivate_window();
        assert!(!vcx.update(|window, _| window.is_window_active()));

        dispatch_go_to_sshd(&mut vcx);

        assert!(vcx.update(|window, _| window.is_window_active()));
        assert_eq!(active_tab(&mut vcx), Tab::Services);
    }

    #[gpui_kit::test]
    fn empty_state_filter_clear_resets_strip(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        init(cx, engine.clone());
        let handle = cx.update(|cx| {
            set_test_page(cx, Tab::Services, services_page);
            open_main_window_on(Tab::Services, cx).expect("main window opens")
        });
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let page = vcx.update(|_, cx| {
            main_window_view(cx)
                .expect("main window view")
                .read(cx)
                .page_entity::<ServicesPage>(Tab::Services)
                .expect("services page")
        });
        let focus = vcx.update(|_, cx| page.read(cx).table().read(cx).focus_handle().clone());
        let draw = |vcx: &mut VisualTestContext| {
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
        };

        // Text "zzz" through type-to-filter, then Failed on.
        vcx.update(|window, cx| window.focus(&focus, cx));
        vcx.simulate_keystrokes("z z z");
        vcx.executor().advance_clock(FILTER_DEBOUNCE);
        draw(&mut vcx);
        assert_eq!(
            vcx.read_entity(&page, |p, _| p.filter_text().to_owned()),
            "zzz"
        );
        vcx.update(|window, cx| {
            window.focus(&focus, cx);
            window.dispatch_action(
                Box::new(ServicesToggleStateFilter {
                    state: StateFilterArg::Failed,
                }),
                cx,
            );
        });
        draw(&mut vcx);
        assert!(!vcx.read_entity(&page, |p, _| p.strip().is_default()));
        publish_services(
            &engine,
            1,
            Vec::new(),
            &[],
            Vec::new(),
            ServiceCounts::default(),
        );
        draw(&mut vcx);
        assert_eq!(vcx.read_entity(&page, |p, _| p.body()), SysBody::NoMatches);

        vcx.update(|window, cx| {
            window.focus(&focus, cx);
            window.dispatch_action(Box::new(ServicesClearAllFilters), cx);
        });
        vcx.executor().advance_clock(FILTER_DEBOUNCE);
        draw(&mut vcx);
        assert_eq!(
            vcx.read_entity(&page, |p, _| p.filter_text().to_owned()),
            ""
        );
        assert!(vcx.read_entity(&page, |p, _| p.strip().is_default()));
        let query = last_services_query(&engine).expect("a query");
        assert!(query.filter.is_empty());
        assert!(query.services.expect("a strip filter").states.is_empty());
    }

    /// Publishes `units` as the unfiltered records and group and unit rows for the first `shown`
    /// of them, stamped with `generation`.
    fn publish_view(
        engine: &FakeEngine,
        version: u64,
        units: &[UnitRecord],
        shown: usize,
        generation: u64,
    ) {
        let rows = rows(&units[..shown], &[]);
        engine.publish_with(|snapshot| {
            snapshot.services = Arc::new(ServicesSnapshot {
                header: ComponentHeader {
                    version,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                rows: rows.into(),
                query_generation: generation,
                units: units.to_vec().into(),
                counts: ServiceCounts::default(),
                managers: [ManagerState::Connected; 2],
                manager_counts: [units.len() as u32, 0],
                ..ServicesSnapshot::default()
            });
        });
    }

    fn sshd() -> UnitRecord {
        record("sshd.service", UnitType::Service, ServiceStatus::Running)
    }

    /// The fixture plus sshd.service, last, so a view can leave it out.
    fn units_with_sshd() -> Vec<UnitRecord> {
        let mut units = fixture();
        units.push(sshd());
        units
    }

    fn go_to(
        vcx: &mut VisualTestContext,
        page: &Entity<ServicesPage>,
        unit: tl_model::table::UnitRef,
    ) {
        vcx.update(|window, cx| {
            page.update(cx, |p, cx| {
                p.select_unit(unit, JumpOrigin::Processes, window, cx)
            });
        });
        vcx.run_until_parked();
    }

    fn selected_ids(vcx: &mut VisualTestContext, page: &Entity<ServicesPage>) -> Vec<u64> {
        let table = vcx.read_entity(page, |p, _| p.table().clone());
        vcx.read_entity(&table, |t, _| t.selection.ids().collect())
    }

    #[gpui_kit::test]
    fn go_to_service_filtered_shows_button(cx: &mut TestAppContext) {
        let engine = FakeEngine::new();
        init(cx, engine.clone());
        let handle = cx.update(|cx| {
            set_test_page(cx, Tab::Services, services_page);
            open_main_window_on(Tab::Services, cx).expect("main window opens")
        });
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let page = vcx.update(|_, cx| {
            main_window_view(cx)
                .expect("main window view")
                .read(cx)
                .page_entity::<ServicesPage>(Tab::Services)
                .expect("services page")
        });
        let focus = vcx.update(|_, cx| page.read(cx).table().read(cx).focus_handle().clone());
        let draw = |vcx: &mut VisualTestContext| {
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
        };
        let toasts = |vcx: &mut VisualTestContext| -> Vec<String> {
            vcx.update(|_, cx| {
                let view = main_window_view(cx).expect("main window");
                let texts = view.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(|t| t.to_string()).collect()
            })
        };
        let generation =
            |engine: &FakeEngine| last_services_query(engine).map_or(0, |q| q.generation);

        vcx.update(|window, cx| window.focus(&focus, cx));
        vcx.simulate_keystrokes("n g i n x");
        vcx.executor().advance_clock(FILTER_DEBOUNCE);
        draw(&mut vcx);
        let units = units_with_sshd();
        let others = units.len() - 1;
        publish_view(&engine, 1, &units, others, generation(&engine));
        draw(&mut vcx);

        go_to(&mut vcx, &page, sshd().unit);
        publish_view(&engine, 2, &units, others, generation(&engine));
        draw(&mut vcx);
        assert_eq!(
            toasts(&mut vcx),
            [
                "Switched to Services tab",
                "Service not found in current view"
            ]
        );
        assert_eq!(
            vcx.read_entity(&page, |p, _| p.filter_text().to_owned()),
            "nginx"
        );

        vcx.update(|window, cx| window.dispatch_action(Box::new(ServicesShowHidden), cx));
        vcx.executor().advance_clock(FILTER_DEBOUNCE);
        draw(&mut vcx);
        assert_eq!(
            vcx.read_entity(&page, |p, _| p.filter_text().to_owned()),
            ""
        );
        publish_view(&engine, 3, &units, units.len(), generation(&engine));
        draw(&mut vcx);
        let id = tl_ui_sys::pages::services::delegate::row_key(&SysNodeId::Unit(sshd().unit));
        assert_eq!(selected_ids(&mut vcx, &page), [id]);
    }

    /// Child process rows run the process row commands through the shared entry point
    /// (tasklens-pdu.58): the requests, confirmations and toasts match the Processes tab's.
    mod child_actions {
        use tl_ui_procs::pages::processes::actions::GoToParent;

        use super::*;

        #[gpui_kit::test]
        fn child_go_to_parent_switches_to_processes_tab(cx: &mut TestAppContext) {
            let engine = FakeEngine::new();
            init(cx, engine.clone());
            let handle = cx.update(|cx| {
                set_test_page(cx, Tab::Services, services_page);
                open_main_window_on(Tab::Services, cx).expect("main window opens")
            });
            let mut vcx = VisualTestContext::from_window(handle.into(), cx);
            vcx.update(|window, _| window.activate_window());
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            let page = vcx.update(|_, cx| {
                main_window_view(cx)
                    .expect("main window view")
                    .read(cx)
                    .page_entity::<ServicesPage>(Tab::Services)
                    .expect("services page")
            });
            let mut systemd = child_process(1, ProcFlags::PROTECTED);
            systemd.ppid = 0;
            publish_services(
                &engine,
                1,
                fixture(),
                &[(0, child_key(812))],
                vec![systemd, child_process(812, ProcFlags::empty())],
                ServiceCounts::default(),
            );
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            let table = vcx.read_entity(&page, |p, _| p.table().clone());
            let id = vcx.read_entity(&table, |t, _| t.delegate.row_id(2));
            table.update(&mut vcx, |table, cx| {
                table.selection.click(id);
                cx.notify();
            });
            vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                window.dispatch_action(Box::new(GoToParent), cx);
            });
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(active_tab(&mut vcx), Tab::Processes);
        }
    }
}

/// The shell builds `ServicesPage` on its first visit and shows its toolbar and status text
/// (tasklens-hk6.20).
mod main_window {
    use super::*;

    use tl_model::command::Tab;

    use tl_ui::app::{AppEnv, TasklensUi, UiInit};

    use tl_ui_sys::pages::services::ServicesPage;

    use tl_ui::window::main::{MainWindow, main_window_view, open_main_window};

    use tl_ui_settings::settings::{Settings, StartTab};

    fn boot(cx: &mut TestAppContext) -> (VisualTestContext, Entity<MainWindow>) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let mut settings = Settings::default();
        settings.general.start_tab = StartTab::Processes;
        let init = UiInit {
            settings,
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: Vec::new(),
            env: AppEnv::test(),
        };
        cx.update(|cx| TasklensUi::init(cx, client, init));
        cx.run_until_parked();
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        (vcx, main)
    }

    fn select(vcx: &mut VisualTestContext, main: &Entity<MainWindow>, tab: Tab) {
        main.update_in(vcx, |m, window, cx| m.select_tab(tab, window, cx));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn toolbar_shape(vcx: &mut VisualTestContext) -> (f32, bool, bool) {
        let width = vcx
            .debug_bounds("Toolbar")
            .map(|b| f32::from(b.size.width))
            .expect("the toolbar draws");
        (
            width,
            vcx.debug_bounds("TlShield").is_some(),
            vcx.debug_bounds("TlButton-picker").is_some(),
        )
    }

    #[gpui_kit::test]
    fn main_window_creates_services_page_lazily(cx: &mut TestAppContext) {
        let (mut vcx, main) = boot(cx);
        let page = |vcx: &mut VisualTestContext| {
            vcx.read_entity(&main, |m, _| m.page_entity::<ServicesPage>(Tab::Services))
        };
        assert!(page(&mut vcx).is_none(), "no page before the first visit");
        select(&mut vcx, &main, Tab::Services);
        let first = page(&mut vcx).expect("the Services tab builds a page");
        assert_eq!(vcx.read_entity(&main, |m, _| m.active_tab()), Tab::Services);
        select(&mut vcx, &main, Tab::Processes);
        select(&mut vcx, &main, Tab::Services);
        let again = page(&mut vcx).expect("the page is kept");
        assert_eq!(first.entity_id(), again.entity_id());
    }

    #[gpui_kit::test]
    fn services_toolbar_slots(cx: &mut TestAppContext) {
        let (mut vcx, main) = boot(cx);
        select(&mut vcx, &main, Tab::Processes);
        let processes = toolbar_shape(&mut vcx);
        select(&mut vcx, &main, Tab::Information);
        let information = toolbar_shape(&mut vcx);
        select(&mut vcx, &main, Tab::Services);
        let services = toolbar_shape(&mut vcx);
        assert_eq!(
            services, information,
            "the same slots 1 and 4 as the other default tabs"
        );
        assert!(
            services.0 < processes.0,
            "no Processes page buttons on Services"
        );
    }

    #[gpui_kit::test]
    fn services_status_text_in_chip(cx: &mut TestAppContext) {
        let (mut vcx, main) = boot(cx);
        select(&mut vcx, &main, Tab::Services);
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ServicesPage>(Tab::Services))
            .expect("Services built");
        let mut expected = String::new();
        vcx.read_entity(&page, |p, _| p.status_text(&mut expected));
        assert!(!expected.is_empty(), "the loading text shows at first");
        let chip = vcx.read_entity(&main, |m, _| m.status_chip().clone());
        let drawn = vcx.read_entity(&chip, |c, _| c.drawn_text());
        assert_eq!(drawn, expected);
    }
}
