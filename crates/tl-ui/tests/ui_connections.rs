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
