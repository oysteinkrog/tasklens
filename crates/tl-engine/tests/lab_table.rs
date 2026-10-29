//! Table builder tests (E09-014, ADR-013): flat mode (E09-009), tree mode (E09-010), grouped
//! mode (E09-011), hide system processes (E09-012), pseudo rows (E09-023), unit row names
//! (E09-025), plus a 2,000-row timing smoke check and a determinism check.
//!
//! The builder is pure (records in, rows out), so these tests need no runtime. Each test builds
//! its records with `rec` and the `in_app`, `in_unit` and `kthread` helpers, then calls `build`.
//! Records passed to `build` must be sorted by `ProcessKey`, as the reducer keeps them;
//! `rec` sets `start_ticks = pid × 10`, so ascending PIDs are sorted.
//!
//! The unit row name tests run the reducer in a `LabRuntime`, because the names come from
//! `ServicesUpdate` messages and the table version is set at publish.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use rustc_hash::FxHashMap;
use tl_collect::proc::cgroup::{UserPlacement, parse_cgroup_file, placement};
use tl_engine::reducer::table::{self, GROUP_ORDER, IRQ_NOT_ACCOUNTED, PseudoInput, TableInput};
use tl_engine::reducer::{Publisher, Reducer};
use tl_engine::source::WorkerControl;
use tl_engine::update::{
    CgroupStats, EngineMsg, ProcessSweep, ServicesUpdate, SourceUpdate, ZramStat,
};
use tl_model::actions::{CgroupRef, SchedPolicy};
use tl_model::command::Command;
use tl_model::devices::DeviceCatalog;
use tl_model::health::ManagerState;
use tl_model::ids::{ColumnId, GroupKey, NodeId, ProcessKey, PseudoKind};
use tl_model::process::{
    AppCgroupSample, GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessExtras,
    ProcessMeta, ProcessRecord, is_row_member,
};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    ServiceBackend, ServiceStatus, Snapshot, StartupType, SystemSnapshot, UnitRecord,
};
use tl_model::system::CpuSet;
use tl_model::table::{
    Manager, ProcessTableView, SortSpec, TableMode, TableQuery, Total, UnitRef, UnitType,
};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, ProcessField, Validity};

const GIB: u64 = 1 << 30;
const USER_MANAGER: &str = "user.slice/user-1000.slice/user@1000.service";

// Harness.

/// A Background record of user 1000 with every field `Valid`. `start_ticks = pid × 10`.
fn rec(pid: u32, ppid: u32, name: &str, cpu: f32) -> ProcessRecord {
    ProcessRecord {
        key: ProcessKey {
            start_ticks: u64::from(pid) * 10,
            boot_id: BootId(1),
            pid,
        },
        ppid,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: name.into(),
            name: name.into(),
            exe: None,
            cmdline: None,
            user: "user".into(),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: format!("{name} user").to_lowercase().into(),
        }),
        group: GroupKind::Background.key(),
        state: ProcState::Sleeping,
        flags: ProcFlags::empty(),
        nice: 0,
        threads: 1,
        cpu_pct: cpu,
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

/// A member of app `app_id`: `GroupKind::App` and a cgroup scope under `app.slice`.
fn in_app(mut r: ProcessRecord, app_id: &str) -> ProcessRecord {
    r.group = GroupKind::App {
        app_id: app_id.into(),
    }
    .key();
    let meta = Arc::make_mut(&mut r.meta);
    meta.app_id = Some(app_id.into());
    meta.cgroup = Some(format!("{USER_MANAGER}/app.slice/app-{app_id}.scope").into());
    r
}

/// A process of systemd unit `unit` in `slice`. `system.slice` makes it a System process of
/// the system manager; any other slice sits under the user manager and stays Background.
fn in_unit(mut r: ProcessRecord, slice: &str, unit: &str) -> ProcessRecord {
    let path = if slice == "system.slice" {
        r.group = GroupKind::System.key();
        format!("system.slice/{unit}")
    } else {
        format!("{USER_MANAGER}/{slice}/{unit}")
    };
    Arc::make_mut(&mut r.meta).cgroup = Some(path.into());
    r
}

/// A kernel thread `kworker/<pid>` under kthreadd.
fn kthread(pid: u32) -> ProcessRecord {
    let mut r = rec(pid, 2, &format!("kworker/{pid}"), 0.0);
    r.group = GroupKind::System.key();
    r.flags |= ProcFlags::KERNEL_THREAD;
    r.uid = 0;
    r
}

fn query(mode: TableMode, col: &str, asc: bool, filter: &str, expanded: &[NodeId]) -> TableQuery {
    let column = ColumnId::from_str(col).unwrap_or_else(|| panic!("unknown column {col:?}"));
    TableQuery {
        mode,
        sort: SortSpec {
            column,
            ascending: asc,
        },
        filter: Arc::from(filter),
        expanded: Arc::from(expanded.to_vec()),
        ..TableQuery::default()
    }
}

/// The unit of a record, read from its cgroup path the way the sweep fills
/// `ProcessExtras.unit`: the last component when it is a `.service` or `.scope`. App scopes and
/// the user manager itself are not units here.
fn unit_of(r: &ProcessRecord) -> Option<UnitRef> {
    let path = r.meta.cgroup.as_deref()?;
    if path.contains("/app.slice/") {
        return None;
    }
    let last = path.rsplit('/').next()?;
    if !(last.ends_with(".service") || last.ends_with(".scope")) || last.starts_with("user@") {
        return None;
    }
    let manager = if path.contains("/user@") {
        Manager::User
    } else {
        Manager::System
    };
    Some(UnitRef {
        manager,
        name: last.into(),
    })
}

fn extras(r: &ProcessRecord) -> ProcessExtras {
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
        exe_name: r.meta.comm.clone(),
        exe_path: format!("/usr/bin/{}", r.meta.comm).into(),
        exe_deleted: false,
        unit: unit_of(r),
        state_letter: b'S',
        frozen: false,
        disk_read_bps: 0.0,
        disk_write_raw_bps: 0.0,
        flag_since_ns: 0,
        pgrp: 0,
        sid: 0,
        last_cpu: 0,
    }
}

fn build(records: &[ProcessRecord], q: &TableQuery, settings: &EngineSettings) -> ProcessTableView {
    build_with(records, &[], q, settings)
}

/// `build` with app cgroup samples.
fn build_with(
    records: &[ProcessRecord],
    app_stats: &[AppCgroupSample],
    q: &TableQuery,
    settings: &EngineSettings,
) -> ProcessTableView {
    build_full(records, app_stats, &[], None, q, settings)
}

/// `build` with pseudo rows.
fn build_pseudo(
    records: &[ProcessRecord],
    pseudo: &PseudoInput,
    q: &TableQuery,
    settings: &EngineSettings,
) -> ProcessTableView {
    build_full(records, &[], &[], Some(pseudo), q, settings)
}

/// `build` with unit cgroup samples.
fn build_units(
    records: &[ProcessRecord],
    unit_stats: &[CgroupStats],
    q: &TableQuery,
    settings: &EngineSettings,
) -> ProcessTableView {
    build_full(records, &[], unit_stats, None, q, settings)
}

fn build_full(
    records: &[ProcessRecord],
    app_stats: &[AppCgroupSample],
    unit_stats: &[CgroupStats],
    pseudo: Option<&PseudoInput>,
    q: &TableQuery,
    settings: &EngineSettings,
) -> ProcessTableView {
    let mem_groups = FxHashMap::default();
    build_mem(
        records,
        app_stats,
        unit_stats,
        pseudo,
        &mem_groups,
        q,
        settings,
    )
}

/// `build_full` with the memory group key per process.
fn build_mem(
    records: &[ProcessRecord],
    app_stats: &[AppCgroupSample],
    unit_stats: &[CgroupStats],
    pseudo: Option<&PseudoInput>,
    mem_groups: &FxHashMap<ProcessKey, Arc<str>>,
    q: &TableQuery,
    settings: &EngineSettings,
) -> ProcessTableView {
    assert!(
        records.is_sorted_by_key(|r| r.key),
        "records must be sorted by ProcessKey"
    );
    let extras: Vec<ProcessExtras> = records.iter().map(extras).collect();
    let system = SystemSnapshot::default();
    let devices = DeviceCatalog::default();
    let unit_names = FxHashMap::default();
    let input = TableInput {
        records,
        extras: &extras,
        app_stats,
        unit_stats,
        system: &system,
        devices: &devices,
        settings,
        unit_names: &unit_names,
        unit_users: &unit_names,
        pseudo,
        mem_groups,
    };
    table::build(&input, q)
}

/// (name, depth) per row: the process name, the group row name, or the header name.
fn names(view: &ProcessTableView, records: &[ProcessRecord]) -> Vec<(String, u8)> {
    view.rows
        .iter()
        .map(|row| {
            let name = match (row.record, row.group) {
                (Some(i), _) => records[i as usize].meta.name.to_string(),
                (None, Some(g)) => view.groups[g as usize].name.to_string(),
                (None, None) => GROUP_ORDER
                    .iter()
                    .find(|c| row.node == NodeId::Group(c.key()))
                    .map_or_else(|| format!("{:?}", row.node), |c| c.name().to_string()),
            };
            (name, row.depth)
        })
        .collect()
}

