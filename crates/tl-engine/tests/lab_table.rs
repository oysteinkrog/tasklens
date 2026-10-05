//! Table builder tests (E09-014, ADR-013): flat mode (E09-009), tree mode (E09-010), grouped
//! mode (E09-011), hide system processes (E09-012), plus a 2,000-row timing smoke check and a
//! determinism check.
//!
//! The builder is pure (records in, rows out), so these tests need no runtime. Each test builds
//! its records with `rec` and the `in_app`, `in_unit` and `kthread` helpers, then calls `build`.
//! Records passed to `build` must be sorted by `ProcessKey`, as the reducer keeps them;
//! `rec` sets `start_ticks = pid × 10`, so ascending PIDs are sorted.

use std::sync::Arc;
use std::time::Instant;

use rustc_hash::FxHashMap;
use tl_engine::reducer::table::{self, GROUP_ORDER, TableInput};
use tl_model::actions::{CgroupRef, SchedPolicy};
use tl_model::devices::DeviceCatalog;
use tl_model::ids::{ColumnId, GroupKey, NodeId, ProcessKey};
use tl_model::process::{
    AppCgroupSample, GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessExtras,
    ProcessMeta, ProcessRecord,
};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::SystemSnapshot;
use tl_model::system::CpuSet;
use tl_model::table::{Manager, ProcessTableView, SortSpec, TableMode, TableQuery, UnitRef};
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
        unit_stats: &[],
        system: &system,
        devices: &devices,
        settings,
        unit_names: &unit_names,
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
        cgroup: CgroupRef {
            rel_path: format!("{USER_MANAGER}/app.slice/app-firefox.scope").into(),
            cgroup_id: 1,
        },
        validity: FieldValidity::all(Validity::Valid),
    }];
    let view = build_with(&records, &stats, &q, &settings());
    assert_eq!(view.groups[0].memory_bytes, GIB);
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
