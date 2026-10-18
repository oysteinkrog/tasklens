//! Connections delegate tests (tasklens-9o0.14, UI-SYS-061 and UI-SYS-062).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{App, SharedString, TestAppContext, px};
use smallvec::SmallVec;
use tl_model::actions::{Family, FourTuple, Proto};
use tl_model::ids::ProcessKey;
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::snapshot::{
    ComponentHeader, ConnRecord, ConnState, ConnectionsSnapshot, ProcessSnapshot, RemoteHost,
    Snapshot,
};
use tl_model::table::{ConnColumn, SysNodeId, SysRow};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::pages::connections::delegate::{
    CONNECTION_COLUMNS, ConnectionsDelegate, DEFAULT_COLUMNS, default_width, id, state_class,
};
use tl_ui::pages::sys_table::SysDelegate as _;
use tl_ui::table::cells::{CellCtx, CellOut, name_look};
use tl_ui::table::columns::TableColumn as _;
use tl_ui::table::tree_table::{NameIcon, RowFlags, TreeTableDelegate as _};
use tl_ui::theme::{current, metrics};

fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    tl_ui::theme::store::init(
        cx,
        &ThemeSettings::default(),
        &AppearanceSettings::default(),
        Vec::new(),
    );
}

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

fn delegate(
    cx: &mut TestAppContext,
    records: Vec<ConnRecord>,
    mut processes: Vec<ProcessRecord>,
) -> ConnectionsDelegate {
    processes.sort_unstable_by_key(|process| process.key);
    cx.update(|cx| {
        setup(cx);
        let mut delegate = ConnectionsDelegate::new(cx);
        let rows = records
            .iter()
            .enumerate()
            .map(|(ix, record)| SysRow {
                node: SysNodeId::Conn(record.cookie),
                depth: 0,
                expanded: false,
                has_children: false,
                record: Some(ix as u32),
            })
            .collect::<Vec<_>>();
        let mut snapshot = Snapshot::empty(BootId(1), 1);
        snapshot.connections = Arc::new(ConnectionsSnapshot {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: rows.into(),
            query_generation: 1,
            records: records.into(),
            ..ConnectionsSnapshot::default()
        });
        snapshot.processes = Arc::new(ProcessSnapshot {
            records: processes.into(),
            ..ProcessSnapshot::default()
        });
        delegate.install(&Arc::new(snapshot));
        delegate
    })
}

fn cell(delegate: &ConnectionsDelegate, column: ConnColumn) -> CellOut {
    cell_at(delegate, 0, column)
}

fn cell_at(delegate: &ConnectionsDelegate, row: usize, column: ConnColumn) -> CellOut {
    let mut out = CellOut {
        want_text: true,
        want_tooltip: true,
        ..CellOut::default()
    };
    delegate.cell(row, id(column), &mut out);
    out
}

fn text(out: &CellOut) -> String {
    out.arc
        .as_deref()
        .map_or_else(|| out.text.clone(), ToOwned::to_owned)
}

#[gpui_kit::test]
fn connections_columns_default_and_optional(cx: &mut TestAppContext) {
    let delegate = cx.update(|cx| {
        setup(cx);
        ConnectionsDelegate::new(cx)
    });
    assert_eq!(
        delegate.columns().order,
        CONNECTION_COLUMNS.map(id).as_slice()
    );
    let minimums = [
        60., 120., 120., 90., 60., 60., 60., 60., 60., 60., 60., 60., 60., 60., 60.,
    ];
    let templates = [
        None,
        None,
        None,
        None,
        Some("999999 +99"),
        None,
        None,
        None,
        Some("99999"),
        Some("99999"),
        Some("999999"),
        Some("999999"),
        None,
        Some("9999999999"),
        None,
    ];
    for ((column, minimum), template) in CONNECTION_COLUMNS.into_iter().zip(minimums).zip(templates)
    {
        assert_eq!(
            delegate.columns().widths[&id(column)],
            px(default_width(column))
        );
        assert_eq!(
            delegate.columns().visible.contains(&id(column)),
            DEFAULT_COLUMNS.contains(&column)
        );
        assert_eq!(id(column).min_width(), Some(minimum), "{column:?}");
        assert_eq!(
            delegate.column_spec(id(column)).template,
            template,
            "{column:?}"
        );
    }
}

#[gpui_kit::test]
fn conn_row_shared_pid_plus_n(cx: &mut TestAppContext) {
    let processes = vec![
        process(5000, "five", "alice", 1000),
        process(4321, "first", "alice", 1000),
        process(4400, "second", "alice", 1000),
    ];
    let owners = SmallVec::from_vec(processes.iter().map(|p| p.key).collect());
    let delegate = delegate(cx, vec![connection(1, owners)], processes);
    assert_eq!(text(&cell(&delegate, ConnColumn::Pid)), "4321 +2");
    let pid_tip = cell(&delegate, ConnColumn::Pid).tooltip.unwrap();
    let process_tip = cell(&delegate, ConnColumn::Process).tooltip.unwrap();
    assert_eq!(pid_tip, process_tip);
    assert_eq!(pid_tip.lines().count(), 3);
    assert_eq!(delegate.name_cell(0).primary, "first");
    assert_eq!(
        delegate.owner_tooltip_lines(0).unwrap(),
        ["PID 4321  first", "PID 4400  second", "PID 5000  five"].map(SharedString::from)
    );
}

#[gpui_kit::test]
fn conn_shared_tooltip_cap_16(cx: &mut TestAppContext) {
    let processes = (1..=20)
        .map(|pid| process(pid, &format!("p{pid}"), "alice", 1000))
        .collect::<Vec<_>>();
    let owners = SmallVec::from_vec(processes.iter().map(|p| p.key).collect());
    let delegate = delegate(cx, vec![connection(1, owners)], processes);
    let lines = delegate.owner_tooltip_lines(0).unwrap();
    assert_eq!(lines.len(), 17);
    assert_eq!(lines.last().unwrap(), "and 4 more");
}

#[gpui_kit::test]
fn conn_row_kernel_owner(cx: &mut TestAppContext) {
    let mut record = connection(1, SmallVec::new());
    record.state = ConnState::TimeWait;
    record.inode = 0;
    let delegate = delegate(cx, vec![record], Vec::new());
    assert_eq!(text(&cell(&delegate, ConnColumn::Pid)), "-");
    let name = delegate.name_cell(0);
    assert_eq!(name.primary, "No owner (kernel)");
    assert!(matches!(name.icon, NameIcon::None));
    let flags = delegate.row_flags(0);
    assert!(flags.has(RowFlags::SECONDARY_NAME));
    cx.update(|cx| {
        let theme = current(cx).clone();
        let ctx = CellCtx {
            m: metrics(cx),
            theme: &theme,
            selected: false,
            width: px(180.),
        };
        assert_eq!(name_look(flags, &ctx), (theme.text_secondary, 1.0));
    });
}

#[gpui_kit::test]
fn conn_row_unresolved_owner_is_question_mark(cx: &mut TestAppContext) {
    let record = connection(1, SmallVec::new());
    let delegate = delegate(cx, vec![record], Vec::new());
    assert_eq!(text(&cell(&delegate, ConnColumn::Pid)), "?");
    assert_eq!(delegate.name_cell(0).primary, "?");
}

#[gpui_kit::test]
fn conn_row_restricted_lock(cx: &mut TestAppContext) {
    let mut record = connection(1, SmallVec::new());
    record.owner_validity = Validity::Restricted;
    record.uid = 1001;
    let delegate = delegate(cx, vec![record], vec![process(9, "hidden", "bob", 1001)]);
    assert_eq!(text(&cell(&delegate, ConnColumn::Pid)), "?");
    assert_eq!(delegate.name_cell(0).glyphs[0].0, IconName::Lock);
    let pid_tip = cell(&delegate, ConnColumn::Pid).tooltip.unwrap();
    let process_tip = cell(&delegate, ConnColumn::Process).tooltip.unwrap();
    assert_eq!(pid_tip, process_tip);
    assert_eq!(
        delegate.owner_tooltip_lines(0).unwrap()[0],
        "Owned by bob. Needs the tasklens helper to identify the process"
    );
    assert_eq!(
        pid_tip,
        "Owned by bob. Needs the tasklens helper to identify the process"
    );
}

#[gpui_kit::test]
#[cfg(feature = "test-hooks")]
fn conn_restricted_tooltip_is_localized(cx: &mut TestAppContext) {
    let mut delegate = cx.update(|cx| {
        setup(cx);
        tl_ui::i18n::init_strings(cx, "en-US");
        tl_ui::i18n::set_test_catalogue(
            cx,
            "connections-owner-restricted-tip = Socket de { $user } requires helper",
        );
        ConnectionsDelegate::new(cx)
    });
    let mut record = connection(1, SmallVec::new());
    record.owner_validity = Validity::Restricted;
    record.uid = 1001;
    let snapshot = cx.update(|_| {
        let mut snapshot = Snapshot::empty(BootId(1), 1);
        snapshot.connections = Arc::new(ConnectionsSnapshot {
            rows: vec![SysRow {
                node: SysNodeId::Conn(1),
                depth: 0,
                expanded: false,
                has_children: false,
                record: Some(0),
            }]
            .into(),
            records: vec![record].into(),
            ..ConnectionsSnapshot::default()
        });
        snapshot.processes = Arc::new(ProcessSnapshot {
            records: vec![process(9, "hidden", "bob", 1001)].into(),
            ..ProcessSnapshot::default()
        });
        Arc::new(snapshot)
    });
    delegate.install(&snapshot);
    assert_eq!(
        delegate.owner_tooltip_lines(0).unwrap()[0],
        "Socket de bob requires helper"
    );
}

#[gpui_kit::test]
fn conn_state_labels_and_tones(cx: &mut TestAppContext) {
    let states = [
        ConnState::from_tcp_state(10),
        ConnState::from_tcp_state(12),
        ConnState::from_tcp_state(7),
        ConnState::from_tcp_state(13),
    ];
    let records = states
        .into_iter()
        .enumerate()
        .map(|(ix, state)| {
            let mut record = connection(ix as u64 + 1, SmallVec::new());
            record.state = state;
            record
        })
        .collect();
    let delegate = delegate(cx, records, Vec::new());
    let theme = cx.update(|cx| current(cx).clone());
    let cells = (0..states.len())
        .map(|row| cell_at(&delegate, row, ConnColumn::State))
        .collect::<Vec<_>>();

    assert_eq!(text(&cells[0]), "LISTENING");
    assert_eq!(cells[0].text_color, Some(theme.status.info));
    assert_eq!(text(&cells[1]), "SYN-RECEIVED");
    assert_eq!(cells[1].text_color, None);
    assert_eq!(text(&cells[2]), "CLOSED");
    assert_eq!(cells[2].text_color, Some(theme.text_secondary));
    assert_eq!(text(&cells[3]), "UNKNOWN");
    assert_eq!(cells[3].text_color, Some(theme.status.warning));
    assert_eq!(state_class(states[1]), state_class(ConnState::SynReceived));
}

