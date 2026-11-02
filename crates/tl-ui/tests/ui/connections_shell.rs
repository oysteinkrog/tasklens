//! Connections page tests that need `MainWindow`: the jump into the Processes tab, the tab strip,
//! the status chip and the keymap at app init. The other Connections tests moved to tl-ui-sys
//! (tasklens-zljr5.43).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use gpui_kit::{TestAppContext, px};
use smallvec::SmallVec;
use tl_model::actions::{Family, FourTuple, Proto};
use tl_model::ids::ProcessKey;
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::snapshot::{
    ComponentHeader, ConnRecord, ConnState, ConnectionsSnapshot, ProcessSnapshot, RemoteHost,
    TcpInfoView,
};
use tl_model::table::{SysNodeId, SysRow};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid),
        boot_id: BootId(1),
        pid,
    }
}

fn process(pid: u32, name: &str, user: &str, uid: u32) -> ProcessRecord {
    let text = |value: &str| Arc::<str>::from(value);
    ProcessRecord {
        key: key(pid),
        ppid: 1,
        uid,
        meta: Arc::new(ProcessMeta {
            comm: text(name),
            name: text(name),
            exe: None,
            cmdline: None,
            user: text(user),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: text(name),
        }),
        group: GroupKind::Background.key(),
        state: ProcState::Sleeping,
        flags: ProcFlags::empty(),
        nice: 0,
        threads: 1,
        cpu_pct: 0.,
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

fn connection(cookie: u64, owners: SmallVec<[ProcessKey; 1]>) -> ConnRecord {
    ConnRecord {
        cookie,
        proto: Proto::Tcp,
        family: Family::Inet,
        four_tuple: FourTuple {
            local: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
            remote: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 443),
        },
        local: Arc::from("127.0.0.1:8080"),
        foreign: Arc::from("10.0.0.1:443"),
        state: ConnState::Established,
        owners,
        owner_validity: Validity::Valid,
        uid: 1000,
        inode: 1234567,
        ifindex: 0,
        interface: None,
        recv_q: 123456,
        send_q: 0,
        unit: None,
        tcp: None,
        remote_host: RemoteHost::NotRequested,
        netns: None,
    }
}

/// `TasklensUi::init` binds the Connections keys and the shell builds `ConnectionsPage` for its
/// tab (tasklens-9o0.20).
#[gpui_kit::test]
fn connections_page_registered(cx: &mut TestAppContext) {
    use gpui_kit::{AppContext as _, VisualTestContext};
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::pages::connections::ConnectionsPage;
    use tl_ui::window::main::{main_window_view, open_main_window};
    use tl_ui_settings::settings::{Settings, StartTab, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let mut settings = Settings::default();
    settings.general.start_tab = StartTab::Connections;
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
    let page = vcx.read_entity(&main, |m, _| {
        m.page_entity::<ConnectionsPage>(Tab::Connections)
    });
    assert!(
        page.is_some(),
        "the Connections tab builds a ConnectionsPage"
    );
}

/// Jump to process and Go to service from the Connections tab (tasklens-9o0.18).
mod jump {

    use super::*;
    use gpui_kit::{AppContext as _, VisualTestContext};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::ids::NodeId;
    use tl_model::snapshot::{
        ConnCounts, ServiceStatus, ServicesSnapshot, StartupType, UnitRecord,
    };
    use tl_model::table::{
        Manager, ProcessTableView, SysTable, TableRow, Total, UnitRef, UnitType,
    };
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::pages::connections::ConnectionsPage;
    use tl_ui::pages::connections::actions::{
        ConnectionGoToService, ConnectionJumpToProcess, GoToConnection,
    };
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui_settings::settings::{Settings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: gpui_kit::Entity<ConnectionsPage>,
    }

    fn unit_record(unit: UnitRef) -> UnitRecord {
        UnitRecord {
            unit,
            kind: UnitType::Service,
            description: Arc::from("OpenSSH Daemon"),
            status: ServiceStatus::Running,
            sub_state: Arc::from("running"),
            load_state: Arc::from("loaded"),
            startup: StartupType::Enabled,
            main_pid: 4321,
            user: Arc::from("root"),
            path: Arc::from("/usr/bin/sshd"),
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
        }
    }

    fn rig(cx: &mut TestAppContext, record: ConnRecord, processes: Vec<ProcessRecord>) -> Rig {
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
            .update(|cx| open_main_window_on(Tab::Connections, cx))
            .expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let page = vcx.update(|_, cx| {
            main_window_view(cx)
                .expect("main window")
                .read(cx)
                .page_entity::<ConnectionsPage>(Tab::Connections)
                .expect("Connections page")
        });
        let mut rig = Rig { engine, vcx, page };
        rig.publish(record, processes);
        rig
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn publish(&mut self, record: ConnRecord, mut processes: Vec<ProcessRecord>) {
            let generation = self
                .engine
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetSysTableQuery(query) => Some(query.generation),
                    _ => None,
                })
                .unwrap_or(0);
            processes.sort_unstable_by_key(|process| process.key);
            let cookie = record.cookie;
            self.engine.publish_with(|snapshot| {
                snapshot.connections = Arc::new(ConnectionsSnapshot {
                    header: ComponentHeader {
                        version: 1,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    rows: vec![SysRow {
                        node: SysNodeId::Conn(cookie),
                        depth: 0,
                        expanded: false,
                        has_children: false,
                        record: Some(0),
                    }]
                    .into(),
                    query_generation: generation,
                    records: vec![record].into(),
                    counts: ConnCounts {
                        total: 1,
                        tcp: 1,
                        ..ConnCounts::default()
                    },
                    ..ConnectionsSnapshot::default()
                });
                snapshot.processes = Arc::new(ProcessSnapshot {
                    records: processes.into(),
                    ..ProcessSnapshot::default()
                });
            });
            self.draw();
        }

        /// Selects the only row, focuses the table and dispatches `action`.
        fn dispatch(&mut self, select: bool, action: impl gpui_kit::Action) {
            let table = self.vcx.read_entity(&self.page, |p, _| p.table().clone());
            if select {
                let cookie = 1;
                table.update(&mut self.vcx, |table, cx| {
                    table.selection.click(cookie);
                    cx.notify();
                });
            }
            self.vcx
                .update(|window, cx| table.read(cx).focus_handle().clone().focus(window, cx));
            self.draw();
            self.vcx.dispatch_action(action);
            self.vcx.run_until_parked();
        }

        fn toasts(&mut self) -> Vec<String> {
            self.vcx.update(|_, cx| {
                let view = main_window_view(cx).expect("main window");
                let texts = view.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(ToString::to_string).collect()
            })
        }

        /// Publishes `unit` as the only service and collapses both Services groups, so the
        /// query the jump sends shows which manager group it expanded.
        fn publish_unit(&mut self, unit: UnitRef) {
            self.vcx.update(|_, cx| {
                TlSettings::update(cx, |data| {
                    data.state.services.group_expanded.system = false;
                    data.state.services.group_expanded.user = false;
                });
            });
            self.engine.publish_with(|snapshot| {
                snapshot.services = Arc::new(ServicesSnapshot {
                    units: vec![unit_record(unit)].into(),
                    ..ServicesSnapshot::default()
                });
            });
            self.draw();
        }

        /// The expanded rows of the last Services query the page sent.
        fn services_expanded(&self) -> Vec<SysNodeId> {
            self.engine
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

        fn active_tab(&mut self) -> Tab {
            self.vcx.update(|_, cx| {
                main_window_view(cx)
                    .expect("main window")
                    .read(cx)
                    .active_tab()
            })
        }

        fn table_queries(&self) -> Vec<tl_model::table::TableQuery> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::SetTableQuery(q) => Some(q),
                    _ => None,
                })
                .collect()
        }

        /// Answers the latest Processes query with `keys` as the view.
        fn answer(&mut self, keys: &[ProcessKey]) {
            let generation = self
                .table_queries()
                .last()
                .expect("a Processes query was sent")
                .generation;
            let rows: Vec<TableRow> = keys
                .iter()
                .map(|key| TableRow {
                    node: NodeId::Process(*key),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: Some(0),
                    group: None,
                    descendants: 0,
                    subtree: None,
                })
                .collect();
            self.engine.publish_with(|s| {
                let header = ComponentHeader {
                    version: s.sequence,
                    ..ComponentHeader::default()
                };
                s.table = Arc::new(ProcessTableView {
                    header,
                    query_generation: generation,
                    rows: rows.into(),
                    ..ProcessTableView::default()
                });
            });
            self.draw();
        }

        /// Cookies selected in the Connections table.
        fn selected_conns(&mut self) -> Vec<u64> {
            let table = self.vcx.read_entity(&self.page, |p, _| p.table().clone());
            self.vcx
                .read_entity(&table, |t, _| t.selection.ids().collect())
        }

        /// Publishes the record with an empty view, as a filter that hides it would.
        fn publish_hidden(&mut self, record: ConnRecord) {
            let generation = self
                .engine
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetSysTableQuery(query) => Some(query.generation),
                    _ => None,
                })
                .unwrap_or(0);
            self.engine.publish_with(|snapshot| {
                snapshot.connections = Arc::new(ConnectionsSnapshot {
                    header: ComponentHeader {
                        version: 2,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    query_generation: generation,
                    records: vec![record].into(),
                    ..ConnectionsSnapshot::default()
                });
            });
            self.draw();
        }

        fn selected(&mut self) -> Vec<tl_ui::pages::processes::delegate::ProcRowId> {
            self.vcx.update(|_, cx| {
                let view = main_window_view(cx).expect("main window");
                let page = view
                    .read(cx)
                    .page_entity::<ProcessesPage>(Tab::Processes)
                    .expect("Processes page");
                page.read(cx).table().read(cx).selection.ids().collect()
            })
        }
    }

    fn owned(pids: &[u32]) -> ConnRecord {
        let owners: SmallVec<[ProcessKey; 1]> = pids.iter().map(|p| super::key(*p)).collect();
        super::connection(1, owners)
    }

    fn procs(pids: &[u32]) -> Vec<ProcessRecord> {
        pids.iter()
            .map(|p| super::process(*p, "daemon", "root", 0))
            .collect()
    }

    #[gpui_kit::test]
    fn conn_jump_uses_lowest_pid(cx: &mut TestAppContext) {
        let mut rig = rig(cx, owned(&[5000, 4321]), procs(&[5000, 4321]));
        rig.dispatch(true, ConnectionJumpToProcess);
        rig.answer(&[super::key(4321)]);
        assert_eq!(
            rig.selected(),
            [tl_ui::pages::processes::delegate::ProcRowId::Process(
                super::key(4321)
            )]
        );
    }

    #[gpui_kit::test]
    fn conn_jump_success_toast(cx: &mut TestAppContext) {
        let mut rig = rig(cx, owned(&[4321]), procs(&[4321]));
        rig.dispatch(true, ConnectionJumpToProcess);
        rig.answer(&[super::key(4321)]);
        assert_eq!(rig.active_tab(), Tab::Processes);
        assert_eq!(rig.selected().len(), 1);
        assert_eq!(rig.toasts(), ["Switched to owning process."]);
    }

    #[gpui_kit::test]
    fn conn_jump_hidden_show_button(cx: &mut TestAppContext) {
        let mut rig = rig(cx, owned(&[4321]), procs(&[4321]));
        // Build the lazy Processes page, then come back.
        rig.vcx.update(|window, cx| {
            let view = main_window_view(cx).expect("main window");
            view.update(cx, |v, cx| v.select_tab(Tab::Processes, window, cx));
            view.update(cx, |v, cx| v.select_tab(Tab::Connections, window, cx));
        });
        rig.draw();
        let processes = rig.vcx.update(|_, cx| {
            main_window_view(cx)
                .expect("main window")
                .read(cx)
                .page_entity::<ProcessesPage>(Tab::Processes)
                .expect("Processes page")
        });
        rig.vcx.update(|_, cx| {
            processes.update(cx, |p, cx| {
                p.table()
                    .update(cx, |t, _| t.delegate.query_mut().set_filter("foo".into()));
            });
        });
        rig.dispatch(true, ConnectionJumpToProcess);
        rig.answer(&[]);
        assert_eq!(
            rig.toasts(),
            ["Owning process not visible in the current process view."]
        );
        assert!(rig.selected().is_empty());
        // The Show button dispatches this action and clears the filter.
        let before = rig.table_queries().len();
        rig.vcx
            .dispatch_action(tl_ui::pages::processes::actions::ProcessesShowHidden);
        rig.draw();
        let queries = rig.table_queries();
        assert!(queries.len() > before, "Show sends a new query");
        assert_eq!(queries.last().map(|q| q.filter.as_ref()), Some(""));
    }

    #[gpui_kit::test]
    fn conn_jump_exited_toast(cx: &mut TestAppContext) {
        // The owner is no longer in the process snapshot.
        let mut rig = rig(cx, owned(&[4321]), Vec::new());
        rig.dispatch(true, ConnectionJumpToProcess);
        assert_eq!(rig.toasts(), ["Process not found in current view"]);
    }

    #[gpui_kit::test]
    fn conn_jump_no_owner_toast(cx: &mut TestAppContext) {
        let mut rig = rig(cx, owned(&[]), Vec::new());
        let before = rig.table_queries().len();
        rig.dispatch(true, ConnectionJumpToProcess);
        assert_eq!(rig.toasts(), ["No owning process for this connection"]);
        assert_eq!(rig.active_tab(), Tab::Connections);
        assert_eq!(rig.table_queries().len(), before, "no query is sent");
    }

    #[gpui_kit::test]
    fn go_to_connection_selects_row(cx: &mut TestAppContext) {
        let mut rig = rig(cx, owned(&[4321]), procs(&[4321]));
        rig.vcx.dispatch_action(GoToConnection { inode: 1234567 });
        rig.draw();
        assert_eq!(rig.selected_conns(), [1]);
        assert_eq!(rig.active_tab(), Tab::Connections);
        assert!(rig.toasts().is_empty());
    }

    #[gpui_kit::test]
    fn go_to_connection_unknown_inode_toasts(cx: &mut TestAppContext) {
        let mut rig = rig(cx, owned(&[4321]), procs(&[4321]));
        rig.vcx.dispatch_action(GoToConnection { inode: 99 });
        rig.draw();
        assert!(rig.selected_conns().is_empty());
        assert_eq!(rig.toasts(), ["Connection not found in current view"]);
    }

    #[gpui_kit::test]
    fn go_to_connection_filtered_out_toasts(cx: &mut TestAppContext) {
        let mut rig = rig(cx, owned(&[4321]), procs(&[4321]));
        rig.publish_hidden(owned(&[4321]));
        rig.vcx.dispatch_action(GoToConnection { inode: 1234567 });
        rig.draw();
        assert!(rig.selected_conns().is_empty());
        assert_eq!(rig.toasts(), ["Connection not found in current view"]);
    }

    #[gpui_kit::test]
    fn conn_go_to_service(cx: &mut TestAppContext) {
        let mut record = owned(&[4321]);
        record.unit = Some(UnitRef {
            manager: Manager::System,
            name: Arc::from("sshd.service"),
        });
        let mut rig = rig(cx, record, procs(&[4321]));
        rig.publish_unit(UnitRef {
            manager: Manager::System,
            name: Arc::from("sshd.service"),
        });
        // The main window handles GoToService itself: it switches to Services and the page
        // looks the unit up by manager and name.
        assert_eq!(rig.active_tab(), Tab::Connections);
        rig.dispatch(true, ConnectionGoToService);
        assert_eq!(rig.active_tab(), Tab::Services);
        assert!(
            rig.toasts().iter().any(|t| t == "Switched to Services tab"),
            "the page found System sshd.service: {:?}",
            rig.toasts()
        );
        assert_eq!(
            rig.services_expanded(),
            [SysNodeId::ServiceGroup(Manager::System)],
            "the jump expanded only the System group"
        );
    }
}

