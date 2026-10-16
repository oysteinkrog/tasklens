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
    ComponentHeader, ConnRecord, ConnState, ConnectionsSnapshot, ProcessSnapshot, Snapshot,
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
        remote_host: None,
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

    #[gpui_kit::test]
    fn conn_menu_close_hidden_before_p3(cx: &mut TestAppContext) {
        let mut rig = menu_rig(cx, vec![super::connection(1, SmallVec::new())]);
        let menu = rig.build_menu(&[1], None);
        assert!(
            rig.menu_entries(&menu)
                .iter()
                .all(|entry| { entry.label.as_deref() != Some("Close TCP connection") })
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
mod jump {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use gpui_kit::{AppContext as _, VisualTestContext};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::ids::NodeId;
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::ConnCounts;
    use tl_model::table::{Manager, ProcessTableView, TableRow, UnitRef};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::pages::connections::ConnectionsPage;
    use tl_ui::pages::connections::actions::{ConnectionGoToService, ConnectionJumpToProcess};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::services::actions::{GoToService, JumpOrigin, ManagerArg};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: gpui_kit::Entity<ConnectionsPage>,
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
    fn conn_go_to_service(cx: &mut TestAppContext) {
        let mut record = owned(&[4321]);
        record.unit = Some(UnitRef {
            manager: Manager::System,
            name: Arc::from("sshd.service"),
        });
        let mut rig = rig(cx, record, procs(&[4321]));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        cx.update(|cx| {
            cx.on_action(move |a: &GoToService, _| sink.borrow_mut().push(a.clone()));
        });
        rig.dispatch(true, ConnectionGoToService);
        assert_eq!(
            *seen.borrow(),
            [GoToService {
                manager: ManagerArg::System,
                name: "sshd.service".into(),
                origin: JumpOrigin::Connections,
            }]
        );
    }
}
