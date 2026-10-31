//! LabRuntime tests for client sessions (q4x76.4, ADR-018): each session has its own table
//! and system table queries and gets its own views; the local session alone behaves as before.
//! Sessions opened with a `PortState` (q4x76.5) close and take demand through that state.
//!
//! Each `run` call gets its own task. The messages are queued before the task starts and the
//! sender is dropped, so `run` drains the queue and then returns.

use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::client_session::{
    ClientMsg, ClientSessionId, ClientSessions, OpenError, PortState, SESSIONS_MAX, SessionLink,
};
use tl_engine::reducer::{Publisher, Reducer};
use tl_engine::source::WorkerControl;
use tl_engine::update::{EngineMsg, ProcessSweep, ServicesUpdate, SourceUpdate};
use tl_model::actions::SchedPolicy;
use tl_model::command::Command;
use tl_model::demand::Demand;
use tl_model::health::ManagerState;
use tl_model::ids::{ColumnId, GroupKey, ProcessKey};
use tl_model::process::{ProcFlags, ProcState, ProcessExtras, ProcessMeta, ProcessRecord};
use tl_model::snapshot::{ServiceBackend, ServiceStatus, Snapshot, StartupType, UnitRecord};
use tl_model::system::CpuSet;
use tl_model::table::{
    Manager, SortSpec, SysColumnId, SysNodeId, SysSortSpec, SysTable, SysTableQuery, TableMode,
    TableQuery, Total, UnitRef, UnitType,
};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};

const BOOT: BootId = BootId(0x1234);
const REMOTE: ClientSessionId = ClientSessionId(7);

struct Rig {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    reducer: Option<Reducer>,
    publisher: Option<Publisher>,
    _wake_rx: async_channel::Receiver<()>,
}

type RunOutput = Arc<Mutex<Option<(Reducer, Publisher)>>>;

impl Rig {
    fn new() -> Self {
        let (control_tx, _control_rx) = watch::channel(WorkerControl::default());
        let (events_tx, _events_rx) = async_channel::bounded(64);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let publisher = Publisher {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
            wake_tx,
            stop_publishing: false,
        };
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        Rig {
            lab,
            root,
            reducer: Some(Reducer::new(1, BOOT, control_tx, events_tx)),
            publisher: Some(publisher),
            _wake_rx: wake_rx,
        }
    }

    fn reducer(&self) -> &Reducer {
        self.reducer.as_ref().expect("the reducer is back")
    }

    /// Queues `msgs` as one batch, runs the loop until the channel closes and returns the
    /// local session's last snapshot.
    fn run(&mut self, msgs: Vec<EngineMsg>) -> Arc<Snapshot> {
        let (tx, mut rx) = mpsc::channel::<EngineMsg>(msgs.len().max(1));
        for msg in msgs {
            tx.try_send(msg)
                .expect("the channel has room for every message");
        }
        drop(tx);
        let mut reducer = self.reducer.take().expect("one run at a time");
        let mut publisher = self.publisher.take().expect("one run at a time");
        let out: RunOutput = Arc::default();
        let slot = Arc::clone(&out);
        let (task, _handle) = self
            .lab
            .state
            .create_task(self.root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                reducer
                    .run(&cx, &mut rx, &mut publisher)
                    .await
                    .expect("no invariant violation");
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((reducer, publisher));
            })
            .expect("task is created");
        self.lab.scheduler.lock().schedule(task, 0);
        self.lab.run_until_idle();
        let (reducer, publisher) = out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("run returned");
        let latest = publisher.latest.load_full();
        self.reducer = Some(reducer);
        self.publisher = Some(publisher);
        latest
    }
}

/// A link and the wake receiver that keeps it open.
fn link() -> (SessionLink, async_channel::Receiver<()>) {
    let (wake_tx, wake_rx) = async_channel::bounded(1);
    // Nothing here sends actions, so the events receiver may go.
    let (events_tx, _events_rx) = async_channel::bounded(1);
    let (requests_tx, _requests_rx) = async_channel::unbounded();
    let link = SessionLink {
        latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
        wake_tx,
        events_tx,
        requests_tx,
    };
    (link, wake_rx)
}