/// The Connections tab inside `MainWindow`, end to end through the fake engine
/// (tasklens-9o0.2, UI-SYS-060 to UI-SYS-068, table (b)).
mod main_window {
    use super::*;
    use gpui_kit::AppContext as _;
    use gpui_kit::{Entity, Modifiers, MouseButton, MouseDownEvent, VisualTestContext, point};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::ids::NodeId;
    use tl_model::snapshot::ConnCounts;
    use tl_model::table::{ProcessTableView, SysTable, TableRow};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::menu::{MenuEntryInfo, TlMenuPopup, open_context_menu};
    use tl_ui::demand::DEMAND_COALESCE;
    use tl_ui::pages::connections::ConnectionsPage;
    use tl_ui::pages::connections::delegate::ConnectionsDelegate;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::table::tree_table::TreeTableDelegate as _;
    use tl_ui::window::main::{MainWindow, main_window_view, open_main_window};
    use tl_ui_settings::settings::{Settings, StartTab, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    const FIREFOX_COOKIE: u64 = 2;

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        main: Entity<MainWindow>,
        version: u64,
    }

    fn boot(cx: &mut TestAppContext) -> Rig {
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
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        cx.run_until_parked();
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.simulate_resize(gpui_kit::size(px(1280.), px(800.)));
        vcx.run_until_parked();
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        Rig {
            engine,
            vcx,
            main,
            version: 0,
        }
    }