fn pids(view: &ProcessTableView, records: &[ProcessRecord]) -> Vec<u32> {
    view.rows
        .iter()
        .filter_map(|row| row.record)
        .map(|i| records[i as usize].key.pid)
        .collect()
}

fn settings() -> EngineSettings {
    EngineSettings::default()
}

fn node(records: &[ProcessRecord], pid: u32) -> NodeId {
    let r = records
        .iter()
        .find(|r| r.key.pid == pid)
        .expect("no record with that PID");
    NodeId::Process(r.key)
}

fn nodes(records: &[ProcessRecord], pids: &[u32]) -> Vec<NodeId> {
    pids.iter().map(|&p| node(records, p)).collect()
}

fn headers() -> Vec<NodeId> {
    GROUP_ORDER.iter().map(|c| NodeId::Group(c.key())).collect()
}

/// The header row counts in `GROUP_ORDER`.
fn header_counts(view: &ProcessTableView) -> Vec<u32> {
    view.rows
        .iter()
        .filter(|row| row.record.is_none() && row.group.is_none())
        .map(|row| row.descendants)
        .collect()
}

fn row(name: &str, depth: u8) -> (String, u8) {
    (name.to_string(), depth)
}

// Flat mode (E09-009).

#[test]
fn flat_sort_pid_both_directions() {
    let records = vec![
        rec(1, 0, "a", 0.0),
        rec(2, 0, "b", 0.0),
        rec(3, 0, "c", 0.0),
    ];
    let view = build(
        &records,
        &query(TableMode::Flat, "pid", true, "", &[]),
        &settings(),
    );
    assert_eq!(pids(&view, &records), [1, 2, 3]);
    let view = build(
        &records,
        &query(TableMode::Flat, "pid", false, "", &[]),
        &settings(),
    );
    assert_eq!(pids(&view, &records), [3, 2, 1]);
    assert!(view.groups.is_empty());
    assert!(view.rows.iter().all(|r| r.depth == 0 && !r.has_children));
}

#[test]
fn tie_break_by_key() {
    // Key order is start_ticks first: PID 7 started before PID 3.
    let mut early = rec(7, 0, "early", 5.0);
    early.key.start_ticks = 10;
    let mut late = rec(3, 0, "late", 5.0);
    late.key.start_ticks = 70;
    let busy = rec(9, 0, "busy", 50.0);
    let records = vec![early, late, busy];
    for asc in [true, false] {
        let view = build(
            &records,
            &query(TableMode::Flat, "cpu", asc, "", &[]),
            &settings(),
        );
        let order = pids(&view, &records);
        let ties: Vec<u32> = order.iter().copied().filter(|&p| p != 9).collect();
        assert_eq!(ties, [7, 3], "ascending {asc}");
        assert_eq!(order[if asc { 2 } else { 0 }], 9, "ascending {asc}");
    }
}

#[test]
fn filter_terms_and_pid_prefix() {
    let records = vec![
        rec(1234, 1, "Firefox", 0.0),
        rec(1250, 1, "bash", 0.0),
        rec(2123, 1, "Firefox", 0.0),
    ];
    let view = build(
        &records,
        &query(TableMode::Flat, "pid", true, "fire 12", &[]),
        &settings(),
    );
    assert_eq!(pids(&view, &records), [1234]);
    assert_eq!(view.shown, 1);
    assert_eq!(view.total, 3);
}

#[test]
fn filter_user_field() {
    let mut root = rec(10, 1, "sshd", 0.0);
    let meta = Arc::make_mut(&mut root.meta);
    meta.user = "root".into();
    meta.search_lower = "sshd root".into();
    let records = vec![root, rec(20, 1, "bash", 0.0)];
    let view = build(
        &records,
        &query(TableMode::Flat, "pid", true, "root", &[]),
        &settings(),
    );
    assert_eq!(pids(&view, &records), [10]);
}

#[test]
fn non_sortable_falls_back_to_pid() {
    let mut records: Vec<ProcessRecord> = (1..=5).map(|p| rec(p, 0, "p", 0.0)).collect();
    for (r, pss) in records.iter_mut().zip([50, 10, 40, 20, 30]) {
        r.pss_bytes = pss;
    }
    for asc in [true, false] {
        let view = build(
            &records,
            &query(TableMode::Flat, "pss", asc, "", &[]),
            &settings(),
        );
        assert_eq!(pids(&view, &records), [1, 2, 3, 4, 5], "ascending {asc}");
    }
}

#[test]
fn absent_sorts_last() {
    let mut warming = rec(1, 0, "warming", 99.0);
    warming.validity.set(ProcessField::Cpu, Validity::WarmingUp);
    let records = vec![warming, rec(2, 0, "low", 1.0), rec(3, 0, "high", 9.0)];
    let view = build(
        &records,
        &query(TableMode::Flat, "cpu", false, "", &[]),
        &settings(),
    );
    assert_eq!(pids(&view, &records), [3, 2, 1]);
    let view = build(
        &records,
        &query(TableMode::Flat, "cpu", true, "", &[]),
        &settings(),
    );
    assert_eq!(pids(&view, &records), [2, 3, 1]);
}

#[test]
fn generation_echo() {
    let records = vec![rec(1, 0, "a", 0.0)];
    let mut q = query(TableMode::Flat, "pid", true, "", &[]);
    q.generation = 42;
    assert_eq!(build(&records, &q, &settings()).query_generation, 42);
}

// Tree mode (E09-010).

/// A(1) -> B(2) -> C(3).
fn abc() -> Vec<ProcessRecord> {
    vec![
        rec(1, 0, "A", 0.0),
        rec(2, 1, "B", 0.0),
        rec(3, 2, "C", 0.0),
    ]
}

/// (pid, depth, has_children, descendants) per row of a tree build sorted by PID ascending.
fn tree(
    records: &[ProcessRecord],
    filter: &str,
    include_children: bool,
    expanded: &[u32],
) -> Vec<(u32, u8, bool, u32)> {
    let mut q = query(
        TableMode::Tree,
        "pid",
        true,
        filter,
        &nodes(records, expanded),
    );
    q.include_children = include_children;
    let view = build(records, &q, &settings());
    view.rows
        .iter()
        .map(|row| {
            let r = &records[row.record.expect("tree rows are processes") as usize];
            (r.key.pid, row.depth, row.has_children, row.descendants)
        })
        .collect()
}

#[test]
fn tree_depths_and_has_children() {
    let rows = tree(&abc(), "", false, &[1, 2]);
    assert_eq!(rows, [(1, 0, true, 2), (2, 1, true, 1), (3, 2, false, 0)]);
}

#[test]
fn collapsed_parent_descendant_count() {
    let rows = tree(&abc(), "", false, &[1]);
    assert_eq!(rows, [(1, 0, true, 2), (2, 1, true, 1)]);
}

#[test]
fn filtered_child_becomes_root() {
    let rows = tree(&abc(), "c", false, &[1, 2]);
    assert_eq!(rows, [(3, 0, false, 0)]);
}

#[test]
fn include_children_pulls_descendants() {
    let rows = tree(&abc(), "a", true, &[1, 2]);
    assert_eq!(rows, [(1, 0, true, 2), (2, 1, true, 1), (3, 2, false, 0)]);
    let rows = tree(&abc(), "a", false, &[1, 2]);
    assert_eq!(rows, [(1, 0, false, 0)]);
}

#[test]
fn exited_parent_keeps_place_then_child_reroots() {
    let mut records = abc();
    records[1].flags |= ProcFlags::EXITED;
    records[1].state = ProcState::Zombie;
    let rows = tree(&records, "", false, &[1, 2]);
    assert_eq!(rows, [(1, 0, true, 2), (2, 1, true, 1), (3, 2, false, 0)]);
    records.remove(1);
    let rows = tree(&records, "", false, &[1]);
    assert_eq!(rows, [(1, 0, false, 0), (3, 0, false, 0)]);
}

#[test]
fn siblings_sorted_under_parent() {
    let records = vec![
        rec(10, 0, "low", 10.0),
        rec(11, 10, "low-a", 1.0),
        rec(12, 10, "low-b", 5.0),
        rec(20, 0, "high", 20.0),
        rec(21, 20, "high-a", 3.0),
        rec(22, 20, "high-b", 7.0),
    ];
    let q = query(
        TableMode::Tree,
        "cpu",
        false,
        "",
        &nodes(&records, &[10, 20]),
    );
    let view = build(&records, &q, &settings());
    assert_eq!(pids(&view, &records), [20, 22, 21, 10, 12, 11]);
}

// Grouped mode (E09-011). Depths (UI-PRC-002): headers 0; apps, unit rows, Kernel threads and
// loose processes 1; processes under an app, a unit or Kernel threads 2 and deeper.