#[gpui_kit::test]
fn conn_foreign_wildcard_ports_are_empty(cx: &mut TestAppContext) {
    let records = ["*:*", "0.0.0.0:*", "[::]:*"]
        .into_iter()
        .enumerate()
        .map(|(ix, foreign)| {
            let mut record = connection(ix as u64 + 1, SmallVec::new());
            record.foreign = Arc::from(foreign);
            record
        })
        .collect();
    let delegate = delegate(cx, records, Vec::new());
    for row in 0..3 {
        assert_eq!(text(&cell_at(&delegate, row, ConnColumn::ForeignPort)), "");
    }
}

#[gpui_kit::test]
fn conn_counts_grouping_and_inode(cx: &mut TestAppContext) {
    let delegate = delegate(cx, vec![connection(1, SmallVec::new())], Vec::new());
    assert_eq!(text(&cell(&delegate, ConnColumn::RecvQ)), "123\u{2009}456");
    assert_eq!(text(&cell(&delegate, ConnColumn::Inode)), "1234567");
}

#[gpui_kit::test]
fn conn_row_ids_stable_by_cookie(cx: &mut TestAppContext) {
    let mut delegate = delegate(
        cx,
        vec![
            connection(11, SmallVec::new()),
            connection(22, SmallVec::new()),
        ],
        Vec::new(),
    );
    assert_eq!([delegate.row_id(0), delegate.row_id(1)], [11, 22]);
    let second = cx.update(|_| {
        let mut snapshot = Snapshot::empty(BootId(1), 2);
        snapshot.connections = Arc::new(ConnectionsSnapshot {
            header: ComponentHeader {
                version: 2,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: vec![
                SysRow {
                    node: SysNodeId::Conn(22),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: Some(0),
                },
                SysRow {
                    node: SysNodeId::Conn(11),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: Some(1),
                },
            ]
            .into(),
            query_generation: 2,
            records: vec![
                connection(22, SmallVec::new()),
                connection(11, SmallVec::new()),
            ]
            .into(),
            ..ConnectionsSnapshot::default()
        });
        Arc::new(snapshot)
    });
    delegate.install(&second);
    assert_eq!([delegate.row_id(0), delegate.row_id(1)], [22, 11]);
    assert_eq!(delegate.row_index(11), Some(1));
    assert_eq!(delegate.row_index(22), Some(0));
}

/// The ConnectionsPage entity (tasklens-9o0.15, UI-SYS-060 and UI-SYS-067).
#[cfg(feature = "test-hooks")]
mod page {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use super::*;
    use gpui_kit::base::Root;
    use gpui_kit::{
        Action, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render,
        StyleRefinement, Styled as _, VisualTestContext, Window, WindowBounds, WindowOptions, div,
        point, size,
    };
    use tl_model::actions::ActionEvent;
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, CommandError, Tab};
    use tl_model::demand::Demand;
    use tl_model::health::SourceId;
    use tl_model::journal::{JournalHandle, JournalQuery};
    use tl_model::settings::{EngineSettings, Settings, UiState};
    use tl_model::snapshot::{ConnCounts, Snapshot};
    use tl_model::table::{Manager, SysColumnId, SysTable, UnitRef};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::menu::{MenuEntryInfo, MenuEntryKind, TlMenu, TlMenuPopup};
    use tl_ui::components::toast::ToastQueue;
    use tl_ui::demand::{DEMAND_COALESCE, GlobalDemand};
    use tl_ui::hub::GlobalHub;
    use tl_ui::pages::connections::actions::bind_keys_connections;
    use tl_ui::pages::connections::actions::{ConnectionCopyForeign, ConnectionCopyLocal};
    use tl_ui::pages::connections::{ConnectionsPage, REFRESH_TOAST_AFTER};
    use tl_ui::pages::processes::actions::CopyDetails;
    use tl_ui::pages::sys_table::SysBody;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{GlobalShellEvents, PauseChanged};

    struct Host {
        page: Entity<ConnectionsPage>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.page
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        page: Entity<ConnectionsPage>,
        queue: Entity<ToastQueue>,
        vcx: VisualTestContext,
        version: u64,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let engine = FakeEngine::new();
        rig_with_client(cx, engine.clone(), engine)
    }

    fn rig_with_client(
        cx: &mut TestAppContext,
        engine: Arc<FakeEngine>,
        client: Arc<dyn EngineClient>,
    ) -> Rig {
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
            TasklensUi::init(cx, client.clone(), init);
            bind_keys_connections(cx);
            cx.set_reduce_motion(true);
        });
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1280.), px(800.)),
            })),
            ..WindowOptions::default()
        };
        let page_slot = Rc::new(RefCell::new(None));
        let handle = cx
            .update(|cx| {
                let page_slot = Rc::clone(&page_slot);
                cx.open_window(options, move |window, cx| {
                    let hub = cx.global::<GlobalHub>().0.clone();
                    let demand = cx.global::<GlobalDemand>().0.clone();
                    let page = cx.new(|cx| ConnectionsPage::new(&hub, &demand, client, window, cx));
                    *page_slot.borrow_mut() = Some(page.clone());
                    let host = cx.new(|_| Host { page });
                    cx.new(|cx| Root::new(host, window, cx))
                })
            })
            .expect("the window opens");
        let page = page_slot.borrow_mut().take().expect("the page opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.))));
        let mut rig = Rig {
            engine,
            page,
            queue,
            vcx,
            version: 0,
        };
        rig.draw();
        rig
    }

    /// Publishes connection data only after the page sends a complete visible-surface demand.
    /// This matches the real worker's start condition from the leader decision.
    struct DemandDrivenEngine {
        inner: Arc<FakeEngine>,
    }

    impl EngineClient for DemandDrivenEngine {
        fn latest(&self) -> Arc<Snapshot> {
            self.inner.latest()
        }

        fn wake(&self) -> async_channel::Receiver<()> {
            self.inner.wake()
        }

        fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
            self.inner.action_events()
        }

        fn set_demand(&self, demand: Demand) {
            self.inner.set_demand(demand.clone());
            if !demand.any_surface_visible || !demand.connections {
                return;
            }
            let generation = self
                .inner
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetSysTableQuery(query) if query.table == SysTable::Connections => {
                        Some(query.generation)
                    }
                    _ => None,
                })
                .expect("connections query precedes demand flush");
            let record = super::connection(7, SmallVec::new());
            self.inner.publish_with(|snapshot| {
                snapshot.connections = Arc::new(ConnectionsSnapshot {
                    header: ComponentHeader {
                        version: 1,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    rows: vec![row(7, 0)].into(),
                    query_generation: generation,
                    records: vec![record].into(),
                    counts: ConnCounts {
                        total: 1,
                        tcp: 1,
                        established: 1,
                        ..ConnCounts::default()
                    },
                    ..ConnectionsSnapshot::default()
                });
            });
        }

        fn set_settings(&self, settings: EngineSettings) {
            self.inner.set_settings(settings);
        }

        fn send(&self, command: Command) -> Result<(), CommandError> {
            self.inner.send(command)
        }

        fn journal(&self, query: JournalQuery) -> JournalHandle {
            self.inner.journal(query)
        }
    }

    fn row(cookie: u64, record: usize) -> SysRow {
        SysRow {
            node: SysNodeId::Conn(cookie),
            depth: 0,
            expanded: false,
            has_children: false,
            record: Some(record as u32),
        }
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.update(|window, cx| {
                window.draw(cx).clear(cx);
                let hub = cx.global::<GlobalHub>().0.clone();
                hub.update(cx, |hub, cx| hub.note_render(cx));
            });
            self.vcx.run_until_parked();
        }

        fn set_active(&mut self, active: bool) {
            self.page
                .update(&mut self.vcx, |page, cx| page.set_active(active, cx));
            self.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
            self.vcx.run_until_parked();
        }

        fn generation(&self) -> u64 {
            self.engine
                .commands()
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetSysTableQuery(query) => Some(query.generation),
                    _ => None,
                })
                .unwrap_or(0)
        }

        fn publish(&mut self, cookies: &[u64], counts: ConnCounts) {
            let records = cookies
                .iter()
                .map(|cookie| super::connection(*cookie, SmallVec::new()))
                .collect::<Vec<_>>();
            self.publish_records(records, counts);
        }

        fn publish_records(&mut self, records: Vec<ConnRecord>, counts: ConnCounts) {
            self.publish_records_and_processes(records, Vec::new(), counts);
        }

        fn publish_records_and_processes(
            &mut self,
            records: Vec<ConnRecord>,
            mut processes: Vec<ProcessRecord>,
            counts: ConnCounts,
        ) {
            self.version += 1;
            let generation = self.generation();
            let rows = records
                .iter()
                .enumerate()
                .map(|(index, record)| row(record.cookie, index))
                .collect::<Vec<_>>();
            let version = self.version;
            processes.sort_unstable_by_key(|process| process.key);
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
            self.vcx.run_until_parked();
            self.draw();
        }

        fn build_menu(&mut self, targets: &[u64], col: Option<SysColumnId>) -> Entity<TlMenuPopup> {
            let table = self.table();
            self.vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                TlMenu::build(window, cx, focus, |menu, window, cx| {
                    table.update(cx, |table, cx| {
                        table.delegate.context_menu(targets, col, menu, window, cx)
                    })
                })
            })
        }

        fn menu_entries(&mut self, menu: &Entity<TlMenuPopup>) -> Vec<MenuEntryInfo> {
            self.vcx
                .update(|window, cx| menu.read(cx).entries_for_test(window))
        }

        fn status(&mut self) -> String {
            self.vcx.update(|_, cx| {
                let mut out = String::new();
                self.page.read(cx).status_text(&mut out);
                out
            })
        }

        fn table(&self) -> Entity<tl_ui::table::tree_table::TlTreeTable<ConnectionsDelegate>> {
            self.vcx
                .read_entity(&self.page, |page, _| page.table().clone())
        }

        fn set_paused(&mut self, paused: bool) {
            self.vcx.update(|_, cx| {
                let events = cx.global::<GlobalShellEvents>().0.clone();
                events.update(cx, |_, cx| cx.emit(PauseChanged(paused)));
            });
            self.vcx.run_until_parked();
        }

        fn refresh_toasts(&mut self) -> usize {
            self.vcx.update(|_, cx| {
                self.queue
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .filter(|text| text.as_ref() == "Connections refreshed.")
                    .count()
            })
        }

        fn select(&mut self, cookies: &[u64]) {
            let table = self.table();
            table.update(&mut self.vcx, |table, cx| {
                for (position, cookie) in cookies.iter().enumerate() {
                    if position == 0 {
                        table.selection.click(*cookie);
                    } else {
                        table.selection.ctrl_click(*cookie);
                    }
                }
                cx.notify();
            });
            self.vcx.run_until_parked();
        }

        fn dispatch(&mut self, action: impl Action) {
            let table = self.table();
            self.vcx
                .update(|window, cx| table.read(cx).focus_handle().clone().focus(window, cx));
            self.draw();
            self.vcx.dispatch_action(action);
            self.vcx.run_until_parked();
        }

        fn clipboard(&mut self) -> Option<String> {
            self.vcx.read_from_clipboard().and_then(|item| item.text())
        }

        fn copy_toasts(&mut self) -> Vec<String> {
            self.vcx.update(|window, cx| {
                ToastQueue::of_window(window, cx)
                    .map(|queue| {
                        queue
                            .read(cx)
                            .visible_texts()
                            .iter()
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            })
        }
    }

    fn menu_shape(entries: &[MenuEntryInfo]) -> Vec<(MenuEntryKind, String)> {
        entries
            .iter()
            .map(|entry| {
                (
                    entry.kind,
                    entry.label.as_deref().unwrap_or("separator").to_owned(),
                )
            })
            .collect()
    }

    fn menu_rig(cx: &mut TestAppContext, records: Vec<ConnRecord>) -> Rig {
        let total = records.len() as u32;
        let mut rig = rig(cx);
        rig.set_active(true);
        rig.publish_records(
            records,
            ConnCounts {
                total,
                tcp: total,
                ..ConnCounts::default()
            },
        );
        rig
    }

    #[gpui_kit::test]
    fn conn_menu_order(cx: &mut TestAppContext) {
        let mut record = super::connection(77, SmallVec::from_slice(&[super::key(4321)]));
        record.unit = Some(UnitRef {
            manager: Manager::User,
            name: Arc::from("firefox.service"),
        });
        let mut rig = menu_rig(cx, vec![record]);
        let menu = rig.build_menu(&[77], Some(SysColumnId::Connections(ConnColumn::Local)));
        assert_eq!(
            menu_shape(&rig.menu_entries(&menu)),
            [
                (MenuEntryKind::Item, "Copy".into()),
                (MenuEntryKind::Item, "Copy row".into()),
                (MenuEntryKind::Item, "Copy rows as table".into()),
                (MenuEntryKind::Item, "Copy local address".into()),
                (MenuEntryKind::Item, "Copy foreign address".into()),
                (MenuEntryKind::Separator, "separator".into()),
                // Shown for a TCP row; disabled here because the rig has no tl-helper.
                (MenuEntryKind::Disabled, "Close TCP connection".into()),
                (MenuEntryKind::Separator, "separator".into()),
                (MenuEntryKind::Item, "Jump to process".into()),
                (MenuEntryKind::Item, "Go to service".into()),
                (MenuEntryKind::Item, "Properties".into()),
            ]
        );
    }

    #[gpui_kit::test]
    fn conn_menu_foreign_disabled_for_listener(cx: &mut TestAppContext) {
        let mut record = super::connection(1, SmallVec::new());
        record.state = ConnState::Listening;
        record.foreign = Arc::from("*:*");
        let mut rig = menu_rig(cx, vec![record]);
        let menu = rig.build_menu(&[1], None);
        let entries = rig.menu_entries(&menu);
        assert_eq!(entries[3].label.as_deref(), Some("Copy foreign address"));
        assert_eq!(entries[3].kind, MenuEntryKind::Disabled);
    }

    #[gpui_kit::test]
    fn conn_menu_jump_disabled_no_owner(cx: &mut TestAppContext) {
        let mut record = super::connection(1, SmallVec::new());
        record.state = ConnState::TimeWait;
        record.inode = 0;
        let mut rig = menu_rig(cx, vec![record]);
        let menu = rig.build_menu(&[1], None);
        let entries = rig.menu_entries(&menu);
        let jump = entries
            .iter()
            .find(|entry| entry.label.as_deref() == Some("Jump to process"))
            .unwrap();
        assert_eq!(jump.kind, MenuEntryKind::Disabled);
        assert_eq!(
            jump.tooltip.as_deref(),
            Some("No owning process for this connection")
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.label.as_deref() != Some("Go to service"))
        );
    }

    #[gpui_kit::test]
    fn conn_menu_jump_disabled_restricted(cx: &mut TestAppContext) {
        let mut record = super::connection(1, SmallVec::new());
        record.owner_validity = Validity::Restricted;
        let mut rig = menu_rig(cx, vec![record]);
        let menu = rig.build_menu(&[1], None);
        let entries = rig.menu_entries(&menu);
        let jump = entries
            .iter()
            .find(|entry| entry.label.as_deref() == Some("Jump to process"))
            .unwrap();
        assert_eq!(jump.kind, MenuEntryKind::Disabled);
        assert_eq!(jump.tooltip.as_deref(), Some("Needs administrator access."));
    }

    #[gpui_kit::test]
    fn conn_menu_multi_select_rules(cx: &mut TestAppContext) {
        let records = [1, 2]
            .map(|cookie| {
                let mut record =
                    super::connection(cookie, SmallVec::from_slice(&[super::key(cookie as u32)]));
                record.unit = Some(UnitRef {
                    manager: Manager::User,
                    name: Arc::from("example.service"),
                });
                record
            })
            .to_vec();
        let mut rig = menu_rig(cx, records);
        let menu = rig.build_menu(&[1, 2], None);
        let entries = rig.menu_entries(&menu);
        for label in [
            "Copy local address",
            "Copy foreign address",
            "Jump to process",
        ] {
            assert_eq!(
                entries
                    .iter()
                    .find(|entry| entry.label.as_deref() == Some(label))
                    .unwrap()
                    .kind,
                MenuEntryKind::Disabled
            );
        }
        assert!(
            entries
                .iter()
                .all(|entry| entry.label.as_deref() != Some("Go to service"))
        );
    }

    #[gpui_kit::test]
    fn conn_menu_targets_captured_at_open(cx: &mut TestAppContext) {
        let mut rig = menu_rig(cx, vec![super::connection(77, SmallVec::new())]);
        let _menu = rig.build_menu(&[77], Some(SysColumnId::Connections(ConnColumn::Local)));
        rig.publish(&[88, 77], ConnCounts::default());
        assert_eq!(
            rig.vcx
                .update(|_, cx| rig.page.read(cx).menu_target_cookies_for_test().to_vec()),
            [77]
        );
    }

    /// tl-ui has no P3 build flag, so the item's hidden case is a UDP row: one separator
    /// between the copy items and Jump to process (UI-SYS-063).
    #[gpui_kit::test]
    fn conn_menu_close_hidden_for_udp(cx: &mut TestAppContext) {
        let mut record = super::connection(1, SmallVec::new());
        record.proto = Proto::Udp;
        record.state = ConnState::Unconnected;
        let mut rig = menu_rig(cx, vec![record]);
        let menu = rig.build_menu(&[1], None);
        let shape = menu_shape(&rig.menu_entries(&menu));
        assert!(
            shape
                .iter()
                .all(|(_, label)| label != "Close TCP connection")
        );
        assert_eq!(
            shape
                .iter()
                .filter(|(kind, _)| *kind == MenuEntryKind::Separator)
                .count(),
            1
        );
    }

    #[gpui_kit::test]
    fn conn_copy_block_golden(cx: &mut TestAppContext) {
        let mut record = super::connection(1, SmallVec::from_slice(&[super::key(4321)]));
        record.four_tuple = FourTuple {
            local: "192.168.1.10:51234".parse().unwrap(),
            remote: "140.82.112.4:443".parse().unwrap(),
        };
        record.local = Arc::from("192.168.1.10:51234");
        record.foreign = Arc::from("140.82.112.4:443");
        let mut rig = menu_rig(cx, vec![record]);
        rig.publish_records_and_processes(
            vec![{
                let mut record = super::connection(1, SmallVec::from_slice(&[super::key(4321)]));
                record.four_tuple = FourTuple {
                    local: "192.168.1.10:51234".parse().unwrap(),
                    remote: "140.82.112.4:443".parse().unwrap(),
                };
                record.local = Arc::from("192.168.1.10:51234");
                record.foreign = Arc::from("140.82.112.4:443");
                record
            }],
            vec![super::process(4321, "firefox", "alice", 1000)],
            ConnCounts::default(),
        );
        rig.select(&[1]);
        rig.dispatch(CopyDetails);
        let expected = include_str!("golden/conn_copy_block.txt").trim_end_matches('\n');
        assert_eq!(rig.clipboard().as_deref(), Some(expected));
        assert_eq!(
            rig.copy_toasts().last().map(String::as_str),
            Some("Connection copied")
        );
    }

    #[gpui_kit::test]
    fn conn_copy_ipv6_and_wildcards(cx: &mut TestAppContext) {
        let mut record = super::connection(1, SmallVec::new());
        record.family = Family::Inet6;
        record.local = Arc::from("[::1]:631");
        record.foreign = Arc::from("*:*");
        record.state = ConnState::Listening;
        record.inode = 0;
        let mut rig = menu_rig(cx, vec![record]);
        rig.select(&[1]);
        rig.dispatch(CopyDetails);
        let copied = rig.clipboard().unwrap();
        assert!(copied.contains("Local address: ::1\nLocal port: 631"));
        assert!(copied.contains("Foreign address: *\nForeign port: *"));
    }

    #[gpui_kit::test]
    fn conn_copy_shared_all_pids(cx: &mut TestAppContext) {
        let processes = [
            super::process(5000, "five", "alice", 1000),
            super::process(4321, "first", "alice", 1000),
            super::process(4400, "second", "alice", 1000),
        ];
        let owners = SmallVec::from_vec(processes.iter().map(|process| process.key).collect());
        let mut rig = menu_rig(cx, vec![super::connection(1, owners.clone())]);
        rig.publish_records_and_processes(
            vec![super::connection(1, owners)],
            processes.into(),
            ConnCounts::default(),
        );
        rig.select(&[1]);
        rig.dispatch(CopyDetails);
        assert!(rig.clipboard().unwrap().contains("PID: 4321, 4400, 5000"));
    }

    #[gpui_kit::test]
    fn conn_copy_kernel_and_restricted(cx: &mut TestAppContext) {
        let mut kernel = super::connection(1, SmallVec::new());
        kernel.inode = 0;
        let mut restricted = super::connection(2, SmallVec::new());
        restricted.owner_validity = Validity::Restricted;
        let mut rig = menu_rig(cx, vec![kernel, restricted]);
        rig.select(&[1, 2]);
        rig.dispatch(CopyDetails);
        let copied = rig.clipboard().unwrap();
        assert!(copied.contains("PID: -\nProcess: No owner (kernel)"));
        assert!(copied.contains("PID: ?\nProcess: Restricted"));
    }

    #[gpui_kit::test]
    fn conn_copy_nothing_selected(cx: &mut TestAppContext) {
        let mut rig = menu_rig(cx, vec![super::connection(1, SmallVec::new())]);
        rig.dispatch(CopyDetails);
        assert_eq!(
            rig.copy_toasts().last().map(String::as_str),
            Some("Select a connection row first.")
        );
    }

    #[gpui_kit::test]
    fn conn_copy_local_and_foreign_toasts(cx: &mut TestAppContext) {
        let mut record = super::connection(1, SmallVec::new());
        record.local = Arc::from("192.168.1.10:51234");
        let mut rig = menu_rig(cx, vec![record]);
        let _ = rig.build_menu(&[1], Some(SysColumnId::Connections(ConnColumn::Local)));
        rig.dispatch(ConnectionCopyLocal);
        assert_eq!(rig.clipboard().as_deref(), Some("192.168.1.10:51234"));
        assert_eq!(
            rig.copy_toasts().last().map(String::as_str),
            Some("Local address copied")
        );

        let _ = rig.build_menu(&[1], Some(SysColumnId::Connections(ConnColumn::Foreign)));
        rig.dispatch(ConnectionCopyForeign);
        assert_eq!(rig.clipboard().as_deref(), Some("10.0.0.1:443"));
        assert_eq!(
            rig.copy_toasts().last().map(String::as_str),
            Some("Foreign address copied")
        );
    }

    #[gpui_kit::test]
    fn conn_copy_tsv_glyph_empty(cx: &mut TestAppContext) {
        let mut record = super::connection(1, SmallVec::new());
        record.owner_validity = Validity::Restricted;
        let mut rig = menu_rig(cx, vec![record]);
        rig.select(&[1]);
        rig.dispatch(tl_ui::table::actions::CopyRowsAsTable);
        let copied = rig.clipboard().unwrap();
        assert_eq!(
            copied.lines().next(),
            Some("Proto\tAddress local\tAddress foreign\tState\tPID\tProcess")
        );
        assert_eq!(
            copied.lines().nth(1).unwrap().split('\t').next_back(),
            Some("")
        );
    }

    #[gpui_kit::test]
    fn connections_status_text(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        rig.publish(
            &[1, 2, 3],
            ConnCounts {
                total: 214,
                tcp: 168,
                udp: 46,
                established: 97,
                listening: 31,
            },
        );
        assert_eq!(
            rig.status(),
            "214 Total | 168 TCP | 46 UDP | 97 EST | 31 LISTEN"
        );
        let table = rig.table();
        table.update(&mut rig.vcx, |table, cx| {
            table.selection.click(1);
            table.selection.ctrl_click(2);
            table.selection.ctrl_click(3);
            cx.notify();
        });
        rig.vcx.run_until_parked();
        assert!(rig.status().ends_with(" | 3 selected"));

        rig.publish(
            &[1, 2, 3],
            ConnCounts {
                total: 12_345,
                tcp: 10_001,
                udp: 2_344,
                established: 9_999,
                listening: 31,
            },
        );
        assert!(
            rig.status()
                .starts_with("12\u{2009}345 Total | 10\u{2009}001 TCP")
        );
    }

    #[gpui_kit::test]
    fn connections_demand_and_first_visit_refresh(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        assert!(
            rig.engine
                .demands()
                .last()
                .is_some_and(|d| d.any_surface_visible && d.connections)
        );
        rig.set_active(false);
        assert_eq!(rig.engine.demands().last(), Some(&Demand::default()));
        rig.set_active(true);
        let refreshes = rig
            .engine
            .commands()
            .iter()
            .filter(|command| matches!(command, Command::RefreshNow(SourceId::Connections)))
            .count();
        assert_eq!(refreshes, 1);
    }

    #[gpui_kit::test]
    fn connections_data_arrives_from_demand_driven_engine(cx: &mut TestAppContext) {
        let inner = FakeEngine::new();
        let client: Arc<dyn EngineClient> = Arc::new(DemandDrivenEngine {
            inner: Arc::clone(&inner),
        });
        let mut rig = rig_with_client(cx, inner, client);
        assert_eq!(
            rig.vcx.read_entity(&rig.page, |page, _| page.body()),
            SysBody::Skeleton
        );
        rig.set_active(true);
        rig.draw();
        assert_eq!(
            rig.vcx.read_entity(&rig.page, |page, _| page.body()),
            SysBody::Rows
        );
        assert_eq!(rig.status(), "1 Total | 1 TCP | 0 UDP | 1 EST | 0 LISTEN");
    }

    #[gpui_kit::test]
    fn connections_selection_stable_across_refresh(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        let cookies = (1..=50).collect::<Vec<_>>();
        rig.publish(&cookies, ConnCounts::default());
        let table = rig.table();
        table.update(&mut rig.vcx, |table, cx| {
            assert!(table.jump_to(30, cx));
        });
        rig.draw();
        let before = rig
            .vcx
            .read_entity(&table, |table, _| table.first_visible_row().unwrap());

        let mut reordered = cookies.clone();
        reordered.swap(9, 10);
        reordered.swap(29, 30);
        rig.publish(&reordered, ConnCounts::default());
        let (selected, after, visible, selected_index) = rig.vcx.read_entity(&table, |table, _| {
            (
                table.selection.contains(&30),
                table.first_visible_row().unwrap(),
                table.visible_range(),
                table.delegate.row_index(30).unwrap(),
            )
        });
        assert!(selected);
        assert_eq!(after.0, before.0, "scroll anchor keeps the same cookie");
        assert!(visible.contains(&selected_index));
    }

    #[gpui_kit::test]
    fn connections_paused_on_ctrl(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        rig.publish(
            &[11],
            ConnCounts {
                total: 1,
                tcp: 1,
                ..ConnCounts::default()
            },
        );
        rig.set_paused(true);
        rig.publish(
            &[22],
            ConnCounts {
                total: 1,
                udp: 1,
                ..ConnCounts::default()
            },
        );
        let table = rig.table();
        assert_eq!(
            rig.vcx
                .read_entity(&table, |table, _| table.delegate.row_id(0)),
            11
        );
        assert_eq!(rig.status(), "1 Total | 1 TCP | 0 UDP | 0 EST | 0 LISTEN");
        rig.set_paused(false);
        assert_eq!(
            rig.vcx
                .read_entity(&table, |table, _| table.delegate.row_id(0)),
            22
        );
        assert_eq!(rig.status(), "1 Total | 0 TCP | 1 UDP | 0 EST | 0 LISTEN");
    }

    #[gpui_kit::test]
    fn connections_refresh_toast(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        assert!(
            rig.vcx
                .update(|window, cx| ToastQueue::of_window(window, cx).is_some())
        );
        rig.set_active(true);
        rig.publish(&[1], ConnCounts::default());
        rig.page.update(&mut rig.vcx, |page, cx| page.refresh(cx));
        rig.vcx.run_until_parked();
        rig.vcx
            .executor()
            .advance_clock(REFRESH_TOAST_AFTER + Duration::from_millis(1));
        rig.vcx.run_until_parked();
        assert_eq!(rig.refresh_toasts(), 1);
        rig.vcx.executor().advance_clock(Duration::from_secs(1));
        rig.vcx.run_until_parked();
        assert_eq!(rig.refresh_toasts(), 1, "fallback toast is one shot");

        rig.page.update(&mut rig.vcx, |page, cx| page.refresh(cx));
        rig.vcx.run_until_parked();
        rig.vcx.executor().advance_clock(Duration::from_millis(100));
        rig.publish(&[1], ConnCounts::default());
        assert_eq!(
            rig.refresh_toasts(),
            1,
            "new version replaces the old toast"
        );
        rig.vcx
            .executor()
            .advance_clock(REFRESH_TOAST_AFTER + Duration::from_millis(1));
        rig.vcx.run_until_parked();
        assert_eq!(rig.refresh_toasts(), 1, "new-version toast is one shot");
    }

    #[gpui_kit::test]
    fn connections_filter_matches_port(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.set_active(true);
        let before = rig.generation();
        rig.vcx.update(|_, cx| {
            let events = cx.global::<GlobalShellEvents>().0.clone();
            events.update(cx, |_, cx| {
                cx.emit(tl_ui::window::main::FilterChanged {
                    tab: Tab::Connections,
                    text: "443".into(),
                });
            });
        });
        rig.vcx.run_until_parked();
        let query = rig
            .engine
            .commands()
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetSysTableQuery(query) if query.table == SysTable::Connections => {
                    Some(query)
                }
                _ => None,
            })
            .expect("connections filter query sent");
        assert_eq!(query.filter.as_ref(), "443");
        assert!(query.generation > before);
    }

    /// Close TCP connection (tasklens-9o0.28, UI-SYS-064).
    mod close {
        use super::*;
        use tl_model::actions::{
            ActionError, ActionEvent as FlowEvent, ActionId, ActionKind, ActionRequest,
            ActionState, ActionTarget, RejectReason,
        };
        use tl_model::health::HelperState;
        use tl_ui::action_tracker::PushedToast;
        use tl_ui::components::dialog::{DialogButton, TlDialogView};
        use tl_ui::components::toast::ToastKind;
        use tl_ui::components::window_frame::TlSecondaryWindow;
        use tl_ui::pages::connections::actions::ConnectionCloseTcp;
        use tl_ui::pages::connections::listen_confirm_texts;

        const CLOSE: &str = "Close TCP connection";

        struct Dialog {
            window: gpui_kit::WindowHandle<Root>,
            view: Entity<TlDialogView>,
        }

        fn listening(cookie: u64, addr: &str) -> ConnRecord {
            let mut record = super::super::connection(cookie, SmallVec::new());
            record.state = ConnState::Listening;
            record.four_tuple = FourTuple {
                local: addr.parse().expect("a socket address"),
                remote: "0.0.0.0:0".parse().expect("a socket address"),
            };
            record.local = Arc::from(addr);
            record.foreign = Arc::from("*:*");
            record
        }

        fn established(cookie: u64, local: &str) -> ConnRecord {
            let mut record = super::super::connection(cookie, SmallVec::new());
            record.four_tuple.local = local.parse().expect("a socket address");
            record.local = Arc::from(local);
            record
        }

        impl Rig {
            fn set_helper(&mut self, helper: HelperState) {
                self.engine
                    .publish_with(|s| Arc::make_mut(&mut s.health).helper = helper);
                self.vcx.run_until_parked();
                self.draw();
            }

            fn set_destroy_supported(&mut self, supported: Option<bool>) {
                self.engine.publish_with(|s| {
                    let mut conns = ConnectionsSnapshot::clone(&s.connections);
                    conns.destroy_supported = supported;
                    s.connections = Arc::new(conns);
                });
                self.vcx.run_until_parked();
                self.draw();
            }

            fn close_entry(&mut self, targets: &[u64]) -> Option<MenuEntryInfo> {
                let menu = self.build_menu(targets, None);
                self.menu_entries(&menu)
                    .into_iter()
                    .find(|entry| entry.label.as_deref() == Some(CLOSE))
            }

            /// Opens the row menu on `targets` and chooses Close TCP connection.
            fn choose_close(&mut self, targets: &[u64]) {
                let _menu = self.build_menu(targets, None);
                self.dispatch(ConnectionCloseTcp);
            }

            fn requests(&self) -> Vec<ActionRequest> {
                self.engine
                    .commands()
                    .into_iter()
                    .filter_map(|c| match c {
                        Command::Action(r) => Some(r),
                        _ => None,
                    })
                    .collect()
            }

            fn emit(&mut self, id: ActionId, target: Option<u16>, state: ActionState) {
                self.engine.emit(FlowEvent { id, target, state });
                self.vcx.run_until_parked();
            }

            fn finish(&mut self, id: ActionId, states: &[ActionState]) {
                self.emit(id, None, ActionState::Started);
                for (i, state) in states.iter().enumerate() {
                    self.emit(id, Some(i as u16), state.clone());
                }
                self.emit(id, None, ActionState::summarize(states));
                self.draw();
            }

            fn pushed(&mut self) -> Vec<PushedToast> {
                let actions = self
                    .vcx
                    .read_entity(&self.page, |page, _| page.actions_for_test().clone());
                self.vcx.read_entity(&actions, |a, _| a.pushed().to_vec())
            }

            fn state_cell(&mut self, ix: usize) -> String {
                let table = self.table();
                self.vcx.read_entity(&table, |table, _| {
                    let mut out = CellOut::default();
                    table
                        .delegate
                        .cell(ix, SysColumnId::Connections(ConnColumn::State), &mut out);
                    super::super::text(&out).to_owned()
                })
            }

            fn page_window_id(&mut self) -> gpui_kit::WindowId {
                gpui_kit::VisualContext::window_handle(&self.vcx).window_id()
            }

            fn dialog(&mut self) -> Option<Dialog> {
                let parent = self.page_window_id();
                let window = self
                    .vcx
                    .update(|_, cx| cx.windows().into_iter().find(|w| w.window_id() != parent))?
                    .downcast::<Root>()?;
                let frame = self
                    .vcx
                    .update(|_, cx| TlSecondaryWindow::<TlDialogView>::find(window, cx))?;
                let view = self.vcx.read(|cx| frame.read(cx).content().clone());
                Some(Dialog { window, view })
            }

            fn is_open(&mut self, d: &Dialog) -> bool {
                let id = d.window.window_id();
                self.vcx
                    .update(|_, cx| cx.windows().iter().any(|w| w.window_id() == id))
            }
        }

        fn close_rig(cx: &mut TestAppContext, records: Vec<ConnRecord>) -> Rig {
            let mut rig = menu_rig(cx, records);
            rig.set_helper(HelperState::Unlocked);
            rig
        }

        fn socket(record: &ConnRecord) -> ActionTarget {
            ActionTarget::Socket {
                cookie: record.cookie,
                family: record.family,
                proto: record.proto,
                four_tuple: record.four_tuple,
            }
        }

        fn press(cx: &mut TestAppContext, dialog: &Dialog, keys: &str) {
            let mut vcx = VisualTestContext::from_window(dialog.window.into(), cx);
            vcx.update(|window, _| window.activate_window());
            vcx.simulate_keystrokes(keys);
            vcx.run_until_parked();
        }

        /// All five rows of the UI-SYS-064 item table, first match wins.
        #[gpui_kit::test]
        fn conn_close_menu_states(cx: &mut TestAppContext) {
            let mut udp = super::super::connection(2, SmallVec::new());
            udp.proto = Proto::Udp;
            udp.state = ConnState::Unconnected;
            let mut time_wait = super::super::connection(3, SmallVec::new());
            time_wait.state = ConnState::TimeWait;
            let tcp = super::super::connection(1, SmallVec::new());
            let mut rig = menu_rig(cx, vec![tcp, udp, time_wait]);

            // Every target UDP or TIME-WAIT: hidden.
            assert!(rig.close_entry(&[2]).is_none());
            assert!(rig.close_entry(&[3]).is_none());
            assert!(rig.close_entry(&[2, 3]).is_none());

            // No tl-helper: disabled. A mixed selection still shows the item.
            let entry = rig.close_entry(&[1, 2]).expect("shown for a TCP target");
            assert_eq!(entry.kind, MenuEntryKind::Disabled);
            assert_eq!(entry.tooltip.as_deref(), Some("Needs the tasklens helper"));

            // The kernel lacks destroy: disabled, even with the helper installed.
            rig.set_helper(HelperState::Locked);
            rig.set_destroy_supported(Some(false));
            let entry = rig.close_entry(&[1]).unwrap();
            assert_eq!(entry.kind, MenuEntryKind::Disabled);
            assert_eq!(
                entry.tooltip.as_deref(),
                Some("Kernel lacks CONFIG_INET_DIAG_DESTROY")
            );

            // Enabled.
            rig.set_destroy_supported(None);
            let entry = rig.close_entry(&[1]).unwrap();
            assert_eq!(entry.kind, MenuEntryKind::Item);

            // A close pending on every closable target: disabled with "Closing…".
            rig.choose_close(&[1]);
            assert_eq!(rig.requests().len(), 1);
            let entry = rig.close_entry(&[1, 3]).unwrap();
            assert_eq!(entry.kind, MenuEntryKind::Disabled);
            assert_eq!(entry.tooltip.as_deref(), Some("Closing…"));
        }

        #[gpui_kit::test]
        fn close_listening_requires_confirm(cx: &mut TestAppContext) {
            let mut rig = close_rig(cx, vec![listening(1, "0.0.0.0:22")]);
            rig.choose_close(&[1]);
            let dialog = rig.dialog().expect("the confirmation opens");
            assert!(
                rig.requests().is_empty(),
                "nothing is sent before confirming"
            );
            let (title, text, destructive, cancel) = rig.vcx.read(|cx| {
                let d = dialog.view.read(cx).dialog();
                (
                    d.window_title().to_string(),
                    d.text().to_string(),
                    d.is_destructive(),
                    d.cancel_text().to_string(),
                )
            });
            assert_eq!(title, "tasklens");
            assert_eq!(
                text,
                "The program will stop accepting new connections on 0.0.0.0:22."
            );
            assert!(destructive, "\"Close socket\" is a Danger button");
            assert_eq!(cancel, "Cancel");
            let addresses = [SharedString::from("0.0.0.0:22")];
            let texts = rig.vcx.read(|cx| listen_confirm_texts(&addresses, cx));
            assert_eq!(texts.heading, "Close listening socket?");

            let view = dialog.view.clone();
            let focused = {
                let mut vcx = VisualTestContext::from_window(dialog.window.into(), cx);
                vcx.update(|window, cx| view.read(cx).focused_button(window))
            };
            assert_eq!(focused, Some(DialogButton::Cancel));
            // A stray Enter cancels and sends nothing.
            press(cx, &dialog, "enter");
            assert!(!rig.is_open(&dialog));
            assert!(rig.requests().is_empty());

            // Tab then Enter closes the socket.
            rig.choose_close(&[1]);
            let dialog = rig.dialog().expect("the confirmation opens again");
            press(cx, &dialog, "tab enter");
            assert!(!rig.is_open(&dialog));
            let sent = rig.requests();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].kind, ActionKind::SockDestroy);
            assert_eq!(
                sent[0].targets.as_slice(),
                [socket(&listening(1, "0.0.0.0:22"))]
            );
        }

        #[gpui_kit::test]
        fn close_listening_plural_heading(cx: &mut TestAppContext) {
            let addresses: Vec<SharedString> = (0..7)
                .map(|i| SharedString::from(format!("0.0.0.0:{}", 8000 + i)))
                .collect();
            let mut rig = close_rig(cx, vec![listening(1, "0.0.0.0:22")]);
            let texts = rig.vcx.read(|cx| listen_confirm_texts(&addresses, cx));
            assert_eq!(texts.heading, "Close 7 listening sockets?");
            assert_eq!(texts.addresses, addresses[..5]);
            assert_eq!(texts.more.as_deref(), Some("and 2 more"));

            let two = &addresses[..2];
            let texts = rig.vcx.read(|cx| listen_confirm_texts(two, cx));
            assert_eq!(texts.heading, "Close 2 listening sockets?");
            assert_eq!(texts.addresses, two);
            assert!(texts.more.is_none());

            // Through the page: two listeners and one established socket, one confirmation.
            let records = vec![
                listening(1, "0.0.0.0:22"),
                listening(2, "[::]:80"),
                established(3, "127.0.0.1:5000"),
            ];
            rig.publish_records(records, ConnCounts::default());
            rig.choose_close(&[1, 2, 3]);
            let dialog = rig.dialog().expect("the confirmation opens");
            press(cx, &dialog, "tab enter");
            let sent = rig.requests();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].targets.len(), 3);
        }

        #[gpui_kit::test]
        fn close_toast_sequence(cx: &mut TestAppContext) {
            let mut rig = close_rig(cx, vec![super::super::connection(1, SmallVec::new())]);
            assert_eq!(rig.state_cell(0), "ESTABLISHED");
            rig.choose_close(&[1]);
            assert!(
                rig.dialog().is_none(),
                "no confirmation for an established socket"
            );
            let sent = rig.requests();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].kind, ActionKind::SockDestroy);
            assert_eq!(rig.pushed().len(), 1);
            let pending = rig.pushed()[0].clone();
            assert_eq!(pending.kind, ToastKind::Progress);
            assert_eq!(pending.text, "TCP connection close requested");
            assert_eq!(rig.state_cell(0), "Closing…");

            rig.finish(sent[0].id, &[ActionState::Succeeded]);
            let done = rig.pushed().last().cloned().unwrap();
            assert_eq!(done.kind, ToastKind::Success);
            assert_eq!(done.text, "TCP connection closed");
            assert_eq!(
                done.key, pending.key,
                "the result replaces the pending toast"
            );
            let shown = rig.copy_toasts();
            assert!(shown.iter().any(|t| t == "TCP connection closed"));
            assert!(!shown.iter().any(|t| t == "TCP connection close requested"));
            // The row keeps "Closing…" until it leaves the snapshot.
            assert_eq!(rig.state_cell(0), "Closing…");
            rig.publish(&[2], ConnCounts::default());
            rig.publish(&[1], ConnCounts::default());
            assert_eq!(rig.state_cell(0), "ESTABLISHED");
        }

        #[gpui_kit::test]
        fn close_eopnotsupp_disables_item(cx: &mut TestAppContext) {
            let mut rig = close_rig(cx, vec![super::super::connection(1, SmallVec::new())]);
            rig.choose_close(&[1]);
            let sent = rig.requests();
            // The engine answers EOPNOTSUPP from the helper with Rejected(Unsupported).
            rig.finish(
                sent[0].id,
                &[ActionState::Rejected(RejectReason::Unsupported)],
            );
            let toast = rig.pushed().last().cloned().unwrap();
            assert_eq!(toast.kind, ToastKind::Error);
            assert!(
                toast.text.starts_with("Failed to close TCP connection: "),
                "{}",
                toast.text
            );
            // A failure drops "Closing…".
            assert_eq!(rig.state_cell(0), "ESTABLISHED");
            rig.set_destroy_supported(Some(false));
            let entry = rig.close_entry(&[1]).unwrap();
            assert_eq!(entry.kind, MenuEntryKind::Disabled);
            assert_eq!(
                entry.tooltip.as_deref(),
                Some("Kernel lacks CONFIG_INET_DIAG_DESTROY")
            );
        }

        /// Security regression: the request carries the cookie and four-tuple captured at menu
        /// open. A newer snapshot with the same four-tuple and a new cookie changes nothing.
        #[gpui_kit::test]
        fn close_targets_exact_cookie_and_tuple(cx: &mut TestAppContext) {
            let old = established(77, "192.168.1.10:51234");
            let mut rig = close_rig(cx, vec![old.clone()]);
            let _menu = rig.build_menu(&[77], None);
            let reused = established(88, "192.168.1.10:51234");
            rig.publish_records(vec![reused], ConnCounts::default());
            rig.dispatch(ConnectionCloseTcp);
            let sent = rig.requests();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].kind, ActionKind::SockDestroy);
            assert_eq!(sent[0].targets.as_slice(), [socket(&old)]);
        }

        #[gpui_kit::test]
        fn close_auth_cancelled_toast(cx: &mut TestAppContext) {
            let mut rig = close_rig(cx, vec![super::super::connection(1, SmallVec::new())]);
            rig.set_helper(HelperState::Locked);
            rig.choose_close(&[1]);
            let sent = rig.requests();
            assert_eq!(sent.len(), 1);
            // The polkit prompt is up: the toast stays pending.
            rig.emit(sent[0].id, None, ActionState::Started);
            let pending = rig.pushed().last().cloned().unwrap();
            assert_eq!(pending.kind, ToastKind::Progress);
            assert_eq!(pending.text, "TCP connection close requested");
            rig.finish(
                sent[0].id,
                &[ActionState::Failed(ActionError::AuthCancelled)],
            );
            let toast = rig.pushed().last().cloned().unwrap();
            assert_eq!(toast.kind, ToastKind::Warning);
            assert_eq!(toast.text, "Authentication cancelled");
            assert_eq!(toast.key, pending.key);
            assert_eq!(rig.state_cell(0), "ESTABLISHED");
        }
    }

    /// The Namespaces control (tasklens-9o0.31, UI-SYS-070).
    mod netns {
        use super::*;
        use tl_model::health::{HelperStartOutcome, HelperState};
        use tl_ui::pages::connections::actions::{ConnectionNetnsAll, ConnectionNetnsHost};
        use tl_ui::window::main::HelperInstalled;

        fn installed_rig(cx: &mut TestAppContext, installed: bool) -> Rig {
            let mut rig = menu_rig(cx, vec![super::super::connection(1, SmallVec::new())]);
            rig.vcx
                .update(|_, cx| cx.set_global(HelperInstalled(installed)));
            rig
        }

        fn set_helper(rig: &mut Rig, helper: HelperState) {
            rig.engine
                .publish_with(|s| Arc::make_mut(&mut s.health).helper = helper);
            rig.vcx.run_until_parked();
            rig.draw();
        }

        fn label(rig: &mut Rig) -> String {
            rig.vcx
                .update(|_, cx| rig.page.read(cx).netns_label(cx).to_string())
        }

        fn visible(rig: &mut Rig) -> Vec<ConnColumn> {
            let table = rig.table();
            rig.vcx.read_entity(&table, |table, _| {
                table
                    .delegate
                    .columns()
                    .visible_columns()
                    .iter()
                    .filter_map(|c| match c {
                        SysColumnId::Connections(column) => Some(*column),
                        _ => None,
                    })
                    .collect()
            })
        }

        fn netns_demand(rig: &mut Rig) -> bool {
            rig.vcx.executor().advance_clock(DEMAND_COALESCE * 2);
            rig.vcx.run_until_parked();
            rig.engine
                .demands()
                .last()
                .is_some_and(|d| d.connections_all_netns)
        }

        fn starts(rig: &Rig) -> usize {
            rig.engine
                .commands()
                .iter()
                .filter(|c| matches!(c, Command::StartHelperSession))
                .count()
        }

        fn netns_menu(rig: &mut Rig) -> Vec<MenuEntryInfo> {
            let page = rig.page.clone();
            let menu = rig
                .vcx
                .update(|window, cx| ConnectionsPage::build_netns_menu(&page, window, cx));
            rig.menu_entries(&menu)
        }

        #[gpui_kit::test]
        fn netns_all_disabled_without_helper(cx: &mut TestAppContext) {
            let mut rig = installed_rig(cx, false);
            let entries = netns_menu(&mut rig);
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].label.as_deref(), Some("Host only"));
            assert!(entries[0].checked);
            assert_eq!(entries[1].label.as_deref(), Some("All"));
            assert_eq!(entries[1].kind, MenuEntryKind::Disabled);
            assert_eq!(
                entries[1].tooltip.as_deref(),
                Some("Needs the tasklens helper")
            );
            rig.dispatch(ConnectionNetnsAll);
            assert_eq!(label(&mut rig), "Namespaces: Host only");
            assert!(!netns_demand(&mut rig));
        }

        #[gpui_kit::test]
        fn netns_all_adds_column(cx: &mut TestAppContext) {
            let mut rig = installed_rig(cx, true);
            set_helper(&mut rig, HelperState::Unlocked);
            assert_eq!(label(&mut rig), "Namespaces: Host only");
            assert!(!visible(&mut rig).contains(&ConnColumn::Netns));
            rig.dispatch(ConnectionNetnsAll);
            assert_eq!(label(&mut rig), "Namespaces: All");
            assert!(netns_demand(&mut rig));
            let columns = visible(&mut rig);
            assert_eq!(columns[0], ConnColumn::Proto);
            assert_eq!(columns[1], ConnColumn::Netns, "right after Proto");
            assert_eq!(starts(&rig), 0, "an unlocked helper needs no new session");
            let entries = netns_menu(&mut rig);
            assert!(!entries[0].checked);
            assert_eq!(entries[1].kind, MenuEntryKind::Item);
            assert!(entries[1].checked);
        }

        #[gpui_kit::test]
        fn netns_host_only_hides_column(cx: &mut TestAppContext) {
            let mut rig = installed_rig(cx, true);
            set_helper(&mut rig, HelperState::Unlocked);
            rig.dispatch(ConnectionNetnsAll);
            assert!(netns_demand(&mut rig));
            rig.dispatch(ConnectionNetnsHost);
            assert_eq!(label(&mut rig), "Namespaces: Host only");
            assert!(!visible(&mut rig).contains(&ConnColumn::Netns));
            assert!(!netns_demand(&mut rig));
        }

        #[gpui_kit::test]
        fn netns_auth_cancelled_keeps_host_only(cx: &mut TestAppContext) {
            let mut rig = installed_rig(cx, true);
            set_helper(&mut rig, HelperState::Locked);
            rig.dispatch(ConnectionNetnsAll);
            assert_eq!(starts(&rig), 1, "a locked helper starts a session");
            assert_eq!(label(&mut rig), "Namespaces: All", "while the prompt is up");
            rig.engine.publish_with(|s| {
                let health = Arc::make_mut(&mut s.health);
                health.helper_start = Some((1, HelperStartOutcome::Cancelled));
                health.version += 1;
            });
            rig.vcx.run_until_parked();
            rig.draw();
            assert_eq!(label(&mut rig), "Namespaces: Host only");
            assert!(!visible(&mut rig).contains(&ConnColumn::Netns));
            assert!(!netns_demand(&mut rig));
            // The page pushes no toast of its own: the helper flow owns "Authentication cancelled".
            assert_eq!(rig.refresh_toasts(), 0);
        }

        #[gpui_kit::test]
        fn netns_session_only(cx: &mut TestAppContext) {
            let mut rig = installed_rig(cx, true);
            set_helper(&mut rig, HelperState::Unlocked);
            rig.dispatch(ConnectionNetnsAll);
            assert!(rig.vcx.read_entity(&rig.page, |p, _| p.netns_all()));
            // A restart builds a new page: no settings key carries the choice over.
            let mut again = installed_rig(cx, true);
            assert_eq!(label(&mut again), "Namespaces: Host only");
            assert!(!visible(&mut again).contains(&ConnColumn::Netns));
        }
    }
}