fn open(id: ClientSessionId, link: &SessionLink) -> EngineMsg {
    EngineMsg::Client(ClientMsg::Open {
        id,
        link: link.clone(),
    })
}

fn on(id: ClientSessionId, cmd: Command) -> EngineMsg {
    if id == ClientSessionId::LOCAL {
        EngineMsg::Command(cmd)
    } else {
        EngineMsg::Client(ClientMsg::Command {
            id,
            cmd: Box::new(cmd),
        })
    }
}

/// A flat table filtered by `filter`. Digits match PIDs by prefix.
fn table_query(generation: u64, filter: &str) -> Command {
    Command::SetTableQuery(TableQuery {
        generation,
        mode: TableMode::Flat,
        sort: SortSpec {
            column: ColumnId::Pid,
            ascending: true,
        },
        filter: Arc::from(filter),
        ..TableQuery::default()
    })
}

/// The Services table with the User group open, filtered by `filter`.
fn services_query(generation: u64, filter: &str) -> Command {
    Command::SetSysTableQuery(SysTableQuery {
        table: SysTable::Services,
        generation,
        sort: SysSortSpec {
            column: SysColumnId::all(SysTable::Services)[0],
            ascending: true,
        },
        filter: Arc::from(filter),
        expanded: Arc::from([SysNodeId::ServiceGroup(Manager::User)]),
        services: None,
    })
}

fn proc_record(pid: u32) -> ProcessRecord {
    ProcessRecord {
        key: ProcessKey {
            start_ticks: u64::from(pid) * 10,
            boot_id: BOOT,
            pid,
        },
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: "x".into(),
            name: "x".into(),
            exe: None,
            cmdline: None,
            user: "user".into(),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: "x user".into(),
        }),
        group: GroupKey(0),
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

fn proc_extras() -> ProcessExtras {
    ProcessExtras {
        cpu_kernel_pct: 0.0,
        cpu_user_pct: 0.0,
        cpu_relative_pct: 0.0,
        utime_ticks: 0,
        stime_ticks: 0,
        start_boot_ns: 0,
        rss_peak_bytes: 0,
        rss_shared_bytes: 0,
        commit_bytes: 0,
        vsize_bytes: 0,
        vsize_peak_bytes: 0,
        page_faults: 0,
        page_faults_delta: 0,
        major_faults: 0,
        policy: SchedPolicy::Other,
        rt_prio: 0,
        affinity: CpuSet::empty(),
        handles: None,
        exe_name: "x".into(),
        exe_path: "/usr/bin/x".into(),
        exe_deleted: false,
        unit: None,
        state_letter: b'S',
        frozen: false,
        disk_read_bps: 0.0,
        disk_write_raw_bps: 0.0,
        flag_since_ns: 0,
        pgrp: 0,
        sid: 0,
        last_cpu: 0,
        wine: None,
    }
}

/// A sweep that sees the processes `pids`, with index aligned extras.
fn sweep(t_ns: u64, pids: &[u32]) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Processes(ProcessSweep {
        observed_start: t_ns,
        observed_end: t_ns,
        records: pids.iter().map(|&pid| proc_record(pid)).collect(),
        extras: pids.iter().map(|_| proc_extras()).collect(),
        extras_p2: Vec::new(),
        app_stats: Vec::new(),
        unit_stats: Vec::new(),
        system_threads: 0,
        file_nr: None,
        online_cpus: 1,
        low_memory: None,
    }))
}

fn missing<T: Default>() -> Total<T> {
    Total {
        value: T::default(),
        validity: Validity::Missing,
    }
}

fn unit_rec(name: &str) -> UnitRecord {
    UnitRecord {
        unit: UnitRef {
            manager: Manager::User,
            name: Arc::from(name),
        },
        kind: UnitType::Service,
        description: Arc::from(""),
        status: ServiceStatus::Running,
        sub_state: Arc::from("running"),
        load_state: Arc::from("loaded"),
        startup: StartupType::Enabled,
        main_pid: 0,
        user: Arc::from("user"),
        path: Arc::from(""),
        unit_file: None,
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

/// The user manager's units `names`.
fn services(names: &[&str]) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Services(ServicesUpdate {
        manager: Manager::User,
        state: ManagerState::Connected,
        units: names.iter().map(|n| unit_rec(n)).collect(),
        details: Vec::new(),
        backend: ServiceBackend::Systemd,
        validity: Validity::Valid,
    }))
}