/// 2 app processes, 3 background, 4 system, the system ones busiest.
fn mixed() -> Vec<ProcessRecord> {
    vec![
        in_app(rec(100, 1, "firefox", 1.0), "firefox"),
        in_app(rec(101, 100, "firefox-content", 2.0), "firefox"),
        rec(200, 1, "bash", 30.0),
        rec(201, 1, "zsh", 20.0),
        rec(202, 1, "fish", 10.0),
        in_unit(rec(300, 1, "sshd", 90.0), "system.slice", "sshd.service"),
        in_unit(rec(301, 1, "cron", 80.0), "system.slice", "cron.service"),
        in_unit(rec(302, 1, "dbus", 70.0), "system.slice", "dbus.service"),
        in_unit(rec(303, 1, "udevd", 60.0), "system.slice", "udevd.service"),
    ]
}

#[test]
fn group_order_fixed() {
    let records = mixed();
    for (col, asc) in [("cpu", false), ("pid", true), ("name", false)] {
        let view = build(
            &records,
            &query(TableMode::Grouped, col, asc, "", &[]),
            &settings(),
        );
        assert_eq!(
            names(&view, &records),
            [
                row("Apps", 0),
                row("Background processes", 0),
                row("System processes", 0),
            ],
            "sort {col} {asc}"
        );
        assert_eq!(header_counts(&view), [2, 3, 4]);
    }
    let view = build(
        &records,
        &query(TableMode::Grouped, "cpu", false, "", &headers()),
        &settings(),
    );
    assert_eq!(
        names(&view, &records),
        [
            row("Apps", 0),
            row("firefox", 1),
            row("Background processes", 0),
            row("bash", 1),
            row("zsh", 1),
            row("fish", 1),
            row("System processes", 0),
            row("sshd", 1),
            row("cron", 1),
            row("dbus", 1),
            row("udevd", 1),
        ]
    );
    assert_eq!(view.shown, 9);
}

fn app_key(app_id: &str) -> GroupKey {
    GroupKind::App {
        app_id: app_id.into(),
    }
    .key()
}

/// Firefox: primary P (cpu 10, 1 GiB anon) and child C (cpu 5, 0.5 GiB anon).
fn app_pc() -> Vec<ProcessRecord> {
    let mut p = in_app(rec(100, 1, "Firefox", 10.0), "firefox");
    p.rss_anon_bytes = GIB;
    let mut c = in_app(rec(101, 100, "Web Content", 5.0), "firefox");
    c.rss_anon_bytes = GIB / 2;
    vec![p, c]
}

#[test]
fn app_row_aggregates_once() {
    let records = app_pc();
    let mut expanded = headers();
    expanded.push(NodeId::Group(app_key("firefox")));
    expanded.push(node(&records, 100));
    let view = build(
        &records,
        &query(TableMode::Grouped, "cpu", false, "", &expanded),
        &settings(),
    );

    assert_eq!(view.groups.len(), 1);
    let g = &view.groups[0];
    assert_eq!(g.cpu_pct, 15.0);
    assert_eq!(g.members, 2);
    assert_eq!(g.kind, GroupRowKind::App);
    assert_eq!(g.node, NodeId::Group(app_key("firefox")));
    assert_eq!(header_counts(&view), [2, 0, 0]);
    // The child sits in the primary's subtree, one level under it.
    assert_eq!(
        names(&view, &records),
        [
            row("Apps", 0),
            row("Firefox", 1),
            row("Firefox", 2),
            row("Web Content", 3),
            row("Background processes", 0),
            row("System processes", 0),
        ]
    );
}

/// An app row takes the desktop path of its first member and shares its `Arc`; an app with no
/// desktop entry gets `None`.
#[test]
fn app_row_desktop_path_from_first_member() {
    let path: Arc<str> = Arc::from("/usr/share/applications/firefox.desktop");
    let mut records = app_pc();
    for r in &mut records {
        Arc::make_mut(&mut r.meta).desktop_path = Some(Arc::clone(&path));
    }
    records.push(in_app(rec(200, 1, "tool", 1.0), "tool"));
    let view = build(
        &records,
        &query(TableMode::Grouped, "pid", true, "", &[]),
        &settings(),
    );

    let group = |key| {
        view.groups
            .iter()
            .find(|g| g.key == key)
            .expect("app row built")
    };
    let firefox = group(app_key("firefox"));
    assert!(
        firefox
            .desktop_path
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(p, &path))
    );
    assert_eq!(group(app_key("tool")).desktop_path, None);
}

#[test]
fn app_memory_from_cgroup_anon() {
    let records = app_pc();
    let q = query(TableMode::Grouped, "pid", true, "", &[]);
    let view = build(&records, &q, &settings());
    assert_eq!(view.groups[0].memory_bytes, GIB + GIB / 2);

    let stats = [AppCgroupSample {
        group: app_key("firefox"),
        anon_bytes: GIB,
        swap_bytes: 0,
        io_read_bps: 0.0,
        io_write_bps: 0.0,
        rios: 0,
        wios: 0,
        pids: 2,
        frozen: false,
        cgroup: CgroupRef {
            rel_path: format!("{USER_MANAGER}/app.slice/app-firefox.scope").into(),
            cgroup_id: 1,
        },
        validity: FieldValidity::all(Validity::Valid),
    }];
    let view = build_with(&records, &stats, &q, &settings());
    assert_eq!(view.groups[0].memory_bytes, GIB);
}

/// An app row is frozen when its cgroup's `cgroup.events` says so (tasklens-8q0.41).
#[test]
fn app_row_frozen_from_cgroup_events() {
    let mut records = app_pc();
    records.push(in_app(rec(200, 1, "Tool", 1.0), "tool"));
    let q = query(TableMode::Grouped, "pid", true, "", &[]);
    let frozen_of = |view: &ProcessTableView, app: &str| {
        view.groups
            .iter()
            .find(|g| g.key == app_key(app))
            .expect("app row built")
            .frozen
    };

    let view = build(&records, &q, &settings());
    assert!(!frozen_of(&view, "firefox"), "no cgroup sample, not frozen");

    let sample = |app: &str, id: u64, frozen: bool| AppCgroupSample {
        group: app_key(app),
        anon_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.0,
        io_write_bps: 0.0,
        rios: 0,
        wios: 0,
        pids: 1,
        frozen,
        cgroup: CgroupRef {
            rel_path: format!("{USER_MANAGER}/app.slice/app-{app}.scope").into(),
            cgroup_id: id,
        },
        validity: FieldValidity::all(Validity::Valid),
    };
    let stats = [sample("firefox", 1, true), sample("tool", 2, false)];
    let view = build_with(&records, &stats, &q, &settings());
    assert!(frozen_of(&view, "firefox"));
    assert!(!frozen_of(&view, "tool"));
}

/// A unit row is frozen when its unit cgroup sample says so (tasklens-8q0.41). The sample is
/// matched by the unit name and manager in its cgroup path.
#[test]
fn unit_row_frozen_from_unit_stats() {
    // A unit folds into a row when it has more than one process.
    let proc_in = |pid: u32, name: &str, slice: &str, unit: &str| {
        in_unit(rec(pid, 1, name, 1.0), slice, unit)
    };
    let records = vec![
        proc_in(10, "pipewire", "session.slice", "pipewire.service"),
        proc_in(11, "wireplumber", "session.slice", "pipewire.service"),
        proc_in(20, "kded6", "session.slice", "plasma-kded6.service"),
        proc_in(21, "kded6-helper", "session.slice", "plasma-kded6.service"),
        proc_in(30, "sshd", "system.slice", "sshd.service"),
        proc_in(31, "sshd-session", "system.slice", "sshd.service"),
    ];
    let sample = |rel: String, frozen: bool| CgroupStats {
        group: GroupKind::Background.key(),
        cgroup: CgroupRef {
            rel_path: rel.into(),
            cgroup_id: 0,
        },
        anon_bytes: 0,
        swap_bytes: 0,
        cpu_usage_usec: 0,
        io_rbytes: 0,
        io_wbytes: 0,
        pids_current: 1,
        frozen,
        validity: Validity::Valid,
    };
    let stats = [
        sample(
            format!("{USER_MANAGER}/session.slice/pipewire.service"),
            true,
        ),
        sample("system.slice/sshd.service".into(), false),
    ];
    let view = build_units(
        &records,
        &stats,
        &query(TableMode::Grouped, "pid", true, "", &[]),
        &settings(),
    );
    let frozen_of = |name: &str| {
        view.groups
            .iter()
            .find(|g| g.kind == GroupRowKind::Unit && &*g.name == name)
            .expect("unit row built")
            .frozen
    };
    assert!(frozen_of("pipewire.service"));
    assert!(!frozen_of("sshd.service"));
    assert!(
        !frozen_of("plasma-kded6.service"),
        "no cgroup sample, not frozen"
    );
}