/// `TasklensUi::init` binds the Connections keys and the shell builds `ConnectionsPage` for its
/// tab (tasklens-9o0.20).
#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn connections_page_registered(cx: &mut TestAppContext) {
    use gpui_kit::{AppContext as _, VisualTestContext};
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{Settings, StartTab, UiState};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::pages::connections::ConnectionsPage;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window};

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
#[cfg(feature = "test-hooks")]
mod jump {

    use super::*;
    use gpui_kit::{AppContext as _, VisualTestContext};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::ids::NodeId;
    use tl_model::settings::{Settings, UiState};
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
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};

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

/// The Connection Properties window (tasklens-9o0.19).
#[cfg(feature = "test-hooks")]
mod props {
    use super::*;
    use gpui_kit::base::Root;
    use gpui_kit::{VisualTestContext, WindowHandle};
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::health::HelperState;
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::{ConnCounts, ProcessSnapshot, TcpInfoView};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::window_frame::{TestClientFrame, TlSecondaryWindow, frame_bottom};
    use tl_ui::demand::GlobalDemand;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui::window::registry::{CapClass, GlobalRegistry, WindowKey};
    use tl_ui::windows::connection_properties::{
        ConnectionPropertiesView, open_connection_properties,
    };

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

        fn set_helper(&mut self, helper: HelperState) {
            self.engine.publish_with(|s| {
                let health = Arc::make_mut(&mut s.health);
                health.helper = helper;
                health.version += 1;
            });
            self.main.run_until_parked();
        }