    fn fixture() -> (Vec<ConnRecord>, Vec<ProcessRecord>, ConnCounts) {
        let owner = |pid: u32| -> SmallVec<[ProcessKey; 1]> { SmallVec::from_slice(&[key(pid)]) };
        let mut sshd = super::connection(1, owner(812));
        sshd.uid = 0;
        sshd.state = ConnState::Listening;
        sshd.local = Arc::from("0.0.0.0:22");
        sshd.foreign = Arc::from("0.0.0.0:0");
        let firefox = super::connection(FIREFOX_COOKIE, owner(4321));
        let mut cupsd = super::connection(3, owner(900));
        cupsd.uid = 0;
        cupsd.family = Family::Inet6;
        cupsd.state = ConnState::Listening;
        cupsd.local = Arc::from("[::1]:631");
        cupsd.foreign = Arc::from("[::]:0");
        let mut kernel = super::connection(4, SmallVec::new());
        kernel.state = ConnState::TimeWait;
        kernel.inode = 0;
        let mut avahi = super::connection(5, owner(700));
        avahi.proto = Proto::Udp;
        avahi.state = ConnState::Unconnected;
        avahi.local = Arc::from("0.0.0.0:5353");
        avahi.foreign = Arc::from("0.0.0.0:0");
        let processes = vec![
            super::process(812, "sshd", "root", 0),
            super::process(4321, "firefox", "user", 1000),
            super::process(900, "cupsd", "root", 0),
            super::process(700, "avahi-daemon", "avahi", 970),
        ];
        let counts = ConnCounts {
            total: 5,
            tcp: 4,
            udp: 1,
            established: 1,
            listening: 2,
        };
        (vec![sshd, firefox, cupsd, kernel, avahi], processes, counts)
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn select(&mut self, tab: Tab) {
            self.main
                .update_in(&mut self.vcx, |m, window, cx| m.select_tab(tab, window, cx));
            self.draw();
        }

        fn active_tab(&mut self) -> Tab {
            self.vcx.read_entity(&self.main, |m, _| m.active_tab())
        }

        fn page(&mut self) -> Option<Entity<ConnectionsPage>> {
            self.vcx.read_entity(&self.main, |m, _| {
                m.page_entity::<ConnectionsPage>(Tab::Connections)
            })
        }

        fn table(&mut self) -> Entity<tl_ui::table::tree_table::TlTreeTable<ConnectionsDelegate>> {
            let page = self.page().expect("the Connections page exists");
            self.vcx.read_entity(&page, |page, _| page.table().clone())
        }

        /// Publishes the fixture for the last Connections query the page sent.
        fn publish(&mut self) {
            self.version += 1;
            let version = self.version;
            let generation = self
                .engine
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetSysTableQuery(query) if query.table == SysTable::Connections => {
                        Some(query.generation)
                    }
                    _ => None,
                })
                .expect("the page sent a Connections query");
            let (records, mut processes, counts) = fixture();
            processes.sort_unstable_by_key(|process| process.key);
            let rows: Vec<SysRow> = records
                .iter()
                .enumerate()
                .map(|(index, record)| SysRow {
                    node: SysNodeId::Conn(record.cookie),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: Some(index as u32),
                })
                .collect();
            self.engine.publish_with(|snapshot| {
                snapshot.connections = Arc::new(ConnectionsSnapshot {
                    header: ComponentHeader {
                        version,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    rows: rows.into(),
                    query_generation: generation,
                    records: records.into(),
                    counts,
                    ..ConnectionsSnapshot::default()
                });
                snapshot.processes = Arc::new(ProcessSnapshot {
                    records: processes.into(),
                    ..ProcessSnapshot::default()
                });
            });
            self.draw();
        }