#[test]
fn unit_rows_fold_multi_process_units() {
    let records = vec![
        in_unit(
            rec(10, 1, "pipewire", 1.0),
            "session.slice",
            "pipewire.service",
        ),
        in_unit(
            rec(11, 1, "wireplumber", 1.0),
            "session.slice",
            "pipewire.service",
        ),
        in_unit(
            rec(12, 1, "pipewire-pulse", 1.0),
            "session.slice",
            "pipewire.service",
        ),
        in_unit(
            rec(20, 1, "kded6", 1.0),
            "session.slice",
            "plasma-kded6.service",
        ),
        rec(30, 1, "bash", 1.0),
    ];
    let pw = UnitRef {
        manager: Manager::User,
        name: "pipewire.service".into(),
    };
    let mut expanded = headers();
    expanded.push(NodeId::Unit(pw.clone()));
    let view = build(
        &records,
        &query(TableMode::Grouped, "pid", true, "", &expanded),
        &settings(),
    );

    let units: Vec<&GroupRow> = view
        .groups
        .iter()
        .filter(|g| g.kind == GroupRowKind::Unit)
        .collect();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].node, NodeId::Unit(pw));
    assert_eq!(units[0].class, GroupKind::Background);
    assert_eq!(units[0].members, 3);
    assert_eq!(
        names(&view, &records),
        [
            row("Apps", 0),
            row("Background processes", 0),
            // A unit row has no PID, so the PID sort puts it after the processes.
            row("kded6", 1),
            row("bash", 1),
            row("pipewire.service", 1),
            row("pipewire", 2),
            row("wireplumber", 2),
            row("pipewire-pulse", 2),
            row("System processes", 0),
        ]
    );
}

/// Two System units get their own row keys, so App Properties lists and focuses each unit
/// alone (tasklens-8q0.40). `is_row_member` answers for both unit and app keys, and app rows keep
/// the app's class key.
#[test]
fn unit_rows_have_unique_keys() {
    let records = vec![
        in_unit(rec(10, 1, "foo", 1.0), "system.slice", "foo.service"),
        in_unit(
            rec(11, 10, "foo-worker", 1.0),
            "system.slice",
            "foo.service",
        ),
        in_unit(rec(20, 1, "bar", 1.0), "system.slice", "bar.service"),
        in_unit(
            rec(21, 20, "bar-worker", 1.0),
            "system.slice",
            "bar.service",
        ),
        in_app(rec(30, 1, "Firefox", 1.0), "firefox"),
    ];
    let unit = |name: &str| UnitRef {
        manager: Manager::System,
        name: name.into(),
    };
    let (foo, bar) = (unit("foo.service"), unit("bar.service"));
    let view = build(
        &records,
        &query(TableMode::Grouped, "pid", true, "", &headers()),
        &settings(),
    );

    let row = |node: NodeId| {
        view.groups
            .iter()
            .find(|g| g.node == node)
            .expect("the group has a row")
    };
    let foo_row = row(NodeId::Unit(foo.clone()));
    let bar_row = row(NodeId::Unit(bar.clone()));
    assert_eq!(foo_row.class, GroupKind::System);
    assert_eq!(bar_row.class, GroupKind::System);
    assert_eq!(foo_row.key, foo.key());
    assert_eq!(bar_row.key, bar.key());
    assert_ne!(foo_row.key, bar_row.key);
    assert_ne!(foo_row.key, GroupKind::System.key());
    // Records keep their class key.
    assert!(
        records[..4]
            .iter()
            .all(|r| r.group == GroupKind::System.key())
    );

    let app = row(NodeId::Group(app_key("firefox")));
    assert_eq!(app.key, app_key("firefox"));

    let extras: Vec<ProcessExtras> = records.iter().map(extras).collect();
    let members = |key: GroupKey| -> Vec<u32> {
        records
            .iter()
            .zip(&extras)
            .filter(|(r, x)| is_row_member(r, Some(x), key))
            .map(|(r, _)| r.key.pid)
            .collect()
    };
    assert_eq!(members(foo_row.key), [10, 11]);
    assert_eq!(members(bar_row.key), [20, 21]);
    assert_eq!(members(app.key), [30]);
}

#[test]
fn kernel_threads_row_first_in_system() {
    let mut records = vec![in_unit(
        rec(1, 0, "systemd", 0.0),
        "system.slice",
        "init.scope",
    )];
    records.extend((1000..1050).map(kthread));
    records.push(in_unit(
        rec(5000, 1, "sshd", 0.0),
        "system.slice",
        "sshd.service",
    ));

    let view = build(
        &records,
        &query(TableMode::Grouped, "pid", true, "", &headers()),
        &settings(),
    );
    let rows = names(&view, &records);
    assert_eq!(
        rows[2..],
        [
            row("System processes", 0),
            row("Kernel threads", 1),
            row("systemd", 1),
            row("sshd", 1),
        ]
    );
    assert_eq!(header_counts(&view), [0, 0, 52]);
    let kernel = &view.rows[3];
    assert_eq!(kernel.node, NodeId::KernelThreads);
    assert!(!kernel.expanded);
    assert!(kernel.has_children);
    assert_eq!(kernel.descendants, 50);

    let mut expanded = headers();
    expanded.push(NodeId::KernelThreads);
    let view = build(
        &records,
        &query(TableMode::Grouped, "pid", true, "", &expanded),
        &settings(),
    );
    let rows = names(&view, &records);
    assert_eq!(rows.len(), 6 + 50);
    assert_eq!(rows[3], row("Kernel threads", 1));
    assert!(view.rows[3].expanded);
    assert_eq!(rows[4], row("kworker/1000", 2));
    assert_eq!(rows[53], row("kworker/1049", 2));
    assert_eq!(rows[54..], [row("systemd", 1), row("sshd", 1)]);
}

#[test]
fn group_name_filter_matches_members() {
    let mut records = vec![in_unit(
        rec(1, 0, "systemd", 0.0),
        "system.slice",
        "init.scope",
    )];
    records.extend(app_pc());
    records.push(rec(200, 1, "bash", 0.0));
    let q = |filter: &str, expanded: &[NodeId]| {
        query(TableMode::Grouped, "pid", true, filter, expanded)
    };

    let view = build(&records, &q("apps", &[]), &settings());
    assert_eq!(header_counts(&view), [2, 0, 0]);
    assert_eq!(view.shown, 2);

    // The app name matches the child named "Web Content" too.
    let view = build(&records, &q("firefox", &[]), &settings());
    assert_eq!(header_counts(&view), [2, 0, 0]);

    let view = build(&records, &q("system processes", &[]), &settings());
    assert_eq!(header_counts(&view), [0, 0, 1]);

    // Headers stay with count 0 when nothing matches.
    let view = build(&records, &q("nomatch", &headers()), &settings());
    assert_eq!(header_counts(&view), [0, 0, 0]);
    assert_eq!(view.rows.len(), 3);
    assert!(view.rows.iter().all(|r| !r.has_children));
}

/// D-036, UI-PRF-060: `TableQuery::mem_group` shows exactly the processes of that memory
/// group, ANDs with the text filter, and hides the pseudo rows.
#[test]
fn mem_group_filter_exact() {
    let records = vec![
        rec(1, 0, "fish", 0.0),
        rec(2, 1, "claude 2.1.309", 0.0),
        rec(3, 1, "claude 2.1.310", 0.0),
        in_app(rec(4, 0, "firefox", 0.0), "firefox"),
        in_app(rec(5, 4, "Web Content", 0.0), "firefox"),
        // Its name matches "claude", but it is not in the group.
        rec(6, 1, "claude-notes", 0.0),
        kthread(7),
    ];
    let key = |k: &str| -> Arc<str> { Arc::from(k) };
    let mem_groups: FxHashMap<ProcessKey, Arc<str>> = records
        .iter()
        .map(|r| {
            let k = match r.key.pid {
                1 => key("exe:fish"),
                2 | 3 => key("exe:claude"),
                4 | 5 => key("app:firefox"),
                6 => key("exe:vim"),
                _ => key("system"),
            };
            (r.key, k)
        })
        .collect();
    let pseudo = pseudo(Some(1.0), vec![zram("zram0", GIB)]);
    let view = |mode: TableMode, filter: &str, expanded: &[NodeId]| {
        let mut q = query(mode, "pid", true, filter, expanded);
        q.mem_group = Some(Arc::from("exe:claude"));
        build_mem(
            &records,
            &[],
            &[],
            Some(&pseudo),
            &mem_groups,
            &q,
            &settings(),
        )
    };

    // Flat: exactly the claude processes, no pseudo rows.
    let v = view(TableMode::Flat, "", &[]);
    assert_eq!(pids(&v, &records), [2, 3]);
    assert_eq!(v.shown, 2);
    assert!(!has_pseudo(&v));
    // The text filter still applies.
    let v = view(TableMode::Flat, "309", &[]);
    assert_eq!(pids(&v, &records), [2]);
    let v = view(TableMode::Flat, "notes", &[]);
    assert!(pids(&v, &records).is_empty());

    // Grouped: the headers stay, only the members show under them.
    let v = view(TableMode::Grouped, "", &headers());
    assert_eq!(header_counts(&v), [0, 2, 0]);
    assert_eq!(
        names(&v, &records),
        [
            row("Apps", 0),
            row("Background processes", 0),
            row("claude 2.1.309", 1),
            row("claude 2.1.310", 1),
            row("System processes", 0),
        ]
    );
    assert!(!has_pseudo(&v));
    let v = view(TableMode::Grouped, "309", &headers());
    assert_eq!(header_counts(&v), [0, 1, 0]);

    // Tree: the parent is not in the group, so its matching children become roots.
    let v = view(TableMode::Tree, "", &nodes(&records, &[1]));
    let rows: Vec<(u32, u8)> = v
        .rows
        .iter()
        .map(|row| {
            let r = &records[row.record.expect("tree rows are processes") as usize];
            (r.key.pid, row.depth)
        })
        .collect();
    assert_eq!(rows, [(2, 0), (3, 0)]);

    // Without a group the filter is off.
    let q = query(TableMode::Flat, "pid", true, "", &[]);
    let v = build_mem(&records, &[], &[], None, &mem_groups, &q, &settings());
    assert_eq!(v.shown, 7);
}