        fn set_destroy_supported(&mut self, supported: Option<bool>) {
            self.engine.publish_with(|s| {
                let mut conns = ConnectionsSnapshot::clone(&s.connections);
                conns.destroy_supported = supported;
                conns.header.version += 1;
                s.connections = Arc::new(conns);
            });
            self.main.run_until_parked();
        }

        fn toasts(&mut self) -> Vec<String> {
            self.main.update(|_, cx| {
                let view = main_window_view(cx).expect("main window");
                let texts = view.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(ToString::to_string).collect()
            })
        }
    }

    fn open(cx: &mut TestAppContext, cookie: u64) -> Option<WindowHandle<Root>> {
        cx.update(|cx| open_connection_properties(cookie, cx));
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

    fn title(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> String {
        let frame = frame(cx, handle);
        cx.update(|cx| frame.read(cx).title().to_string())
    }

    fn lines(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> Vec<String> {
        let frame = frame(cx, handle);
        cx.update(|cx| {
            let view = frame.read(cx).content().clone();
            let words = tl_ui::components::kv_grid::kv_words(cx);
            view.read(cx).line_texts(&words)
        })
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

    fn golden(name: &str, actual: &str) {
        let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        if std::env::var("TL_UPDATE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&path, actual).expect("write the golden");
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(actual, expected, "{name}");
    }

    #[gpui_kit::test]
    fn conn_props_lines_golden(cx: &mut TestAppContext) {
        let _rig = rig(cx, vec![tcp_record(1)], procs());
        let handle = open(cx, 1).expect("opened");
        assert_eq!(title(cx, handle), "Connection properties: TCP ESTABLISHED");
        let text = lines(cx, handle).join("\n") + "\n";
        golden("conn_props_lines.txt", &text);
    }

    /// Inside the 21 px client frame on each edge (the test hook lays it out as under KWin with
    /// client decorations) the window is the 560 x 520 spec (UI-SYS-068), the last line "Closable
    /// here" shows above the footer without scrolling, and the footer ends inside the window
    /// (tasklens-fut.23).
    #[gpui_kit::test]
    fn conn_props_last_line_shows_inside_the_frame(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(TestClientFrame));
        let _rig = rig(cx, vec![tcp_record(1)], procs());
        let handle = open(cx, 1).expect("opened");
        let texts = lines(cx, handle);
        assert!(
            texts.last().is_some_and(|l| l.starts_with("Closable here")),
            "{texts:?}"
        );
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let inset = vcx.update(|window, cx| frame_bottom(window, cx));
        #[cfg(target_os = "linux")]
        assert_eq!(inset, gpui_kit::px(21.), "the client frame hook is on");
        let window = vcx.update(|window, _| window.bounds());
        let last = format!("TlKv-row-{}", texts.len() - 1);
        let last = vcx.debug_bounds(last.leak()).expect("the last row");
        let footer = vcx.debug_bounds("ConnPropsFooter").expect("footer");
        assert!(last.bottom() <= footer.top(), "{last:?} above {footer:?}");
        assert!(
            footer.bottom() <= window.size.height - inset,
            "{footer:?} in {window:?}"
        );
        assert_eq!(
            window.size,
            gpui_kit::size(
                gpui_kit::px(560.) + inset * 2.,
                gpui_kit::px(520.) + inset * 2.
            )
        );
    }

    fn closable(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> String {
        lines(cx, handle)
            .into_iter()
            .find(|l| l.starts_with("Closable here"))
            .expect("a Closable here line")
    }

    #[gpui_kit::test]
    fn conn_props_closable_reasons(cx: &mut TestAppContext) {
        let mut udp = tcp_record(1);
        udp.proto = Proto::Udp;
        udp.state = ConnState::Unconnected;
        udp.tcp = None;
        let mut time_wait = tcp_record(2);
        time_wait.state = ConnState::TimeWait;
        let mut rig = rig(cx, vec![udp, time_wait, tcp_record(3)], procs());
        // No helper: the UDP and TIME-WAIT reasons still win.
        let handle = open(cx, 1).expect("opened");
        assert_eq!(closable(cx, handle), "Closable here: No (UDP)");
        let handle = open(cx, 2).expect("opened");
        assert_eq!(closable(cx, handle), "Closable here: No (TIME-WAIT)");
        let handle = open(cx, 3).expect("opened");
        assert_eq!(
            closable(cx, handle),
            "Closable here: No (needs the tasklens helper)"
        );
        rig.set_helper(HelperState::Unlocked);
        rig.set_destroy_supported(Some(false));
        assert_eq!(
            closable(cx, handle),
            "Closable here: No (kernel lacks CONFIG_INET_DIAG_DESTROY)"
        );
        rig.set_destroy_supported(None);
        assert_eq!(closable(cx, handle), "Closable here: Yes");
    }

    #[gpui_kit::test]
    fn conn_props_tcp_lines_only_for_tcp(cx: &mut TestAppContext) {
        let mut udp = tcp_record(1);
        udp.proto = Proto::Udp;
        udp.state = ConnState::Unconnected;
        udp.foreign = Arc::from("*:*");
        udp.tcp = None;
        let _rig = rig(cx, vec![udp], procs());
        let handle = open(cx, 1).expect("opened");
        let all = lines(cx, handle);
        for label in [
            "RTT",
            "Congestion window",
            "Bytes sent / received",
            "Retransmits",
            "Last data received",
        ] {
            assert!(
                !all.iter().any(|l| l.starts_with(label)),
                "{label} in {all:?}"
            );
        }
        assert!(all.iter().any(|l| l == "Foreign: *"), "{all:?}");
    }

    #[gpui_kit::test]
    fn conn_props_tcp_value_not_reported(cx: &mut TestAppContext) {
        let mut record = tcp_record(1);
        record.tcp = None;
        let _rig = rig(cx, vec![record], procs());
        let handle = open(cx, 1).expect("opened");
        assert!(
            lines(cx, handle).iter().any(|l| l == "RTT: Not reported"),
            "{:?}",
            lines(cx, handle)
        );
    }

    #[gpui_kit::test]
    fn conn_props_closed_title(cx: &mut TestAppContext) {
        let mut rig = rig(cx, vec![tcp_record(1)], procs());
        let handle = open(cx, 1).expect("opened");
        let before = lines(cx, handle);
        rig.publish(Vec::new(), procs());
        cx.run_until_parked();
        assert_eq!(
            title(cx, handle),
            "Connection properties: TCP ESTABLISHED (closed)"
        );
        assert_eq!(lines(cx, handle), before, "the last values stay");
        let frame = frame(cx, handle);
        let secondary = cx.update(|cx| {
            let view = frame.read(cx).content().clone();
            let words = tl_ui::components::kv_grid::kv_words(cx);
            let theme = current(cx).clone();
            view.read(cx)
                .value_looks(&words, &theme)
                .iter()
                .all(|look| look.color == theme.text_secondary && !look.link)
        });
        assert!(secondary, "every value is text_secondary");
    }

    #[gpui_kit::test]
    fn conn_props_cap(cx: &mut TestAppContext) {
        let records: Vec<ConnRecord> = (1..=11).map(tcp_record).collect();
        let mut rig = rig(cx, records, procs());
        for cookie in 1..=10 {
            assert!(open(cx, cookie).is_some());
        }
        rig.main.update(|window, _| window.activate_window());
        rig.main.run_until_parked();
        assert!(open(cx, 11).is_none());
        let count = cx.update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .count(CapClass::ConnectionProps)
        });
        assert_eq!(count, 10);
        assert!(
            rig.toasts()
                .iter()
                .any(|t| t == "Too many connection properties windows are already open"),
            "{:?}",
            rig.toasts()
        );
    }

    #[gpui_kit::test]
    fn conn_props_same_cookie_focuses(cx: &mut TestAppContext) {
        let _rig = rig(cx, vec![tcp_record(1)], procs());
        let first = open(cx, 1).expect("opened");
        let again = open(cx, 1).expect("still open");
        assert_eq!(first.window_id(), again.window_id());
        let count = cx.update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .count(CapClass::ConnectionProps)
        });
        assert_eq!(count, 1);
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

/// The Connections tab inside `MainWindow`, end to end through the fake engine
/// (tasklens-9o0.2, UI-SYS-060 to UI-SYS-068, table (b)).
#[cfg(feature = "test-hooks")]
mod main_window {
    use super::*;
    use gpui_kit::AppContext as _;
    use gpui_kit::{Entity, Modifiers, MouseButton, MouseDownEvent, VisualTestContext, point};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::ids::NodeId;
    use tl_model::settings::{Settings, StartTab, UiState};
    use tl_model::snapshot::ConnCounts;
    use tl_model::table::{ProcessTableView, SysTable, TableRow};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::menu::{MenuEntryInfo, TlMenuPopup, open_context_menu};
    use tl_ui::demand::DEMAND_COALESCE;
    use tl_ui::pages::connections::ConnectionsPage;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{MainWindow, main_window_view, open_main_window};
    use tl_ui::window::registry::{CapClass, GlobalRegistry, WindowKey};

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

        fn select_row(&mut self, cookie: u64) {
            let table = self.table();
            table.update(&mut self.vcx, |table, cx| {
                table.selection.click(cookie);
                cx.notify();
            });
            self.vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
            });
            self.draw();
        }

        fn selected(&mut self) -> Vec<u64> {
            let table = self.table();
            self.vcx
                .read_entity(&table, |table, _| table.selection.ids().collect())
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
    fn connections_end_to_end_refresh_keeps_selection(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.visit_with_fixture();
        rig.select_row(FIREFOX_COOKIE);
        assert_eq!(rig.selected(), [FIREFOX_COOKIE]);
        for _ in 0..3 {
            rig.vcx
                .executor()
                .advance_clock(std::time::Duration::from_secs(2));
            rig.publish();
            assert_eq!(rig.selected(), [FIREFOX_COOKIE], "the selection stays");
        }
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
    fn connections_enter_opens_properties(cx: &mut TestAppContext) {
        let mut rig = boot(cx);
        rig.visit_with_fixture();
        rig.select_row(FIREFOX_COOKIE);
        rig.vcx.simulate_keystrokes("enter");
        rig.vcx.run_until_parked();
        let (opened, count) = rig.vcx.update(|_, cx| {
            let registry = cx.global::<GlobalRegistry>().0.read(cx);
            (
                registry
                    .get(&WindowKey::ConnectionProperties(FIREFOX_COOKIE))
                    .is_some(),
                registry.count(CapClass::ConnectionProps),
            )
        });
        assert!(opened, "the Properties window is registered for the cookie");
        assert_eq!(count, 1);
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

/// The Remote host column (tasklens-9o0.30, UI-SYS-069).
mod remote_host {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::base::Root;
    use gpui_kit::{AppContext as _, Entity};
    use tl_model::client::EngineClient;
    use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator, GlobalDemand};
    use tl_ui::hub::SnapshotHub;
    use tl_ui::table::columns::Availability;
    use tl_ui::table::columns_window::PickerColumn;
    use tl_ui::table::tree_table::{TlTreeTable, demand_window};
    use tl_ui::testing::fake_engine::FakeEngine;

    fn foreign_record(cookie: u64, host: RemoteHost) -> ConnRecord {
        let mut record = connection(cookie, SmallVec::new());
        record.remote_host = host;
        record
    }

    fn cookies(cx: &mut TestAppContext) -> Vec<u64> {
        cx.executor().advance_clock(DEMAND_COALESCE * 2);
        cx.run_until_parked();
        cx.update(|cx| {
            cx.global::<GlobalDemand>()
                .0
                .read(cx)
                .merged()
                .remote_host_cookies
                .to_vec()
        })
    }

    /// A table over `count` rows with the page's demand wired and the tab active.
    fn rig(cx: &mut TestAppContext, count: u64) -> Entity<TlTreeTable<ConnectionsDelegate>> {
        let records = (1..=count)
            .map(|cookie| foreign_record(cookie, RemoteHost::NotRequested))
            .collect();
        let delegate = delegate(cx, records, Vec::new());
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine;
        cx.update(|cx| {
            let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalDemand(demand));
        });
        let slot = Rc::new(RefCell::new(None));
        let keep = Rc::clone(&slot);
        cx.add_window(move |window, cx| {
            let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
            *keep.borrow_mut() = Some(table.clone());
            Root::new(table, window, cx)
        });
        let table = slot.borrow().clone().expect("table");
        cx.update(|cx| {
            let demand = cx.global::<GlobalDemand>().0.clone();
            table.update(cx, |table, cx| {
                let d = table.delegate_mut();
                d.set_demand(demand);
                d.set_active(true, cx);
            });
        });
        table
    }

    fn show_column(
        cx: &mut TestAppContext,
        table: &Entity<TlTreeTable<ConnectionsDelegate>>,
        on: bool,
    ) {
        cx.update(|cx| {
            table.update(cx, |table, cx| {
                let d = table.delegate_mut();
                let mut layout = d.columns().clone();
                layout
                    .set_visible(id(ConnColumn::RemoteHost), on)
                    .expect("visibility changes");
                d.on_columns_changed(&layout, cx);
            });
        });
    }

    fn report(
        cx: &mut TestAppContext,
        table: &Entity<TlTreeTable<ConnectionsDelegate>>,
        ids: &[u64],
    ) {
        cx.update(|cx| {
            table.update(cx, |table, cx| {
                table.delegate_mut().on_visible_range(ids, cx)
            })
        });
    }

    #[gpui_kit::test]
    fn remote_host_demand_only_when_visible(cx: &mut TestAppContext) {
        let table = rig(cx, 5);
        report(cx, &table, &[1, 2, 3]);
        assert!(cookies(cx).is_empty(), "the column is hidden");
        show_column(cx, &table, true);
        assert_eq!(cookies(cx), [1, 2, 3]);
        show_column(cx, &table, false);
        assert!(
            cookies(cx).is_empty(),
            "hiding the column withdraws the demand"
        );
        show_column(cx, &table, true);
        assert_eq!(cookies(cx), [1, 2, 3]);
        cx.update(|cx| table.update(cx, |t, cx| t.delegate_mut().set_active(false, cx)));
        assert!(cookies(cx).is_empty(), "an inactive tab demands nothing");
    }

    #[gpui_kit::test]
    fn remote_host_visible_rows_plus_overscan(cx: &mut TestAppContext) {
        let table = rig(cx, 100);
        show_column(cx, &table, true);
        // Rows 0 to 29 visible in a 30-row viewport: no rows above row 0.
        let (ids, expected): (Vec<u64>, Vec<u64>) = {
            let window = demand_window(0..30, 100);
            assert_eq!(window, 0..60);
            let ids: Vec<u64> = window.map(|ix| ix as u64 + 1).collect();
            (ids.clone(), ids)
        };
        report(cx, &table, &ids);
        assert_eq!(cookies(cx), expected);
        assert_eq!(expected.len(), 60);
    }

    fn pending_cell(host: RemoteHost, cx: &mut TestAppContext) -> (CellOut, ConnectionsDelegate) {
        let d = delegate(cx, vec![foreign_record(1, host)], Vec::new());
        (cell(&d, ConnColumn::RemoteHost), d)
    }

    #[gpui_kit::test]
    fn remote_host_pending_shows_address(cx: &mut TestAppContext) {
        let (out, _) = pending_cell(RemoteHost::Pending, cx);
        assert_eq!(text(&out), "10.0.0.1");
        let secondary = cx.update(|cx| current(cx).text_secondary);
        assert_eq!(out.text_color, Some(secondary));
    }

    #[gpui_kit::test]
    fn remote_host_found_shows_name(cx: &mut TestAppContext) {
        let (out, _) = pending_cell(RemoteHost::Name(Arc::from("example.org")), cx);
        assert_eq!(text(&out), "example.org");
        assert_eq!(out.text_color, None, "text_primary");
        assert_eq!(out.tooltip.as_deref(), Some("10.0.0.1"));
        let (out, _) = pending_cell(RemoteHost::NoName, cx);
        assert_eq!(text(&out), "10.0.0.1");
        assert_eq!(out.text_color, None, "text_primary");
    }

    #[gpui_kit::test]
    fn remote_host_listener_empty(cx: &mut TestAppContext) {
        let mut record = foreign_record(1, RemoteHost::Pending);
        record.state = ConnState::Listening;
        record.foreign = Arc::from("*:*");
        let d = delegate(cx, vec![record], Vec::new());
        let out = cell(&d, ConnColumn::RemoteHost);
        assert_eq!(text(&out), "");
        assert!(out.tooltip.is_none());
    }

    #[gpui_kit::test]
    fn remote_host_picker_note(cx: &mut TestAppContext) {
        let column = PickerColumn {
            key: "remote_host",
            label: "Remote host".into(),
            full_name: "Remote host".into(),
            group: "Remote host".into(),
            availability: Availability::Available,
            hideable: true,
            visible: false,
        };
        let key = column.note_key().expect("a note");
        assert_eq!(tl_ui::i18n::tr_en(key, &[]), "Sends DNS queries");
        let other = PickerColumn {
            key: "inode",
            ..column
        };
        assert_eq!(other.note_key(), None);
        let _ = cx;
    }
}

/// The Namespace cell: empty for a host socket, the namespace name for another one.
#[gpui_kit::test]
fn netns_cell_shows_name(cx: &mut TestAppContext) {
    let host = connection(1, SmallVec::new());
    let mut other = connection(2, SmallVec::new());
    other.netns = Some(tl_model::snapshot::NetnsRef {
        ino: 4026532000,
        name: Arc::from("blue"),
    });
    let delegate = delegate(cx, vec![host, other], Vec::new());
    assert_eq!(text(&cell_at(&delegate, 0, ConnColumn::Netns)), "");
    assert_eq!(text(&cell_at(&delegate, 1, ConnColumn::Netns)), "blue");
}

/// Text cells draw their values, never the warming-up "…" glyph (tasklens-rsf.30).
mod drawn_text {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::base::Root;
    use gpui_kit::{AppContext as _, VisualTestContext};
    use smallvec::smallvec;
    use tl_model::table::SysColumnId;
    use tl_ui::table::cells::{record_painted_texts, take_painted_texts};
    use tl_ui::table::tree_table::TlTreeTable;

    #[gpui_kit::test]
    fn conn_table_draws_cell_values(cx: &mut TestAppContext) {
        let delegate = delegate(
            cx,
            vec![connection(7, smallvec![key(42)])],
            vec![process(42, "sshd", "root", 0)],
        );
        let slot = Rc::new(RefCell::new(None));
        let keep = Rc::clone(&slot);
        let handle = cx.add_window(move |window, cx| {
            let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
            *keep.borrow_mut() = Some(table.clone());
            Root::new(table, window, cx)
        });
        let table = slot.borrow().clone().expect("table");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        record_painted_texts();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let painted = take_painted_texts();
        assert!(
            vcx.debug_bounds("TlCell-glyph").is_none(),
            "no cell of a current row draws a validity glyph"
        );

        // The one row's painted text by column, left to right. Process is the Name cell.
        let drawn: Vec<(SysColumnId, String)> = vcx.update(|_, cx| {
            let t = table.read(cx);
            let layout = t.columns();
            let mut drawn: Vec<(SysColumnId, String)> = painted
                .iter()
                .filter_map(|(bounds, text)| {
                    let col = layout.column_at_x(bounds.center().x)?;
                    Some((col, text.to_string()))
                })
                .collect();
            drawn.sort_by_key(|(col, _)| layout.visible_columns().iter().position(|c| c == col));
            drawn
        });
        assert_eq!(
            drawn,
            [
                (id(ConnColumn::Proto), "TCP".to_owned()),
                (id(ConnColumn::Local), "127.0.0.1:8080".to_owned()),
                (id(ConnColumn::Foreign), "10.0.0.1:443".to_owned()),
                (id(ConnColumn::State), "ESTABLISHED".to_owned()),
                (id(ConnColumn::Pid), "42".to_owned()),
            ]
        );
    }
}