        /// Visits the Connections tab and publishes the fixture.
        fn visit_with_fixture(&mut self) {
            self.select(Tab::Connections);
            self.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
            self.publish();
        }

        fn chip(&mut self) -> String {
            let chip = self
                .vcx
                .read_entity(&self.main, |m, _| m.status_chip().clone());
            self.vcx.read_entity(&chip, |c, _| c.drawn_text())
        }

        fn page_status(&mut self) -> String {
            let page = self.page().expect("the Connections page exists");
            self.vcx.read_entity(&page, |p, _| {
                let mut out = String::new();
                p.status_text(&mut out);
                out
            })
        }

        /// Right-clicks the first cell of the row for `cookie`.
        fn right_click(&mut self, cookie: u64) -> Option<Entity<TlMenuPopup>> {
            self.draw();
            let table = self.table();
            let ix = self
                .vcx
                .read_entity(&table, |t, _| t.delegate.row_index(cookie))
                .expect("the row exists");
            let body = self
                .vcx
                .debug_bounds("TlTreeTable-body")
                .expect("the table body");
            let row_h = self
                .vcx
                .update(|_, cx| tl_ui::theme::metrics(cx).row_height);
            let at = point(
                body.origin.x + px(40.),
                body.origin.y + row_h * (ix as f32 + 0.5),
            );
            self.vcx.simulate_event(MouseDownEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Right,
                click_count: 1,
                first_mouse: false,
            });
            self.vcx.run_until_parked();
            self.draw();
            self.vcx.update(|window, cx| open_context_menu(window, cx))
        }

        /// Runs the item labelled `label` as a click does: in the table's context.
        fn run_item(&mut self, menu: &Entity<TlMenuPopup>, label: &str) {
            let entries: Vec<MenuEntryInfo> = self
                .vcx
                .update(|window, cx| menu.read(cx).entries_for_test(window));
            let action = entries
                .into_iter()
                .find(|e| e.label.as_deref() == Some(label))
                .and_then(|e| e.action)
                .unwrap_or_else(|| panic!("a {label} item"));
            let table = self.table();
            self.vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                window.dispatch_action(action, cx);
            });
            self.vcx.run_until_parked();
        }

        /// Answers the latest Processes query with `keys` as the view.
        fn answer(&mut self, keys: &[ProcessKey]) {
            let generation = self
                .engine
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetTableQuery(query) => Some(query.generation),
                    _ => None,
                })
                .expect("a Processes query was sent");
            let rows: Vec<TableRow> = keys
                .iter()
                .map(|key| TableRow {
                    node: NodeId::Process(*key),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: Some(0),
                    group: None,
                    descendants: 0,
                    subtree: None,
                })
                .collect();
            self.engine.publish_with(|s| {
                let header = ComponentHeader {
                    version: s.sequence,
                    ..ComponentHeader::default()
                };
                s.table = Arc::new(ProcessTableView {
                    header,
                    query_generation: generation,
                    rows: rows.into(),
                    ..ProcessTableView::default()
                });
            });
            self.draw();
        }

        fn processes_selected(&mut self) -> Vec<ProcRowId> {
            self.vcx.update(|_, cx| {
                let page = main_window_view(cx)
                    .expect("main window")
                    .read(cx)
                    .page_entity::<ProcessesPage>(Tab::Processes)
                    .expect("Processes page");
                page.read(cx).table().read(cx).selection.ids().collect()
            })
        }

        fn last_demand_connections(&self) -> Option<bool> {
            self.engine.demands().last().map(|d| d.connections)
        }
    }

    #[gpui_kit::test]
    fn connections_tab_mounted_and_kept(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        assert!(rig.page().is_none(), "no page before the first visit");
        rig.vcx.simulate_keystrokes("ctrl-5");
        rig.draw();
        assert_eq!(rig.active_tab(), Tab::Connections);
        let first = rig.page().expect("Ctrl+5 builds the Connections page");
        let expected = rig.page_status();
        assert!(!expected.is_empty(), "the page has status text");
        assert_eq!(rig.chip(), expected, "the chip shows the page text");
        rig.select(Tab::Processes);
        rig.select(Tab::Connections);
        let again = rig.page().expect("the page is kept");
        assert_eq!(first.entity_id(), again.entity_id());
    }

    #[gpui_kit::test]
    fn connections_page_layout_golden(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.visit_with_fixture();
        let header = rig
            .vcx
            .debug_bounds("TlTreeTableHeader")
            .expect("header drawn");
        let mut last_x = px(-1.);
        for key in ["proto", "local", "foreign", "state", "pid", "process"] {
            let b = rig
                .vcx
                .debug_bounds(format!("TlTreeTableHeader-label-{key}").leak())
                .unwrap_or_else(|| panic!("{key} label drawn"));
            assert!(b.origin.x > last_x, "{key} is right of the one before");
            assert!(
                b.origin.y >= header.origin.y && b.bottom() <= header.bottom(),
                "{key} sits in the header"
            );
            last_x = b.origin.x;
        }
        let table = rig.table();
        assert_eq!(
            rig.vcx.read_entity(&table, |t, _| t.delegate.row_count()),
            5
        );
        let body = rig
            .vcx
            .debug_bounds("TlTreeTable-body")
            .expect("body drawn");
        assert!(
            body.origin.y >= header.bottom(),
            "rows are below the header"
        );
        assert_eq!(rig.chip(), "5 Total | 4 TCP | 1 UDP | 1 EST | 2 LISTEN");
    }

    #[gpui_kit::test]
    fn connections_end_to_end_jump(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
        rig.answer(&[key(4321)]);
        rig.visit_with_fixture();
        let menu = rig.right_click(FIREFOX_COOKIE).expect("the menu opens");
        rig.run_item(&menu, "Jump to process");
        rig.answer(&[key(4321)]);
        assert_eq!(rig.active_tab(), Tab::Processes);
        assert_eq!(rig.processes_selected(), [ProcRowId::Process(key(4321))]);
    }

    #[gpui_kit::test]
    fn connections_demand_cleared_on_tab_switch(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.visit_with_fixture();
        rig.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
        rig.vcx.run_until_parked();
        assert_eq!(
            rig.last_demand_connections(),
            Some(true),
            "the Connections tab asks for connection data"
        );
        rig.select(Tab::Information);
        rig.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
        rig.vcx.run_until_parked();
        assert_eq!(
            rig.last_demand_connections(),
            Some(false),
            "leaving the tab clears the demand"
        );
    }
}