/// Apps (one nested in another), a two-process unit, loose Background and System processes,
/// a system unit, PID 1 and 10 kernel threads.
fn full() -> Vec<ProcessRecord> {
    let mut records = vec![
        in_unit(rec(1, 0, "systemd", 1.0), "system.slice", "init.scope"),
        in_unit(
            rec(10, 1, "pipewire", 1.0),
            "session.slice",
            "pipewire.service",
        ),
        in_unit(
            rec(11, 10, "pipewire-pulse", 1.0),
            "session.slice",
            "pipewire.service",
        ),
        rec(30, 1, "bash", 1.0),
        in_unit(rec(40, 1, "sshd", 1.0), "system.slice", "sshd.service"),
        in_app(rec(100, 1, "firefox", 1.0), "firefox"),
        in_app(rec(101, 100, "firefox", 2.0), "firefox"),
        in_app(rec(102, 101, "firefox", 2.0), "firefox"),
        in_app(rec(110, 1, "konsole", 1.0), "konsole"),
        // A child of an app process in another app: it starts its own subtree.
        in_app(rec(111, 110, "kate", 1.0), "kate"),
    ];
    records.extend((1000..1010).map(kthread));
    records
}

/// Every header, group and process node: the build with this list shows every row.
fn all_nodes(records: &[ProcessRecord]) -> Vec<NodeId> {
    let mut all = headers();
    all.extend(records.iter().map(|r| NodeId::Process(r.key)));
    all.extend(records.iter().map(|r| NodeId::Group(r.group)));
    all.extend(records.iter().filter_map(unit_of).map(NodeId::Unit));
    all.push(NodeId::KernelThreads);
    all
}

#[test]
fn counts_sum_to_visible_records() {
    let records = full();
    let all = all_nodes(&records);
    for filter in ["", "fire", "pipe", "system processes", "kworker", "1"] {
        let view = build(
            &records,
            &query(TableMode::Grouped, "cpu", false, filter, &all),
            &settings(),
        );
        let sum: u32 = header_counts(&view).iter().sum();
        let mut shown: Vec<u32> = view.rows.iter().filter_map(|r| r.record).collect();
        let rows = shown.len();
        shown.sort_unstable();
        shown.dedup();
        assert_eq!(shown.len(), rows, "filter {filter:?}: a record shows twice");
        assert_eq!(sum as usize, rows, "filter {filter:?}");
        assert_eq!(view.shown, sum, "filter {filter:?}");
        let members: u32 = view.groups.iter().map(|g| g.members).sum();
        let in_groups = view
            .rows
            .iter()
            .filter(|r| r.record.is_some() && r.depth > 1)
            .count();
        assert_eq!(members as usize, in_groups, "filter {filter:?}");
    }
    let view = build(
        &records,
        &query(TableMode::Grouped, "cpu", false, "", &all),
        &settings(),
    );
    assert_eq!(view.shown as usize, records.len());
}

// Hide system processes (E09-012, PROC-046).

fn hide_system() -> EngineSettings {
    EngineSettings {
        hide_system: true,
        ..EngineSettings::default()
    }
}

/// `r` with cgroup path `path`, relative to /sys/fs/cgroup.
fn in_cgroup(mut r: ProcessRecord, path: &str) -> ProcessRecord {
    Arc::make_mut(&mut r.meta).cgroup = Some(path.into());
    r
}

/// PID 1, the user manager, desktop infrastructure, an exception, a terminal in `app.slice`,
/// an app, a system unit and two kernel threads. With `hide_system`, 100, 202 and 204 remain.
fn desktop() -> Vec<ProcessRecord> {
    let mut pid1 = in_cgroup(rec(1, 0, "systemd", 0.0), "init.scope");
    pid1.group = GroupKind::System.key();
    pid1.uid = 0;
    let mut records = vec![
        pid1,
        in_app(rec(100, 200, "firefox", 0.0), "firefox"),
        in_cgroup(
            rec(200, 1, "systemd-user", 0.0),
            &format!("{USER_MANAGER}/init.scope"),
        ),
        in_unit(
            rec(201, 204, "pipewire", 0.0),
            "session.slice",
            "pipewire.service",
        ),
        in_unit(
            rec(202, 200, "plasmashell", 0.0),
            "session.slice",
            "plasma-plasmashell.service",
        ),
        in_unit(
            rec(203, 200, "kded6", 0.0),
            "background.slice",
            "plasma-kded6.service",
        ),
        in_unit(
            rec(204, 200, "bash", 0.0),
            "app.slice",
            "app-konsole@1.service",
        ),
        in_unit(rec(300, 1, "cron", 0.0), "system.slice", "cron.service"),
    ];
    records.extend((1000..1002).map(kthread));
    records
}

#[test]
fn hide_system_removes_system_and_infra() {
    let records = vec![
        in_unit(
            rec(10, 1, "pipewire", 0.0),
            "session.slice",
            "pipewire.service",
        ),
        in_cgroup(
            in_app(rec(20, 1, "firefox", 0.0), "firefox"),
            &format!("{USER_MANAGER}/app.slice/app-firefox@1.service"),
        ),
        in_unit(rec(30, 1, "cron", 0.0), "system.slice", "cron.service"),
    ];
    for mode in [TableMode::Flat, TableMode::Tree, TableMode::Grouped] {
        let q = query(mode, "pid", true, "", &all_nodes(&records));
        let view = build(&records, &q, &hide_system());
        assert_eq!(pids(&view, &records), [20], "{mode:?}");
        // R-280: total leaves out hidden records too.
        assert_eq!(view.total, 1, "{mode:?}");
        assert_eq!(view.shown, 1, "{mode:?}");
    }

    let view = build(
        &desktop(),
        &query(TableMode::Flat, "pid", true, "", &[]),
        &hide_system(),
    );
    assert_eq!(pids(&view, &desktop()), [100, 202, 204]);
    assert_eq!(view.total, 3);
}

#[test]
fn hide_system_exceptions_stay() {
    let records: Vec<ProcessRecord> = table::HIDE_SYSTEM_EXCEPTIONS
        .iter()
        .zip(10u32..)
        .map(|(unit, pid)| in_unit(rec(pid, 1, unit, 0.0), "session.slice", unit))
        .collect();
    let view = build(
        &records,
        &query(TableMode::Flat, "pid", true, "", &[]),
        &hide_system(),
    );
    assert_eq!(pids(&view, &records), [10, 11, 12]);
}

#[test]
fn hide_system_grouped_drops_system_header() {
    let records = desktop();
    let view = build(
        &records,
        &query(TableMode::Grouped, "pid", true, "", &all_nodes(&records)),
        &hide_system(),
    );
    assert!(!view.rows.iter().any(|r| {
        r.node == NodeId::Group(GroupKind::System.key()) || r.node == NodeId::KernelThreads
    }));
    // Background keeps plasmashell and bash; pipewire, kded6 and the user manager are hidden.
    assert_eq!(header_counts(&view), [1, 2]);
    assert_eq!(
        names(&view, &records),
        [
            row("Apps", 0),
            row("firefox", 1),
            row("firefox", 2),
            row("Background processes", 0),
            row("plasmashell", 1),
            row("bash", 1),
        ]
    );
    assert_eq!(view.shown, 3);
}

#[test]
fn hide_system_off_is_noop() {
    let records = desktop();
    for mode in [TableMode::Flat, TableMode::Tree, TableMode::Grouped] {
        let q = query(mode, "pid", true, "", &all_nodes(&records));
        let off = build(&records, &q, &settings());
        assert_eq!(off.shown as usize, records.len(), "{mode:?}");
        assert_eq!(off.total as usize, records.len(), "{mode:?}");
        let mut shown = pids(&off, &records);
        shown.sort_unstable();
        let all: Vec<u32> = records.iter().map(|r| r.key.pid).collect();
        assert_eq!(shown, all, "{mode:?}");
    }
}

