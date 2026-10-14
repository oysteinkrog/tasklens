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
        AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render,
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
    use tl_model::table::SysTable;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::toast::ToastQueue;
    use tl_ui::demand::{DEMAND_COALESCE, GlobalDemand};
    use tl_ui::hub::GlobalHub;
    use tl_ui::pages::connections::actions::bind_keys_connections;
    use tl_ui::pages::connections::{ConnectionsPage, REFRESH_TOAST_AFTER};
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
            self.version += 1;
            let generation = self.generation();
            let records = cookies
                .iter()
                .map(|cookie| super::connection(*cookie, SmallVec::new()))
                .collect::<Vec<_>>();
            let rows = cookies
                .iter()
                .enumerate()
                .map(|(index, cookie)| row(*cookie, index))
                .collect::<Vec<_>>();
            let version = self.version;
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
            });
            self.vcx.run_until_parked();
            self.draw();
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