/// The Connection Properties window with `MainWindow` as the host: the demand while the tab
/// changes, and the Process link that jumps to the Processes tab (tasklens-9o0.19).
mod props {
    use super::*;
    use gpui_kit::base::Root;
    use gpui_kit::{VisualTestContext, WindowHandle};
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::snapshot::{ConnCounts, ProcessSnapshot};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::demand::GlobalDemand;
    use tl_ui::host::HostContext;
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui::window::registry::{GlobalRegistry, WindowKey};
    use tl_ui::windows::connection_properties::{
        ConnectionPropertiesView, open_connection_properties,
    };
    use tl_ui_settings::settings::{Decorations, Settings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    struct Rig {
        engine: Arc<FakeEngine>,
        main: VisualTestContext,
        version: u64,
    }

    fn rig(
        cx: &mut TestAppContext,
        records: Vec<ConnRecord>,
        processes: Vec<ProcessRecord>,
    ) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        // The window is checked with the kit title bar and the client frame. The default is
        // server decorations (D-035).
        let mut settings = Settings::default();
        settings.appearance.decorations = Decorations::Client;
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
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Connections, cx))
            .expect("the main window opens");
        let mut main = VisualTestContext::from_window(handle.into(), cx);
        main.update(|window, _| window.activate_window());
        main.run_until_parked();
        let mut rig = Rig {
            engine,
            main,
            version: 0,
        };
        rig.publish(records, processes);
        rig
    }

    impl Rig {
        fn publish(&mut self, records: Vec<ConnRecord>, mut processes: Vec<ProcessRecord>) {
            self.version += 1;
            processes.sort_unstable_by_key(|process| process.key);
            let version = self.version;
            self.engine.publish_with(|snapshot| {
                snapshot.connections = Arc::new(ConnectionsSnapshot {
                    header: ComponentHeader {
                        version,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    records: records.into(),
                    counts: ConnCounts::default(),
                    ..ConnectionsSnapshot::default()
                });
                snapshot.processes = Arc::new(ProcessSnapshot {
                    records: processes.into(),
                    ..ProcessSnapshot::default()
                });
            });
            self.main.run_until_parked();
        }
    }

    fn open(cx: &mut TestAppContext, cookie: u64) -> Option<WindowHandle<Root>> {
        cx.update(|cx| open_connection_properties(&HostContext::local(cx), cookie, cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            let handle = registry
                .read(cx)
                .get(&WindowKey::ConnectionProperties(cookie));
            handle.and_then(|h| h.downcast::<Root>())
        })
    }

    fn frame(
        cx: &mut TestAppContext,
        handle: WindowHandle<Root>,
    ) -> gpui_kit::Entity<TlSecondaryWindow<ConnectionPropertiesView>> {
        cx.update(|cx| TlSecondaryWindow::<ConnectionPropertiesView>::find(handle, cx))
            .expect("connection properties frame")
    }

    fn tcp_info() -> TcpInfoView {
        TcpInfoView {
            rtt_us: 420,
            rttvar_us: 100,
            snd_cwnd: 10,
            bytes_acked: 1_280_000,
            bytes_received: 36_363_000,
            retransmits: 3,
            last_data_recv_ms: 1240,
        }
    }

    fn tcp_record(cookie: u64) -> ConnRecord {
        let mut record = super::connection(cookie, SmallVec::from_slice(&[super::key(4321)]));
        record.local = Arc::from("192.168.1.10:51234");
        record.foreign = Arc::from("93.184.216.34:443");
        record.recv_q = 0;
        record.interface = Some(Arc::from("eth0"));
        record.tcp = Some(tcp_info());
        record
    }

    fn procs() -> Vec<ProcessRecord> {
        vec![super::process(4321, "curl", "alice", 1000)]
    }

    #[gpui_kit::test]
    fn conn_props_keeps_demand_while_open(cx: &mut TestAppContext) {
        let mut rig = rig(cx, vec![tcp_record(1)], procs());
        let handle = open(cx, 1).expect("opened");
        rig.main.update(|window, cx| {
            let view = main_window_view(cx).expect("main window");
            view.update(cx, |v, cx| v.select_tab(Tab::Processes, window, cx));
        });
        rig.main.run_until_parked();
        let demanded = |cx: &mut TestAppContext| {
            cx.update(|cx| cx.global::<GlobalDemand>().0.read(cx).merged().connections)
        };
        assert!(demanded(cx), "the open window holds Connections demand");
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        cx.run_until_parked();
        assert!(!demanded(cx), "released after the window closes");
    }

    #[gpui_kit::test]
    fn conn_props_process_link_jumps(cx: &mut TestAppContext) {
        let rig = rig(cx, vec![tcp_record(1)], procs());
        let handle = open(cx, 1).expect("opened");
        let mut win = VisualTestContext::from_window(handle.into(), cx);
        let frame = frame(cx, handle);
        let view = cx.update(|cx| frame.read(cx).content().clone());
        win.update(|window, _| window.activate_window());
        win.run_until_parked();
        win.update(|window, cx| window.draw(cx).clear(cx));
        win.update(|window, cx| view.read(cx).link_focus_handle().clone().focus(window, cx));
        win.run_until_parked();
        win.update(|window, cx| window.draw(cx).clear(cx));
        let focused = win.update(|window, cx| view.read(cx).link_focus_handle().is_focused(window));
        assert!(focused, "the Process link has focus");
        let keystroke = gpui_kit::Keystroke::parse("enter").unwrap();
        win.simulate_event(gpui_kit::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        win.simulate_event(gpui_kit::KeyUpEvent { keystroke });
        win.run_until_parked();
        let mut rig = rig;
        let tab = rig.main.update(|_, cx| {
            main_window_view(cx)
                .expect("main window")
                .read(cx)
                .active_tab()
        });
        assert_eq!(tab, Tab::Processes);
    }
}