#[test]
fn hide_system_user_init_scope() {
    let records = desktop();
    let user_manager = &records[2];
    assert_eq!(user_manager.key.pid, 200);
    let class = table::group_class(user_manager);
    assert_eq!(class, table::GroupClass::Background);
    assert!(table::hidden_by_hide_system(user_manager, class));
    let pid1 = &records[0];
    let class = table::group_class(pid1);
    assert_eq!(class, table::GroupClass::System);
    assert!(table::hidden_by_hide_system(pid1, class));

    // In tree mode the hidden user manager's children become roots (PROC-014).
    let q = query(TableMode::Tree, "pid", true, "", &all_nodes(&records));
    let view = build(&records, &q, &hide_system());
    assert_eq!(pids(&view, &records), [100, 202, 204]);
    assert!(view.rows.iter().all(|r| r.depth == 0));
}

// Whole-builder checks (E09-014).

/// `n` records: 20 apps of 10 processes each, one in four of the rest in units of 5, the rest
/// loose Background or System processes, and 100 kernel threads. Sorted by key.
fn generated(n: u32) -> Vec<ProcessRecord> {
    (1..=n)
        .map(|pid| {
            let cpu = ((pid * 37) % 101) as f32 / 10.0;
            let name = format!("proc-{}", (pid * 7919) % 1000);
            match pid {
                p if p <= 100 => kthread(p),
                p if p <= 300 => {
                    let app = (p - 101) / 10;
                    let ppid = if (p - 101) % 10 == 0 { 1 } else { p - 1 };
                    in_app(rec(p, ppid, &name, cpu), &format!("app{app}"))
                }
                p if p % 4 == 0 => {
                    let unit = format!("unit{}.service", p / 20);
                    in_unit(rec(p, 1, &name, cpu), "session.slice", &unit)
                }
                p if p % 4 == 1 => in_unit(
                    rec(p, 1, &name, cpu),
                    "system.slice",
                    &format!("s{p}.service"),
                ),
                p => rec(p, 1, &name, cpu),
            }
        })
        .collect()
}

#[test]
fn build_2000_rows_debug_under_50ms() {
    let records = generated(2000);
    let mut expanded = headers();
    expanded.extend(records.iter().map(|r| NodeId::Group(r.group)));
    expanded.extend(records.iter().filter_map(unit_of).map(NodeId::Unit));
    let q = query(TableMode::Grouped, "cpu", false, "", &expanded);
    let settings = settings();
    for run in 0..10 {
        let start = Instant::now();
        let view = build(&records, &q, &settings);
        let took = start.elapsed();
        assert_eq!(view.shown, 2000);
        assert!(took.as_millis() < 50, "run {run} took {took:?}");
    }
}

#[test]
fn deterministic_order() {
    let records = generated(2000);
    let all = all_nodes(&records);
    for mode in [TableMode::Flat, TableMode::Tree, TableMode::Grouped] {
        let q = query(mode, "name", true, "", &all);
        let a = build(&records, &q, &settings());
        let b = build(&records, &q, &settings());
        assert_eq!(a.rows, b.rows, "{mode:?}");
        assert_eq!(a.groups.len(), b.groups.len(), "{mode:?}");
        assert!(
            a.groups
                .iter()
                .zip(b.groups.iter())
                .all(|(x, y)| x.node == y.node),
            "{mode:?}"
        );
    }
}

// Subtree totals (E09-026).

/// A → B → C with CPU 1, 2, 3.
fn abc_cpu() -> Vec<ProcessRecord> {
    vec![
        rec(1, 0, "A", 1.0),
        rec(2, 1, "B", 2.0),
        rec(3, 2, "C", 3.0),
    ]
}

fn subtree_on() -> EngineSettings {
    EngineSettings {
        subtree_totals: true,
        ..EngineSettings::default()
    }
}

/// Tree build sorted by PID ascending with `expanded` open.
fn tree_view(
    records: &[ProcessRecord],
    filter: &str,
    expanded: &[u32],
    settings: &EngineSettings,
) -> ProcessTableView {
    let q = query(
        TableMode::Tree,
        "pid",
        true,
        filter,
        &nodes(records, expanded),
    );
    build(records, &q, settings)
}

/// The subtree row of the row for `pid`, if it has one.
fn subtree_of<'a>(
    view: &'a ProcessTableView,
    records: &[ProcessRecord],
    pid: u32,
) -> Option<&'a GroupRow> {
    let row = view
        .rows
        .iter()
        .find(|row| {
            row.record
                .is_some_and(|i| records[i as usize].key.pid == pid)
        })
        .expect("no row with that PID");
    row.subtree.map(|s| &view.subtree[s as usize])
}

#[test]
fn subtree_totals_bottom_up() {
    let records = abc_cpu();
    let view = tree_view(&records, "", &[1, 2], &subtree_on());
    let a = subtree_of(&view, &records, 1).expect("A has a subtree row");
    assert_eq!(a.cpu_pct, 6.0);
    assert_eq!(a.node, node(&records, 1));
    assert_eq!(a.members, 3);
    let b = subtree_of(&view, &records, 2).expect("B has a subtree row");
    assert_eq!(b.cpu_pct, 5.0);
    assert!(subtree_of(&view, &records, 3).is_none());
    assert_eq!(view.subtree.len(), 2);
}

#[test]
fn subtree_validity_is_worst_of() {
    let mut records = abc_cpu();
    records[2]
        .validity
        .set(ProcessField::Cpu, Validity::Restricted);
    let view = tree_view(&records, "", &[1, 2], &subtree_on());
    let a = subtree_of(&view, &records, 1).expect("A has a subtree row");
    assert_eq!(a.validity.get(ProcessField::Cpu), Validity::Restricted);
    assert_eq!(a.validity.get(ProcessField::RssAnon), Validity::Valid);
}

#[test]
fn subtree_only_for_collapsed_when_setting_off() {
    let records = abc_cpu();
    let view = tree_view(&records, "", &[1, 2], &settings());
    assert!(view.rows.iter().all(|row| row.subtree.is_none()));
    assert!(view.subtree.is_empty());

    let view = tree_view(&records, "", &[], &settings());
    let a = subtree_of(&view, &records, 1).expect("collapsed A has a subtree row");
    assert_eq!(a.cpu_pct, 6.0);
    assert_eq!(view.rows.len(), 1);
}

#[test]
fn subtree_memory_sum() {
    const MIB: u64 = 1 << 20;
    let mut records = abc_cpu();
    for (r, mib) in records.iter_mut().zip([100, 200, 300]) {
        r.rss_anon_bytes = mib * MIB;
    }
    let view = tree_view(&records, "", &[1, 2], &subtree_on());
    let a = subtree_of(&view, &records, 1).expect("A has a subtree row");
    assert_eq!(a.rss_anon_bytes, 600 * MIB);
    assert_eq!(a.memory_bytes, 600 * MIB);
}

#[test]
fn subtree_sums_visible_only() {
    // A → B → C, plus A → D. The filter drops B; without include_children C becomes a root.
    let records = vec![
        rec(1, 0, "keep-a", 1.0),
        rec(2, 1, "drop-b", 2.0),
        rec(3, 2, "keep-c", 3.0),
        rec(4, 1, "keep-d", 4.0),
    ];
    let view = tree_view(&records, "keep", &[1], &subtree_on());
    let roots: Vec<u32> = view
        .rows
        .iter()
        .filter(|row| row.depth == 0)
        .map(|row| {
            records[row.record.expect("tree rows are processes") as usize]
                .key
                .pid
        })
        .collect();
    assert_eq!(roots, [1, 3]);
    let a = subtree_of(&view, &records, 1).expect("A has a subtree row");
    assert_eq!(a.cpu_pct, 5.0);
    assert_eq!(a.members, 2);
    assert!(subtree_of(&view, &records, 3).is_none());
}

// Pseudo rows (E09-023).

/// A zram device with `disksize`, 3 GiB original, 1 GiB compressed, 1.25 GiB used, reading 100
/// and writing 200 bytes per second.
fn zram(name: &str, disksize: u64) -> ZramStat {
    ZramStat {
        name: name.into(),
        disksize,
        orig_bytes: 3 * GIB,
        compr_bytes: GIB,
        mem_used_total: GIB + GIB / 4,
        read_bps: 100.0,
        write_bps: 200.0,
    }
}

fn pseudo(irq_pct: Option<f32>, zram: Vec<ZramStat>) -> PseudoInput {
    PseudoInput { irq_pct, zram }
}

/// The group row of the pseudo row `kind` in `view`.
fn pseudo_row(view: &ProcessTableView, kind: PseudoKind) -> Option<&GroupRow> {
    view.rows
        .iter()
        .find(|row| row.node == NodeId::Pseudo(kind))
        .map(|row| &view.groups[row.group.expect("a pseudo row indexes a group row") as usize])
}

fn has_pseudo(view: &ProcessTableView) -> bool {
    view.rows
        .iter()
        .any(|row| matches!(row.node, NodeId::Pseudo(_)))
}