/// The PIDs of the table's process rows.
fn pids(snap: &Snapshot) -> Vec<u32> {
    snap.table
        .rows
        .iter()
        .filter_map(|r| r.record)
        .map(|i| snap.processes.records[i as usize].key.pid)
        .collect()
}

/// The unit names of the Services rows.
fn units(snap: &Snapshot) -> Vec<String> {
    snap.services
        .rows
        .iter()
        .filter_map(|r| match &r.node {
            SysNodeId::Unit(u) => Some(u.name.to_string()),
            _ => None,
        })
        .collect()
}

/// The data every test feeds: PIDs 10, 11 and 20, and units alpha, beta and beta2.
fn data() -> Vec<EngineMsg> {
    vec![
        sweep(1_000_000_000, &[10, 11, 20]),
        services(&["alpha.service", "beta.service", "beta2.service"]),
    ]
}

#[test]
fn two_sessions_get_their_own_table_and_services_views() {
    let mut rig = Rig::new();
    let (remote, _remote_wake) = link();
    let mut msgs = vec![open(REMOTE, &remote)];
    msgs.extend(data());
    msgs.extend([
        on(ClientSessionId::LOCAL, table_query(3, "1")),
        on(REMOTE, table_query(8, "2")),
        on(ClientSessionId::LOCAL, services_query(4, "alpha")),
        on(REMOTE, services_query(9, "beta")),
    ]);
    let local = rig.run(msgs);
    let other = remote.latest.load_full();

    assert_eq!(local.sequence, other.sequence, "one publish feeds both");
    assert_eq!(local.table.query_generation, 3);
    assert_eq!(pids(&local), [10, 11]);
    assert_eq!(other.table.query_generation, 8);
    assert_eq!(pids(&other), [20]);
    assert_eq!(local.services.query_generation, 4);
    assert_eq!(units(&local), ["alpha.service"]);
    assert_eq!(local.services.manager_counts, [0, 1]);
    assert_eq!(other.services.query_generation, 9);
    assert_eq!(units(&other), ["beta.service", "beta2.service"]);
    assert_eq!(other.services.manager_counts, [0, 2]);
    // Shared data is the same `Arc` in both snapshots.
    assert!(Arc::ptr_eq(&local.processes, &other.processes));
    assert!(Arc::ptr_eq(&local.services.units, &other.services.units));

    // A query change in one session leaves the other's parts as they were.
    let local_before = Arc::clone(&local);
    let other_before = Arc::clone(&other);
    let local = rig.run(vec![
        on(REMOTE, table_query(10, "")),
        on(REMOTE, services_query(11, "")),
    ]);
    let other = remote.latest.load_full();
    assert!(Arc::ptr_eq(&local.table, &local_before.table));
    assert!(Arc::ptr_eq(&local.services, &local_before.services));
    assert_eq!(pids(&other), [10, 11, 20]);
    assert_eq!(units(&other).len(), 3);
    assert!(other.table.header.version > other_before.table.header.version);
    assert!(other.services.header.version > other_before.services.header.version);

    // New shared data reaches both, each through its own query.
    let local = rig.run(vec![services(&["alpha.service", "beta.service"])]);
    let other = remote.latest.load_full();
    assert_eq!(units(&local), ["alpha.service"]);
    assert_eq!(units(&other), ["alpha.service", "beta.service"]);
    assert_eq!(local.services.query_generation, 4);
    assert_eq!(other.services.query_generation, 11);
    assert!(local.services.header.version > local_before.services.header.version);
}

#[test]
fn a_new_session_starts_with_default_queries_over_current_data() {
    let mut rig = Rig::new();
    let mut msgs = data();
    msgs.extend([
        on(ClientSessionId::LOCAL, table_query(3, "1")),
        on(ClientSessionId::LOCAL, services_query(4, "alpha")),
    ]);
    rig.run(msgs);
    let (remote, _remote_wake) = link();
    let local = rig.run(vec![open(REMOTE, &remote)]);
    let other = remote.latest.load_full();
    assert_eq!(pids(&local), [10, 11]);
    assert_eq!(other.table.query_generation, 0);
    assert_eq!(other.table.total, 3);
    assert_eq!(other.services.query_generation, 0);
    assert_eq!(other.services.units.len(), 3);
    assert!(other.table.header.version >= 1);
    assert!(other.services.header.version >= 1);
    assert!(other.startup.header.version >= 1);
    assert!(other.connections.header.version >= 1);
}