#[test]
fn pseudo_rows_after_kernel_threads() {
    let mut records = vec![in_unit(
        rec(1, 0, "systemd", 0.0),
        "system.slice",
        "init.scope",
    )];
    records.extend((1000..1003).map(kthread));
    records.push(in_unit(
        rec(5000, 1, "sshd", 0.0),
        "system.slice",
        "sshd.service",
    ));
    let input = pseudo(Some(1.5), vec![zram("zram0", 8 * GIB)]);

    let view = build_pseudo(
        &records,
        &input,
        &query(TableMode::Grouped, "pid", true, "", &headers()),
        &settings(),
    );
    assert_eq!(
        names(&view, &records)[2..],
        [
            row("System processes", 0),
            row("Kernel threads", 1),
            row("systemd", 1),
            row("sshd", 1),
            row("Interrupts", 1),
            row("Memory compression (zram)", 1),
        ]
    );
    let n = view.rows.len();
    assert_eq!(
        view.rows[n - 2].node,
        NodeId::Pseudo(PseudoKind::Interrupts)
    );
    assert_eq!(
        view.rows[n - 1].node,
        NodeId::Pseudo(PseudoKind::MemoryCompression)
    );
    assert!(view.rows[n - 2..].iter().all(|r| !r.has_children));
    // The header count and the process counts leave the pseudo rows out.
    assert_eq!(header_counts(&view), [0, 0, 5]);
    assert_eq!((view.total, view.shown), (5, 5));

    let irq = pseudo_row(&view, PseudoKind::Interrupts).expect("Interrupts row");
    assert_eq!(irq.kind, GroupRowKind::Pseudo);
    assert_eq!(irq.cpu_pct, 1.5);
    assert_eq!(irq.validity.get(ProcessField::Cpu), Validity::Valid);
    assert_eq!(
        irq.validity.get(ProcessField::RssAnon),
        Validity::Unsupported
    );
    assert_eq!(
        irq.validity.get(ProcessField::IoRead),
        Validity::Unsupported
    );
    assert_eq!((irq.members, irq.pids), (0, 0));
    assert_eq!(irq.note, None);

    let z = pseudo_row(&view, PseudoKind::MemoryCompression).expect("zram row");
    assert_eq!(z.rss_anon_bytes, GIB + GIB / 4);
    assert_eq!(z.memory_bytes, GIB + GIB / 4);
    assert_eq!(
        (z.io_read_bps, z.io_write_bps, z.disk_bps),
        (100.0, 200.0, 300.0)
    );
    assert_eq!(z.validity.get(ProcessField::Cpu), Validity::Unsupported);
    assert_eq!(z.validity.get(ProcessField::RssAnon), Validity::Valid);
    assert_eq!(z.validity.get(ProcessField::IoWrite), Validity::Valid);
    assert_eq!(z.validity.get(ProcessField::NetRx), Validity::Unsupported);
    assert_eq!(
        z.note.as_deref(),
        Some("Original 3.00 GiB · Compressed 1.00 GiB · Ratio 3.00")
    );
}

#[test]
fn pseudo_zram_only_with_disksize() {
    let records = vec![rec(1, 0, "a", 0.0)];
    let q = query(TableMode::Flat, "pid", true, "", &[]);
    for zram_devices in [vec![], vec![zram("zram0", 0), zram("zram1", 0)]] {
        let view = build_pseudo(&records, &pseudo(Some(0.5), zram_devices), &q, &settings());
        assert!(pseudo_row(&view, PseudoKind::Interrupts).is_some());
        assert!(pseudo_row(&view, PseudoKind::MemoryCompression).is_none());
        assert_eq!(view.groups.len(), 1);
    }

    // Only the set-up device counts toward the sums.
    let view = build_pseudo(
        &records,
        &pseudo(Some(0.5), vec![zram("zram0", 0), zram("zram1", GIB)]),
        &q,
        &settings(),
    );
    let z = pseudo_row(&view, PseudoKind::MemoryCompression).expect("zram row");
    assert_eq!(z.rss_anon_bytes, GIB + GIB / 4);
    assert_eq!(z.io_read_bps, 100.0);
}

#[test]
fn pseudo_interrupts_unsupported_without_irq_accounting() {
    let records = vec![rec(1, 0, "a", 0.0)];
    let view = build_pseudo(
        &records,
        &pseudo(None, vec![]),
        &query(TableMode::Flat, "cpu", false, "", &[]),
        &settings(),
    );
    let irq = pseudo_row(&view, PseudoKind::Interrupts).expect("Interrupts row");
    assert_eq!(irq.validity.get(ProcessField::Cpu), Validity::Unsupported);
    assert_eq!(irq.note.as_deref(), Some(IRQ_NOT_ACCOUNTED));
    // An unsupported CPU sorts last, after a process at 0%.
    assert_eq!(view.rows[1].node, NodeId::Pseudo(PseudoKind::Interrupts));
}

#[test]
fn pseudo_rows_sort_in_flat() {
    let records = vec![rec(1, 0, "one", 1.0), rec(2, 0, "two", 2.0)];
    let input = pseudo(Some(1.5), vec![]);
    for mode in [TableMode::Flat, TableMode::Tree] {
        let view = build_pseudo(
            &records,
            &input,
            &query(mode, "cpu", false, "", &[]),
            &settings(),
        );
        assert_eq!(
            names(&view, &records),
            [row("two", 0), row("Interrupts", 0), row("one", 0)],
            "{mode:?}"
        );
        assert_eq!(view.shown, 2);
    }
}

#[test]
fn pseudo_rows_hidden_by_hide_system() {
    let records = desktop();
    let input = pseudo(Some(1.5), vec![zram("zram0", GIB)]);
    for mode in [TableMode::Grouped, TableMode::Tree, TableMode::Flat] {
        let q = query(mode, "pid", true, "", &headers());
        let view = build_pseudo(&records, &input, &q, &hide_system());
        assert!(!has_pseudo(&view), "{mode:?}");
        assert!(view.groups.iter().all(|g| g.kind != GroupRowKind::Pseudo));
        let view = build_pseudo(&records, &input, &q, &settings());
        assert!(has_pseudo(&view), "{mode:?}");
    }
}

#[test]
fn pseudo_zram_filter_by_name() {
    let records = vec![rec(1, 0, "a", 0.0), kthread(1000)];
    let input = pseudo(Some(1.5), vec![zram("zram0", GIB)]);
    let view = build_pseudo(
        &records,
        &input,
        &query(TableMode::Flat, "pid", true, "zram", &[]),
        &settings(),
    );
    assert_eq!(
        names(&view, &records),
        [row("Memory compression (zram)", 0)]
    );
    assert_eq!(view.shown, 0);

    // Grouped: the System header holds the row, with a count of 0 processes.
    let view = build_pseudo(
        &records,
        &input,
        &query(TableMode::Grouped, "pid", true, "zram", &headers()),
        &settings(),
    );
    assert_eq!(
        names(&view, &records)[2..],
        [
            row("System processes", 0),
            row("Memory compression (zram)", 1),
        ]
    );
    assert_eq!(header_counts(&view), [0, 0, 0]);
    assert!(view.rows[2].has_children);
}

// Unit row names (E09-025).

const NAMES_BOOT: BootId = BootId(1);

/// A reducer and its publisher, run in a `LabRuntime` one batch of messages at a time.
struct Engine {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    reducer: Option<Reducer>,
    publisher: Option<Publisher>,
    /// Kept so the publisher does not stop when it finds the receiver gone.
    _wake_rx: async_channel::Receiver<()>,
}

type RunOutput = Arc<Mutex<Option<(Reducer, Publisher)>>>;

impl Engine {
    fn new() -> Self {
        let (control_tx, _control_rx) = watch::channel(WorkerControl::default());
        let (events_tx, _events_rx) = async_channel::bounded(64);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let publisher = Publisher {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(NAMES_BOOT, 1))),
            wake_tx,
            stop_publishing: false,
        };
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        Engine {
            lab,
            root,
            reducer: Some(Reducer::new(1, NAMES_BOOT, control_tx, events_tx)),
            publisher: Some(publisher),
            _wake_rx: wake_rx,
        }
    }

    /// Queues `msgs` as one batch, runs the loop until the channel closes and returns the
    /// last published snapshot.
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

fn pipewire() -> UnitRef {
    UnitRef {
        manager: Manager::User,
        name: "pipewire.service".into(),
    }
}

/// Three processes in the user unit `pipewire.service`.
fn pipewire_records() -> Vec<ProcessRecord> {
    [
        (10, "pipewire"),
        (11, "wireplumber"),
        (12, "pipewire-pulse"),
    ]
    .into_iter()
    .map(|(pid, name)| in_unit(rec(pid, 1, name, 1.0), "session.slice", "pipewire.service"))
    .collect()
}

/// The grouped query with every header expanded.
fn grouped_query() -> EngineMsg {
    EngineMsg::Command(Command::SetTableQuery(query(
        TableMode::Grouped,
        "pid",
        true,
        "",
        &headers(),
    )))
}

fn sweep_of(records: Vec<ProcessRecord>) -> EngineMsg {
    let extras = records.iter().map(extras).collect();
    EngineMsg::Source(SourceUpdate::Processes(ProcessSweep {
        observed_start: 1,
        observed_end: 1,
        records,
        extras,
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

fn unit_record(unit: UnitRef, description: &str) -> UnitRecord {
    UnitRecord {
        unit,
        kind: UnitType::Service,
        description: description.into(),
        status: ServiceStatus::Running,
        sub_state: "running".into(),
        load_state: "loaded".into(),
        startup: StartupType::Enabled,
        main_pid: 0,
        user: "user".into(),
        path: "".into(),
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

/// A user manager `ServicesUpdate` with one unit per `(name, description)`.
fn services(units: &[(&str, &str)]) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Services(ServicesUpdate {
        manager: Manager::User,
        state: ManagerState::Connected,
        units: units
            .iter()
            .map(|&(name, description)| {
                let unit = UnitRef {
                    manager: Manager::User,
                    name: name.into(),
                };
                unit_record(unit, description)
            })
            .collect(),
        details: Vec::new(),
        backend: ServiceBackend::Systemd,
        validity: Validity::Valid,
    }))
}

/// The name of the unit row of `unit`.
fn unit_row_name(snap: &Snapshot, unit: &UnitRef) -> String {
    snap.table
        .groups
        .iter()
        .find(|g| g.kind == GroupRowKind::Unit && g.node == NodeId::Unit(unit.clone()))
        .map(|g| g.name.to_string())
        .expect("the unit has a row")
}

#[test]
fn unit_row_name_falls_back_to_unit() {
    let mut engine = Engine::new();
    let snap = engine.run(vec![grouped_query(), sweep_of(pipewire_records())]);
    assert_eq!(unit_row_name(&snap, &pipewire()), "pipewire.service");
}

#[test]
fn unit_row_name_from_description() {
    let mut engine = Engine::new();
    let before = engine.run(vec![grouped_query(), sweep_of(pipewire_records())]);
    let after = engine.run(vec![services(&[(
        "pipewire.service",
        "PipeWire Multimedia Service",
    )])]);
    assert_eq!(
        unit_row_name(&after, &pipewire()),
        "PipeWire Multimedia Service"
    );
    assert!(after.table.header.version > before.table.header.version);
    assert_eq!(
        after.processes.header.version, before.processes.header.version,
        "no new sweep"
    );

    // An empty Description keeps the stored name.
    let snap = engine.run(vec![services(&[("pipewire.service", "")])]);
    assert_eq!(
        unit_row_name(&snap, &pipewire()),
        "PipeWire Multimedia Service"
    );
}

#[test]
fn description_change_rebuilds_once() {
    let mut engine = Engine::new();
    engine.run(vec![grouped_query(), sweep_of(pipewire_records())]);
    let first = engine.run(vec![services(&[("pipewire.service", "PipeWire")])]);

    let changed = engine.run(vec![services(&[(
        "pipewire.service",
        "PipeWire Multimedia Service",
    )])]);
    assert_eq!(
        unit_row_name(&changed, &pipewire()),
        "PipeWire Multimedia Service"
    );
    assert_eq!(changed.table.header.version, first.table.header.version + 1);

    // The same names again, and a new name for a unit no process is in: no rebuild.
    let same = engine.run(vec![services(&[
        ("pipewire.service", "PipeWire Multimedia Service"),
        ("cups.service", "CUPS Scheduler"),
    ])]);
    assert_eq!(same.table.header.version, changed.table.header.version);
    assert!(Arc::ptr_eq(&same.table, &changed.table));
}

#[test]
fn unit_manager_from_cgroup_path() {
    // The sweep picks the manager from the placement of the cgroup path (E08-003).
    let manager_of = |path: &str| {
        let cg = parse_cgroup_file(format!("0::/{path}\n").as_bytes()).expect("a v2 path");
        let manager = match placement(&cg) {
            UserPlacement::InUserManager { .. } => Manager::User,
            _ => Manager::System,
        };
        (manager, cg.unit.expect("a unit").to_string())
    };
    assert_eq!(
        manager_of("system.slice/cron.service"),
        (Manager::System, "cron.service".to_string())
    );
    assert_eq!(
        manager_of(&format!("{USER_MANAGER}/session.slice/pipewire.service")),
        (Manager::User, "pipewire.service".to_string())
    );

    let records = vec![
        in_unit(rec(10, 1, "cron", 0.0), "system.slice", "cron.service"),
        in_unit(rec(11, 10, "cron", 0.0), "system.slice", "cron.service"),
    ];
    let cron = UnitRef {
        manager: Manager::System,
        name: "cron.service".into(),
    };
    let view = build(
        &records,
        &query(TableMode::Grouped, "pid", true, "", &headers()),
        &settings(),
    );
    let units: Vec<&GroupRow> = view
        .groups
        .iter()
        .filter(|g| g.kind == GroupRowKind::Unit)
        .collect();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].node, NodeId::Unit(cron));
    assert_eq!(units[0].class, GroupKind::System);
}

// Group row fields and sums (bead 8q0.38).

/// The unit row of `unit`.
fn unit_row<'a>(snap: &'a Snapshot, unit: &UnitRef) -> &'a GroupRow {
    snap.table
        .groups
        .iter()
        .find(|g| g.kind == GroupRowKind::Unit && g.node == NodeId::Unit(unit.clone()))
        .expect("the unit has a row")
}

#[test]
fn unit_row_carries_description_and_user() {
    let mut engine = Engine::new();
    let before = engine.run(vec![grouped_query(), sweep_of(pipewire_records())]);
    let row = unit_row(&before, &pipewire());
    assert_eq!(row.description, None);
    assert_eq!(row.user, None);

    let mut update = services(&[("pipewire.service", "PipeWire Multimedia Service")]);
    if let EngineMsg::Source(SourceUpdate::Services(u)) = &mut update {
        u.units[0].user = "oystein".into();
    }
    let snap = engine.run(vec![update]);
    let row = unit_row(&snap, &pipewire());
    assert_eq!(
        row.description.as_deref(),
        Some("PipeWire Multimedia Service")
    );
    assert_eq!(row.user.as_deref(), Some("oystein"));

    // A rebuild with the same values reuses the stored Arcs.
    let again = engine.run(vec![sweep_of(pipewire_records())]);
    let next = unit_row(&again, &pipewire());
    assert!(Arc::ptr_eq(
        row.description.as_ref().expect("set"),
        next.description.as_ref().expect("set")
    ));
    assert!(Arc::ptr_eq(
        row.user.as_ref().expect("set"),
        next.user.as_ref().expect("set")
    ));
}

#[test]
fn group_rows_sum_extras_columns() {
    let records = pipewire_records();
    let mut extras: Vec<ProcessExtras> = records.iter().map(extras).collect();
    for (n, x) in (1u64..).zip(extras.iter_mut()) {
        x.cpu_kernel_pct = 0.5;
        x.cpu_user_pct = 1.0;
        x.cpu_relative_pct = 16.0;
        x.utime_ticks = 100 * n;
        x.stime_ticks = 10 * n;
        x.commit_bytes = GIB * n;
        x.page_faults = 1000 * n;
        x.page_faults_delta = n;
        x.major_faults = 2 * n;
        x.handles = Some(10);
    }
    let system = SystemSnapshot::default();
    let devices = DeviceCatalog::default();
    let unit_names = FxHashMap::default();
    let settings = settings();
    let build = |extras: &[ProcessExtras]| {
        let input = TableInput {
            records: &records,
            extras,
            app_stats: &[],
            unit_stats: &[],
            system: &system,
            devices: &devices,
            settings: &settings,
            unit_names: &unit_names,
            unit_users: &unit_names,
            pseudo: None,
            mem_groups: &FxHashMap::default(),
        };
        table::build(
            &input,
            &query(TableMode::Grouped, "pid", true, "", &headers()),
        )
    };

    let view = build(&extras);
    let g = view
        .groups
        .iter()
        .find(|g| g.kind == GroupRowKind::Unit)
        .expect("the unit has a row");
    assert_eq!(g.cpu_kernel_pct, 1.5);
    assert_eq!(g.cpu_user_pct, 3.0);
    assert_eq!(g.cpu_relative_pct, 48.0);
    assert_eq!(g.utime_ticks, 600);
    assert_eq!(g.stime_ticks, 60);
    assert_eq!(g.commit_bytes, 6 * GIB);
    assert_eq!(g.page_faults, 6000);
    assert_eq!(g.page_faults_delta, 6);
    assert_eq!(g.major_faults, 12);
    assert_eq!(g.handles, Some(30));

    // One unread fd count makes the sum unknown.
    extras[1].handles = None;
    let view = build(&extras);
    let g = view
        .groups
        .iter()
        .find(|g| g.kind == GroupRowKind::Unit)
        .expect("the unit has a row");
    assert_eq!(g.handles, None);
    assert_eq!(g.commit_bytes, 6 * GIB);
}