/// One session behaves as before: the local snapshot is the same whether or not another
/// session is open and sending its own queries.
#[test]
fn local_session_is_unchanged_by_another_session() {
    let local_msgs = || {
        let mut msgs = data();
        msgs.extend([
            on(ClientSessionId::LOCAL, table_query(3, "1")),
            on(ClientSessionId::LOCAL, services_query(4, "alpha")),
        ]);
        msgs
    };
    let mut alone = Rig::new();
    let a1 = alone.run(local_msgs());
    let a2 = alone.run(vec![
        services(&["alpha.service"]),
        sweep(2_000_000_000, &[10]),
    ]);
    assert_eq!(alone.reducer().clients.len(), 1);

    let mut shared = Rig::new();
    let (remote, _remote_wake) = link();
    let mut msgs = vec![open(REMOTE, &remote)];
    msgs.extend(local_msgs());
    msgs.extend([
        on(REMOTE, table_query(8, "2")),
        on(REMOTE, services_query(9, "beta")),
    ]);
    let s1 = shared.run(msgs);
    let s2 = shared.run(vec![
        services(&["alpha.service"]),
        sweep(2_000_000_000, &[10]),
        on(REMOTE, table_query(12, "")),
    ]);
    assert_eq!(shared.reducer().clients.len(), 2);

    for (a, s) in [(&a1, &s1), (&a2, &s2)] {
        assert_eq!(a.sequence, s.sequence);
        for (x, y) in [
            (&a.table.header, &s.table.header),
            (&a.services.header, &s.services.header),
            (&a.startup.header, &s.startup.header),
            (&a.connections.header, &s.connections.header),
        ] {
            assert_eq!((x.version, x.validity), (y.version, y.validity));
        }
        assert_eq!(a.table.query_generation, s.table.query_generation);
        assert_eq!(a.table.rows, s.table.rows);
        assert_eq!(a.services.query_generation, s.services.query_generation);
        assert_eq!(a.services.rows, s.services.rows);
        assert_eq!(a.services.counts, s.services.counts);
        assert_eq!(a.services.manager_counts, s.services.manager_counts);
    }
}

#[test]
fn registry_limits() {
    let empty = Snapshot::empty(BOOT, 1);
    let mut clients = ClientSessions::new(&empty);
    assert_eq!(clients.len(), 1);
    assert_eq!(clients.local().id(), ClientSessionId::LOCAL);
    let mut wakes = Vec::new();
    for i in 1..SESSIONS_MAX as u32 {
        let (l, w) = link();
        wakes.push(w);
        clients
            .open(ClientSessionId(i), l, &empty)
            .expect("room for 8 beside the local session");
    }
    let (l, _w) = link();
    assert_eq!(
        clients.open(ClientSessionId(1), l.clone(), &empty).err(),
        Some(OpenError::Duplicate)
    );
    assert_eq!(
        clients.open(ClientSessionId(99), l.clone(), &empty).err(),
        Some(OpenError::Full)
    );
    assert!(!clients.close(ClientSessionId::LOCAL), "local stays open");
    assert!(clients.close(ClientSessionId(3)));
    assert!(!clients.close(ClientSessionId(3)));
    clients
        .open(ClientSessionId(99), l, &empty)
        .expect("a closed session frees its place");
    let order: Vec<u32> = clients.iter().map(|s| s.id().0).collect();
    assert_eq!(
        order,
        [0, 1, 2, 4, 5, 6, 7, 8, 99],
        "connect order, local first"
    );
}

#[test]
fn closed_or_gone_sessions_stop_getting_snapshots() {
    let mut rig = Rig::new();
    let (kept, _kept_wake) = link();
    let (gone, gone_wake) = link();
    let gone_id = ClientSessionId(8);
    rig.run(vec![open(REMOTE, &kept), open(gone_id, &gone)]);
    assert_eq!(rig.reducer().clients.len(), 3);

    // A session whose wake receiver is gone is closed at the next publish.
    drop(gone_wake);
    rig.run(data());
    assert_eq!(rig.reducer().clients.len(), 2);
    assert!(rig.reducer().clients.get(gone_id).is_none());

    // A command from a closed session is dropped; the local session is not touched.
    let before = kept.latest.load_full();
    let local = rig.run(vec![
        EngineMsg::Client(ClientMsg::Close(REMOTE)),
        on(REMOTE, table_query(5, "2")),
    ]);
    assert_eq!(rig.reducer().clients.len(), 1);
    assert_eq!(local.table.query_generation, 0);
    assert!(Arc::ptr_eq(&kept.latest.load_full(), &before));
}

fn open_port(id: ClientSessionId, link: &SessionLink, port: &Arc<PortState>) -> EngineMsg {
    EngineMsg::Client(ClientMsg::OpenPort {
        id,
        link: link.clone(),
        port: Arc::clone(port),
    })
}

#[test]
fn a_dropped_port_closes_its_session_without_a_close_message() {
    let mut rig = Rig::new();
    let (l, _wake) = link();
    let port = Arc::new(PortState::default());
    rig.run(vec![open_port(REMOTE, &l, &port)]);
    assert!(rig.reducer().clients.get(REMOTE).is_some());
    assert!(!port.is_ended());

    // The port's drop only sets the flag. No `Close` and no `Wake` reaches the queue, as when
    // the queue was full; the next pass for any other message closes the session.
    port.close();
    rig.run(data());
    assert!(rig.reducer().clients.get(REMOTE).is_none());
    assert!(port.is_ended(), "the slot is free again");
}

#[test]
fn a_port_closed_before_its_open_is_never_opened() {
    let mut rig = Rig::new();
    let (l, _wake) = link();
    let port = Arc::new(PortState::default());
    port.close();
    rig.run(vec![open_port(REMOTE, &l, &port)]);
    assert!(rig.reducer().clients.get(REMOTE).is_none());
    assert!(port.is_ended());
}

#[test]
fn port_demand_is_level_state() {
    let empty = Snapshot::empty(BOOT, 1);
    let mut clients = ClientSessions::new(&empty);
    let (l, _wake) = link();
    let port = Arc::new(PortState::default());
    let first = Demand {
        memory: true,
        ..Demand::default()
    };
    port.set_demand(first.clone());
    let session = clients
        .open_port(REMOTE, l, Arc::clone(&port), &empty)
        .expect("room");
    assert_eq!(
        session.demand, first,
        "the demand set before the open is the first"
    );

    // Two demands between passes: the newer one wins, and nothing is queued.
    port.set_demand(Demand::default());
    let last = Demand {
        memory: true,
        disks: true,
        ..Demand::default()
    };
    port.set_demand(last.clone());
    assert!(!clients.sync_ports());
    assert_eq!(clients.get(REMOTE).expect("open").demand, last);
}

#[test]
fn refused_and_dropped_sessions_end_their_ports() {
    let empty = Snapshot::empty(BOOT, 1);
    let mut clients = ClientSessions::new(&empty);
    let mut wakes = Vec::new();
    let mut ports = Vec::new();
    for i in 1..SESSIONS_MAX as u32 {
        let (l, w) = link();
        wakes.push(w);
        let port = Arc::new(PortState::default());
        clients
            .open_port(ClientSessionId(i), l, Arc::clone(&port), &empty)
            .expect("room for 8 beside the local session");
        ports.push(port);
    }
    let (l, _w) = link();
    let refused = Arc::new(PortState::default());
    assert_eq!(
        clients
            .open_port(ClientSessionId(99), l, Arc::clone(&refused), &empty)
            .err(),
        Some(OpenError::Full)
    );
    assert!(
        refused.is_ended(),
        "a refused port learns it got no session"
    );

    assert!(clients.close(ClientSessionId(1)));
    assert!(ports[0].is_ended());
    assert!(!ports[1].is_ended());

    // A reducer restart drops its registry; every port of it ends.
    drop(clients);
    assert!(ports.iter().all(|p| p.is_ended()));
}
