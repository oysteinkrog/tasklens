//! System table builder: Services, Startup Apps and Connections (UI-SYS-002, R-138).
//!
//! The engine sorts, filters and flattens the three system tables from one `SysTableQuery`. The
//! reducer calls `build` whenever the matching component or its query changes (E09-027). E09
//! owns this file and its signatures; E14-A1 fills the Services body and the shared helpers,
//! E14-005 and E14-006 add Startup and Connections.
//!
//! Reducer code: never await, never do I/O here.

use std::cmp::Ordering;
use std::sync::Arc;

use rustc_hash::FxHashSet;
use tl_model::health::ManagerState;
use tl_model::ids::ProcessKey;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    ConnectionsSnapshot, Impact, ProcessSnapshot, ServiceCounts, ServiceStatus, ServicesSnapshot,
    StartupCounts, StartupEntry, StartupSnapshot, StartupStatus, StartupType, UnitRecord,
};
use tl_model::table::{
    ConnColumn, Manager, ServiceColumn, ServiceFilter, ServiceStateFilter, StartupColumn,
    SysColumnId, SysNodeId, SysRow, SysSortSpec, SysTable, SysTableQuery, Total, UnitRef, UnitType,
};
use tl_model::validity::Validity;

/// Everything `build` reads. Borrowed from the reducer's current state.
pub struct SysTableInput<'a> {
    /// Units, managers, details and backend.
    pub services: &'a ServicesSnapshot,
    /// Startup entries.
    pub startup: &'a StartupSnapshot,
    /// Socket records.
    pub connections: &'a ConnectionsSnapshot,
    /// For the child process rows under a unit.
    pub processes: &'a ProcessSnapshot,
    pub settings: &'a EngineSettings,
}

/// The visible rows of one system table, in display order.
pub struct SysTableOutput {
    pub rows: Arc<[SysRow]>,
    /// Echoes `SysTableQuery::generation` so the UI can tell a stale table.
    pub query_generation: u64,
}

/// The rows of `query.table`. Connections returns no rows until E14-006.
pub fn build(input: &SysTableInput<'_>, query: &SysTableQuery) -> SysTableOutput {
    match query.table {
        SysTable::Services => build_services(input, query).0,
        SysTable::Startup => build_startup(input, query).0,
        SysTable::Connections => finish(Vec::new(), query),
    }
}

/// The default sort of each table (R-011): Services Name ascending, Startup Status ascending,
/// Connections Proto ascending.
///
/// `SysSortSpec` holds one column. Two tables also have a fixed secondary sort that `build`
/// applies after the primary column: Startup then sorts by Name ascending, Connections by local
/// port ascending.
pub fn default_sort(table: SysTable) -> SysSortSpec {
    let column = match table {
        SysTable::Services => SysColumnId::Services(ServiceColumn::Name),
        SysTable::Startup => SysColumnId::Startup(StartupColumn::Status),
        SysTable::Connections => SysColumnId::Connections(ConnColumn::Proto),
    };
    SysSortSpec {
        column,
        ascending: true,
    }
}

// ---- Shared helpers (all three tables) ----

/// The output rows, stamped with the query's generation.
fn finish(rows: Vec<SysRow>, query: &SysTableQuery) -> SysTableOutput {
    SysTableOutput {
        rows: rows.into(),
        query_generation: query.generation,
    }
}

/// Stable sort of record indexes by `primary`, with a final tie-break on the node id order
/// `node`, so rows with equal sort values keep one order across rebuilds.
fn sort_indices(
    idx: &mut [u32],
    primary: impl Fn(usize, usize) -> Ordering,
    node: impl Fn(usize, usize) -> Ordering,
) {
    idx.sort_by(|&a, &b| {
        let (a, b) = (a as usize, b as usize);
        primary(a, b).then_with(|| node(a, b))
    });
}

/// `ord` in the chosen direction.
fn directed(ord: Ordering, ascending: bool) -> Ordering {
    if ascending { ord } else { ord.reverse() }
}

/// Order of two optional sort values in the chosen direction. `None` sorts last in both
/// directions.
fn absent_last<T>(
    a: Option<T>,
    b: Option<T>,
    ascending: bool,
    cmp: impl FnOnce(T, T) -> Ordering,
) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => directed(cmp(a, b), ascending),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// The value when its validity is `Valid`, else `None`, so it sorts last.
fn valid<T: Copy>(t: &Total<T>) -> Option<T> {
    (t.validity == Validity::Valid).then_some(t.value)
}

/// Case-insensitive text order, without allocating. Ties break by byte order, so the order
/// stays total.
fn cmp_text(a: &str, b: &str) -> Ordering {
    // ASCII fast path. At the first non-ASCII byte the whole pair goes to the Unicode path,
    // which orders ASCII text the same way.
    let (x, y) = (a.as_bytes(), b.as_bytes());
    // Equal bytes are equal ignoring case. A first difference at ASCII bytes sits on a char
    // boundary; at any other byte the loop below hands over to the Unicode path.
    let same = x.iter().zip(y).take_while(|(p, q)| p == q).count();
    for (&p, &q) in x[same..].iter().zip(&y[same..]) {
        if !p.is_ascii() || !q.is_ascii() {
            return cmp_text_unicode(a, b);
        }
        let o = p.to_ascii_lowercase().cmp(&q.to_ascii_lowercase());
        if o.is_ne() {
            return o;
        }
    }
    x.len().cmp(&y.len()).then_with(|| a.cmp(b))
}

fn cmp_text_unicode(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
        .then_with(|| a.cmp(b))
}

/// True when `needle`, already lowercase, occurs in `hay` ignoring case. ASCII text, the common
/// case, is matched without allocating. An empty needle matches everything.
fn contains_lower(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if hay.is_ascii() {
        let (h, n) = (hay.as_bytes(), needle.as_bytes());
        let Some(last) = h.len().checked_sub(n.len()) else {
            return false;
        };
        (0..=last).any(|i| {
            h[i].to_ascii_lowercase() == n[0]
                && h[i + 1..i + n.len()]
                    .iter()
                    .zip(&n[1..])
                    .all(|(h, n)| h.to_ascii_lowercase() == *n)
        })
    } else {
        hay.to_lowercase().contains(needle)
    }
}

/// `n` in decimal, written into `buf`.
fn decimal(mut n: u32, buf: &mut [u8; 10]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    std::str::from_utf8(&buf[i..]).unwrap_or_default()
}

// ---- Services ----

/// The Services rows plus the `ServicesSnapshot.counts` for the same query, from one pass.
/// `build` keeps only the rows; the reducer calls this to store both.
///
/// Counts cover unit rows after all filters, including units of collapsed groups (UI-SYS-022).
pub fn build_services(
    input: &SysTableInput<'_>,
    query: &SysTableQuery,
) -> (SysTableOutput, ServiceCounts) {
    let default_filter;
    let filter = match &query.services {
        Some(f) => f,
        None => {
            default_filter = ServiceFilter::default();
            &default_filter
        }
    };
    let text: &str = &query.filter;
    let (column, ascending) = match query.sort.column {
        SysColumnId::Services(c) => (c, query.sort.ascending),
        _ => (ServiceColumn::Name, true),
    };

    let mut group_open = [false; 2];
    let mut unit_open: FxHashSet<&UnitRef> = FxHashSet::default();
    for node in query.expanded.iter() {
        match node {
            SysNodeId::ServiceGroup(m) => group_open[manager_slot(*m)] = true,
            SysNodeId::Unit(u) => {
                unit_open.insert(u);
            }
            _ => {}
        }
    }

    let units = &input.services.units;
    let mut rows = Vec::with_capacity(units.len() + 2);
    let mut counts = ServiceCounts::default();
    let mut members: Vec<u32> = Vec::new();
    let mut children: Vec<(ProcessKey, u32)> = Vec::new();

    for manager in [Manager::System, Manager::User] {
        let slot = manager_slot(manager);
        let shown = match manager {
            Manager::System => filter.system,
            Manager::User => filter.user,
        };
        if !shown {
            continue;
        }
        let group = SysNodeId::ServiceGroup(manager);
        if input.services.managers[slot] == ManagerState::Unavailable {
            rows.push(SysRow {
                node: group,
                depth: 0,
                expanded: group_open[slot],
                has_children: false,
                record: None,
            });
            continue;
        }

        members.clear();
        for (i, u) in units.iter().enumerate() {
            if u.unit.manager == manager && unit_passes(u, filter, text) {
                count_unit(&mut counts, u.status);
                members.push(i as u32);
            }
        }
        // UI-SYS-014: a group the filters empty is hidden.
        if members.is_empty() {
            continue;
        }
        rows.push(SysRow {
            node: group,
            depth: 0,
            expanded: group_open[slot],
            has_children: true,
            record: None,
        });
        if !group_open[slot] {
            continue;
        }

        sort_indices(
            &mut members,
            |a, b| cmp_units(&units[a], &units[b], column, ascending),
            |a, b| units[a].unit.cmp(&units[b].unit),
        );
        for &i in &members {
            let u = &units[i as usize];
            let open = !unit_open.is_empty() && unit_open.contains(&u.unit);
            rows.push(SysRow {
                node: SysNodeId::Unit(u.unit.clone()),
                depth: 1,
                expanded: open,
                has_children: u.has_cgroup_procs,
                record: Some(i),
            });
            if open {
                push_children(&mut rows, &mut children, u, input.processes);
            }
        }
    }

    (finish(rows, query), counts)
}

/// Index of `manager` in `ServicesSnapshot.managers`: 0 System, 1 User.
fn manager_slot(manager: Manager) -> usize {
    match manager {
        Manager::System => 0,
        Manager::User => 1,
    }
}

/// Child process rows of an expanded unit: each `cgroup_pids` key found in the process
/// snapshot, by PID ascending whatever the query sort (UI-SYS-013).
fn push_children(
    rows: &mut Vec<SysRow>,
    scratch: &mut Vec<(ProcessKey, u32)>,
    unit: &UnitRecord,
    processes: &ProcessSnapshot,
) {
    scratch.clear();
    for &key in unit.cgroup_pids.iter() {
        if let Ok(r) = processes.records.binary_search_by(|p| p.key.cmp(&key)) {
            scratch.push((key, r as u32));
        }
    }
    scratch.sort_unstable_by_key(|&(key, _)| (key.pid, key));
    for &(key, r) in scratch.iter() {
        rows.push(SysRow {
            node: SysNodeId::UnitProcess(unit.unit.clone(), key),
            depth: 2,
            expanded: false,
            has_children: false,
            record: Some(r),
        });
    }
}

/// Unit type AND state AND text filter. The manager toggle is applied by the caller.
fn unit_passes(u: &UnitRecord, filter: &ServiceFilter, text: &str) -> bool {
    filter.unit_types.contains(&u.kind)
        && (filter.states.is_empty() || filter.states.contains(&state_class(u)))
        && (text.is_empty() || unit_text_matches(u, text))
}

/// SYS-004 classes, in Mission Center order: running (ActiveState `active` or `reloading`),
/// failed, stopped (enabled and not running), otherwise disabled.
fn state_class(u: &UnitRecord) -> ServiceStateFilter {
    match u.status {
        ServiceStatus::Running | ServiceStatus::Active | ServiceStatus::Reloading => {
            ServiceStateFilter::Running
        }
        ServiceStatus::Failed => ServiceStateFilter::Failed,
        _ if matches!(
            u.startup,
            StartupType::Enabled | StartupType::EnabledRuntime
        ) =>
        {
            ServiceStateFilter::Stopped
        }
        _ => ServiceStateFilter::Disabled,
    }
}

/// UI-SYS-022: running counts ActiveState `active` and `reloading`, stopped counts `inactive`
/// and `failed`; every other state counts in the total only.
fn count_unit(counts: &mut ServiceCounts, status: ServiceStatus) {
    counts.total += 1;
    match status {
        ServiceStatus::Running | ServiceStatus::Active | ServiceStatus::Reloading => {
            counts.running += 1;
        }
        ServiceStatus::Stopped | ServiceStatus::Failed => counts.stopped += 1,
        ServiceStatus::Starting | ServiceStatus::Stopping | ServiceStatus::Maintenance => {}
    }
}

/// True when the lowercase `text` occurs in any text column of the unit (APP-028, R-167).
///
/// Active since is not matched: the reducer has no time zone, and the column shows local time
/// (logged for the owner).
fn unit_text_matches(u: &UnitRecord, text: &str) -> bool {
    let mut pid = [0u8; 10];
    contains_lower(&u.unit.name, text)
        || contains_lower(&u.description, text)
        || contains_lower(status_word(u.status), text)
        || contains_lower(startup_word(u.startup), text)
        || (u.main_pid != 0 && contains_lower(decimal(u.main_pid, &mut pid), text))
        || contains_lower(&u.user, text)
        || contains_lower(&u.path, text)
        || contains_lower(manager_word(u.unit.manager), text)
        || contains_lower(unit_type_word(u.kind), text)
        || contains_lower(&u.sub_state, text)
        || contains_lower(&u.load_state, text)
        || u.unit_file
            .as_deref()
            .is_some_and(|f| contains_lower(f, text))
        || u.triggered_by.iter().any(|t| contains_lower(t, text))
}

/// The Status column word (SYS-003).
fn status_word(s: ServiceStatus) -> &'static str {
    match s {
        ServiceStatus::Running => "Running",
        ServiceStatus::Active => "Active",
        ServiceStatus::Starting => "Starting",
        ServiceStatus::Stopping => "Stopping",
        ServiceStatus::Reloading => "Reloading",
        ServiceStatus::Stopped => "Stopped",
        ServiceStatus::Failed => "Failed",
        ServiceStatus::Maintenance => "Maintenance",
    }
}

/// The Startup type column word (SYS-003).
fn startup_word(s: StartupType) -> &'static str {
    match s {
        StartupType::Enabled => "Enabled",
        StartupType::EnabledRuntime => "Enabled (runtime)",
        StartupType::Disabled => "Disabled",
        StartupType::Static => "Static",
        StartupType::Indirect => "Indirect",
        StartupType::Masked => "Masked",
        StartupType::Generated => "Generated",
        StartupType::Transient => "Transient",
        StartupType::Alias => "Alias",
        StartupType::Linked => "Linked",
        StartupType::NoUnitFile => "-",
    }
}

/// The Manager column word (UI-SYS-013).
fn manager_word(m: Manager) -> &'static str {
    match m {
        Manager::System => "System",
        Manager::User => "User",
    }
}

/// The Unit type column word (UI-SYS-013).
fn unit_type_word(t: UnitType) -> &'static str {
    match t {
        UnitType::Service => "Service",
        UnitType::Socket => "Socket",
        UnitType::Timer => "Timer",
        UnitType::Mount => "Mount",
        UnitType::Automount => "Automount",
        UnitType::Path => "Path",
        UnitType::Target => "Target",
        UnitType::Swap => "Swap",
        UnitType::Slice => "Slice",
        UnitType::Scope => "Scope",
    }
}

/// Order of two units in `column`. Text columns compare case-insensitively; numbers
/// numerically with non-`Valid` values last; Status and Startup type in the order of their
/// SYS-003 mapping tables, which is the enum order.
fn cmp_units(a: &UnitRecord, b: &UnitRecord, column: ServiceColumn, ascending: bool) -> Ordering {
    use ServiceColumn as C;
    let text = |x: &str, y: &str| directed(cmp_text(x, y), ascending);
    match column {
        C::Name => text(&a.unit.name, &b.unit.name),
        C::DisplayName | C::Description => text(&a.description, &b.description),
        C::Status => directed((a.status as u8).cmp(&(b.status as u8)), ascending),
        C::StartupType => directed((a.startup as u8).cmp(&(b.startup as u8)), ascending),
        C::Pid => directed(a.main_pid.cmp(&b.main_pid), ascending),
        C::User => text(&a.user, &b.user),
        C::Path => text(&a.path, &b.path),
        C::Manager => text(manager_word(a.unit.manager), manager_word(b.unit.manager)),
        C::UnitType => text(unit_type_word(a.kind), unit_type_word(b.kind)),
        C::SubState => text(&a.sub_state, &b.sub_state),
        C::LoadState => text(&a.load_state, &b.load_state),
        C::Cpu => absent_last(
            valid(&a.cpu_pct).filter(|v| !v.is_nan()),
            valid(&b.cpu_pct).filter(|v| !v.is_nan()),
            ascending,
            |x: f32, y: f32| x.total_cmp(&y),
        ),
        C::Memory => absent_last(
            valid(&a.memory_bytes),
            valid(&b.memory_bytes),
            ascending,
            |x: u64, y: u64| x.cmp(&y),
        ),
        C::Disk => absent_last(
            valid(&a.disk_bps).filter(|v| !v.is_nan()),
            valid(&b.disk_bps).filter(|v| !v.is_nan()),
            ascending,
            |x: f64, y: f64| x.total_cmp(&y),
        ),
        C::Tasks => absent_last(
            valid(&a.tasks),
            valid(&b.tasks),
            ascending,
            |x: u64, y: u64| x.cmp(&y),
        ),
        C::ActiveSince => absent_last(
            a.active_since_us,
            b.active_since_us,
            ascending,
            |x: u64, y: u64| x.cmp(&y),
        ),
        C::UnitFile => absent_last(
            a.unit_file.as_deref(),
            b.unit_file.as_deref(),
            ascending,
            cmp_text,
        ),
        C::TriggeredBy => directed(cmp_lists(&a.triggered_by, &b.triggered_by), ascending),
    }
}

/// Element-wise `cmp_text` order of two id lists; a shorter prefix sorts first.
fn cmp_lists(a: &[Arc<str>], b: &[Arc<str>]) -> Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| cmp_text(x, y))
        .find(|o| o.is_ne())
        .unwrap_or_else(|| a.len().cmp(&b.len()))
}

// ---- Startup Apps ----

/// The Startup rows plus the `StartupSnapshot.counts` for the same query, from one pass.
/// `build` keeps only the rows; the reducer calls this to store both.
///
/// Counts cover the rows left after the text filter (UI-SYS-049).
pub fn build_startup(
    input: &SysTableInput<'_>,
    query: &SysTableQuery,
) -> (SysTableOutput, StartupCounts) {
    let text: &str = &query.filter;
    let (column, ascending) = match query.sort.column {
        SysColumnId::Startup(c) => (c, query.sort.ascending),
        _ => (StartupColumn::Status, true),
    };
    let entries = &input.startup.entries;

    let mut idx: Vec<u32> = Vec::with_capacity(entries.len());
    let mut counts = StartupCounts::default();
    for (i, e) in entries.iter().enumerate() {
        if text.is_empty() || startup_text_matches(e, text) {
            counts.total += 1;
            if e.status == StartupStatus::Enabled {
                counts.enabled += 1;
            }
            idx.push(i as u32);
        }
    }
    counts.disabled = counts.total - counts.enabled;

    // R-011: Name ascending is the fixed secondary sort, then the `StartupRef`.
    sort_indices(
        &mut idx,
        |a, b| {
            cmp_startup(&entries[a], &entries[b], column, ascending)
                .then_with(|| cmp_text(&entries[a].name, &entries[b].name))
        },
        |a, b| entries[a].id.cmp(&entries[b].id),
    );
    let rows = idx
        .into_iter()
        .map(|i| SysRow {
            node: SysNodeId::Startup(entries[i as usize].id.clone()),
            depth: 0,
            expanded: false,
            has_children: false,
            record: Some(i),
        })
        .collect();
    (finish(rows, query), counts)
}

/// True when the lowercase `text` occurs in any of the seven Startup columns (APP-028).
/// Location is matched on the raw path, not the `~` form the UI shows.
fn startup_text_matches(e: &StartupEntry, text: &str) -> bool {
    contains_lower(&e.name, text)
        || e.publisher
            .as_deref()
            .is_some_and(|p| contains_lower(p, text))
        || contains_lower(e.status.label(), text)
        || contains_lower(impact_word(e.impact), text)
        || contains_lower(e.kind.label(), text)
        || contains_lower(&e.location, text)
        || contains_lower(&e.command, text)
}

/// The Impact column word (SYS-021).
fn impact_word(i: Impact) -> &'static str {
    match i {
        Impact::High => "High",
        Impact::Medium => "Medium",
        Impact::Low => "Low",
        Impact::None => "None",
        Impact::NotMeasured => "Not measured",
    }
}

/// Order of two entries in `column`. Text columns compare case-insensitively, a blank
/// publisher last; Status and Impact in their enum order (UI-SYS-041).
fn cmp_startup(
    a: &StartupEntry,
    b: &StartupEntry,
    column: StartupColumn,
    ascending: bool,
) -> Ordering {
    use StartupColumn as C;
    let text = |x: &str, y: &str| directed(cmp_text(x, y), ascending);
    match column {
        C::Name => text(&a.name, &b.name),
        C::Publisher => absent_last(
            a.publisher.as_deref(),
            b.publisher.as_deref(),
            ascending,
            cmp_text,
        ),
        C::Status => directed((a.status as u8).cmp(&(b.status as u8)), ascending),
        C::Impact => directed(a.impact.cmp(&b.impact), ascending),
        C::Type => text(a.kind.label(), b.kind.label()),
        C::Location => text(&a.location, &b.location),
        C::Command => text(&a.command, &b.command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    use smallvec::SmallVec;
    use tl_model::ids::GroupKey;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::snapshot::StartupKind;
    use tl_model::table::{StartupRef, StartupSource};
    use tl_model::time::BootId;
    use tl_model::validity::FieldValidity;

    fn unit(manager: Manager, name: &str, status: ServiceStatus) -> UnitRecord {
        UnitRecord {
            unit: UnitRef {
                manager,
                name: name.into(),
            },
            kind: UnitType::from_unit_name(name).unwrap_or(UnitType::Service),
            description: "".into(),
            status,
            sub_state: "".into(),
            load_state: "loaded".into(),
            startup: StartupType::Static,
            main_pid: 0,
            user: "root".into(),
            path: "".into(),
            unit_file: None,
            active_since_us: None,
            triggered_by: Arc::from([]),
            control_group: None,
            can_reload: false,
            has_cgroup_procs: false,
            cgroup_pids: Arc::from([]),
            cpu_pct: Total {
                value: 0.0,
                validity: Validity::Missing,
            },
            memory_bytes: Total {
                value: 0,
                validity: Validity::Missing,
            },
            disk_bps: Total {
                value: 0.0,
                validity: Validity::Missing,
            },
            tasks: Total {
                value: 0,
                validity: Validity::Missing,
            },
        }
    }

    fn sys(name: &str) -> UnitRecord {
        unit(Manager::System, name, ServiceStatus::Running)
    }

    fn usr(name: &str) -> UnitRecord {
        unit(Manager::User, name, ServiceStatus::Running)
    }

    fn key(pid: u32, start_ticks: u64) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BootId(1),
            pid,
        }
    }

    fn proc_rec(key: ProcessKey) -> ProcessRecord {
        ProcessRecord {
            key,
            ppid: 1,
            uid: 0,
            meta: Arc::new(ProcessMeta {
                comm: "p".into(),
                name: "p".into(),
                exe: None,
                cmdline: None,
                user: "root".into(),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                search_lower: "p".into(),
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
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn snapshot(units: Vec<UnitRecord>) -> ServicesSnapshot {
        ServicesSnapshot {
            units: units.into(),
            managers: [ManagerState::Connected; 2],
            ..ServicesSnapshot::default()
        }
    }

    fn query(table: SysTable, generation: u64) -> SysTableQuery {
        SysTableQuery {
            table,
            generation,
            sort: default_sort(table),
            filter: Arc::from(""),
            expanded: Arc::from([]),
            services: None,
        }
    }

    /// A Services query with both groups expanded.
    fn open_query() -> SysTableQuery {
        SysTableQuery {
            expanded: Arc::from([
                SysNodeId::ServiceGroup(Manager::System),
                SysNodeId::ServiceGroup(Manager::User),
            ]),
            ..query(SysTable::Services, 1)
        }
    }

    fn with_states(states: &[ServiceStateFilter]) -> SysTableQuery {
        SysTableQuery {
            services: Some(ServiceFilter {
                states: SmallVec::from_slice(states),
                ..ServiceFilter::default()
            }),
            ..open_query()
        }
    }

    fn run_with(
        services: &ServicesSnapshot,
        processes: &ProcessSnapshot,
        q: &SysTableQuery,
    ) -> (SysTableOutput, ServiceCounts) {
        let startup = StartupSnapshot::default();
        let connections = ConnectionsSnapshot::default();
        let settings = EngineSettings::default();
        let input = SysTableInput {
            services,
            startup: &startup,
            connections: &connections,
            processes,
            settings: &settings,
        };
        build_services(&input, q)
    }

    fn run(services: &ServicesSnapshot, q: &SysTableQuery) -> (SysTableOutput, ServiceCounts) {
        run_with(services, &ProcessSnapshot::default(), q)
    }

    /// Unit names of the unit rows, in order.
    fn unit_names(out: &SysTableOutput) -> Vec<&str> {
        out.rows
            .iter()
            .filter_map(|r| match &r.node {
                SysNodeId::Unit(u) => Some(&*u.name),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn services_rows_order_and_depth() {
        let s = snapshot(vec![
            usr("b.service"),
            sys("c.service"),
            sys("a.service"),
            usr("a.service"),
            sys("b.service"),
        ]);
        let (out, _) = run(&s, &open_query());
        let depths: Vec<u8> = out.rows.iter().map(|r| r.depth).collect();
        assert_eq!(depths, [0, 1, 1, 1, 0, 1, 1]);
        assert_eq!(out.rows[0].node, SysNodeId::ServiceGroup(Manager::System));
        assert_eq!(out.rows[4].node, SysNodeId::ServiceGroup(Manager::User));
        assert!(out.rows[0].expanded && out.rows[0].has_children);
        assert_eq!(
            unit_names(&out),
            [
                "a.service",
                "b.service",
                "c.service",
                "a.service",
                "b.service"
            ]
        );
        // `record` indexes the published unit array.
        assert_eq!(out.rows[1].record, Some(2));
        assert_eq!(out.rows[5].record, Some(3));
    }

    #[test]
    fn services_collapsed_group_counts() {
        let s = snapshot(vec![
            sys("a.service"),
            sys("b.service"),
            sys("c.service"),
            usr("d.service"),
            usr("e.service"),
        ]);
        let q = SysTableQuery {
            expanded: Arc::from([SysNodeId::ServiceGroup(Manager::User)]),
            ..query(SysTable::Services, 1)
        };
        let (out, counts) = run(&s, &q);
        assert_eq!(out.rows[0].node, SysNodeId::ServiceGroup(Manager::System));
        assert!(!out.rows[0].expanded);
        assert!(out.rows[0].has_children);
        assert_eq!(unit_names(&out), ["d.service", "e.service"]);
        assert_eq!(counts.total, 5);
        assert_eq!(counts.running, 5);
    }

    #[test]
    fn services_state_filter_or() {
        let mut enabled = unit(Manager::System, "c.service", ServiceStatus::Stopped);
        enabled.startup = StartupType::Enabled;
        let s = snapshot(vec![
            unit(Manager::System, "a.service", ServiceStatus::Running),
            unit(Manager::System, "b.service", ServiceStatus::Failed),
            enabled,
            unit(Manager::System, "d.service", ServiceStatus::Stopped),
            unit(Manager::User, "e.service", ServiceStatus::Failed),
        ]);
        let (out, _) = run(&s, &with_states(&[ServiceStateFilter::Failed]));
        assert_eq!(unit_names(&out), ["b.service", "e.service"]);

        let (out, _) = run(
            &s,
            &with_states(&[ServiceStateFilter::Running, ServiceStateFilter::Stopped]),
        );
        assert_eq!(unit_names(&out), ["a.service", "c.service"]);

        let (out, _) = run(&s, &with_states(&[ServiceStateFilter::Disabled]));
        assert_eq!(unit_names(&out), ["d.service"]);

        // No state selected means all states.
        let (out, _) = run(&s, &with_states(&[]));
        assert_eq!(unit_names(&out).len(), 5);
    }

    #[test]
    fn services_state_and_text_filter() {
        let mut agent = unit(Manager::User, "agent.socket", ServiceStatus::Failed);
        agent.description = "SSH key agent".into();
        let s = snapshot(vec![
            unit(Manager::System, "sshd.service", ServiceStatus::Running),
            unit(Manager::System, "sshfs.service", ServiceStatus::Stopped),
            unit(Manager::System, "cups.service", ServiceStatus::Running),
            agent,
        ]);
        let q = SysTableQuery {
            filter: Arc::from("ssh"),
            ..with_states(&[ServiceStateFilter::Running, ServiceStateFilter::Failed])
        };
        let (out, counts) = run(&s, &q);
        assert_eq!(unit_names(&out), ["sshd.service", "agent.socket"]);
        assert_eq!(counts.total, 2);
    }

    #[test]
    fn services_text_filter_columns() {
        let mut u = sys("a.service");
        u.main_pid = 4321;
        u.unit_file = Some("/usr/lib/systemd/system/a.service".into());
        u.triggered_by = Arc::from([Arc::from("a.timer")]);
        u.startup = StartupType::EnabledRuntime;
        let s = snapshot(vec![u]);
        for text in [
            "432",
            "/usr/lib",
            "a.timer",
            "(runtime)",
            "running",
            "system",
            "service",
        ] {
            let q = SysTableQuery {
                filter: Arc::from(text),
                ..open_query()
            };
            assert_eq!(unit_names(&run(&s, &q).0).len(), 1, "filter {text:?}");
        }
        let q = SysTableQuery {
            filter: Arc::from("nomatch"),
            ..open_query()
        };
        assert!(unit_names(&run(&s, &q).0).is_empty());
    }

    #[test]
    fn services_text_filter_skips_active_since() {
        let mut u = sys("a.service");
        // 2024-01-15 12:00:00 UTC.
        u.active_since_us = Some(1_705_320_000_000_000);
        let s = snapshot(vec![u]);
        for text in ["2024-01-15", "12:00:00", "2024-01-15 12:00:00"] {
            let q = SysTableQuery {
                filter: Arc::from(text),
                ..open_query()
            };
            assert!(unit_names(&run(&s, &q).0).is_empty(), "filter {text:?}");
        }
    }

    #[test]
    fn services_unit_type_filter() {
        let s = snapshot(vec![sys("a.service"), sys("b.timer"), sys("c.socket")]);
        let q = SysTableQuery {
            services: Some(ServiceFilter {
                unit_types: SmallVec::from_slice(&[UnitType::Timer]),
                ..ServiceFilter::default()
            }),
            ..open_query()
        };
        let (out, _) = run(&s, &q);
        assert_eq!(unit_names(&out), ["b.timer"]);

        // Default types: service, socket, timer, mount.
        let s = snapshot(vec![sys("a.service"), sys("b.target")]);
        assert_eq!(unit_names(&run(&s, &open_query()).0), ["a.service"]);
    }

    #[test]
    fn services_manager_toggle_off() {
        let s = snapshot(vec![sys("a.service"), usr("b.service")]);
        let q = SysTableQuery {
            services: Some(ServiceFilter {
                system: false,
                ..ServiceFilter::default()
            }),
            ..open_query()
        };
        let (out, counts) = run(&s, &q);
        assert_eq!(out.rows[0].node, SysNodeId::ServiceGroup(Manager::User));
        assert_eq!(unit_names(&out), ["b.service"]);
        assert_eq!(counts.total, 1);
    }

    #[test]
    fn services_unavailable_group_row() {
        let mut s = snapshot(vec![sys("a.service"), usr("b.service")]);
        s.managers[1] = ManagerState::Unavailable;
        let (out, counts) = run(&s, &open_query());
        let user = out
            .rows
            .iter()
            .find(|r| r.node == SysNodeId::ServiceGroup(Manager::User))
            .expect("user group row");
        assert!(!user.has_children);
        assert_eq!(unit_names(&out), ["a.service"]);
        assert_eq!(counts.total, 1);
    }

    #[test]
    fn services_empty_group_hidden() {
        let s = snapshot(vec![sys("sshd.service"), usr("cups.service")]);
        let q = SysTableQuery {
            filter: Arc::from("sshd"),
            ..open_query()
        };
        let (out, _) = run(&s, &q);
        assert!(
            !out.rows
                .iter()
                .any(|r| r.node == SysNodeId::ServiceGroup(Manager::User))
        );
        assert_eq!(out.rows.len(), 2);
    }

    #[test]
    fn services_child_rows_by_pid() {
        // Key order (start_ticks first) differs from PID order.
        let (k300, k100, k200, gone) = (key(300, 1), key(100, 2), key(200, 3), key(50, 4));
        let mut u = sys("a.service");
        u.has_cgroup_procs = true;
        u.cgroup_pids = Arc::from([k300, k100, k200, gone]);
        let s = snapshot(vec![u, sys("b.service")]);
        let processes = ProcessSnapshot {
            records: Arc::from([proc_rec(k300), proc_rec(k100), proc_rec(k200)]),
            ..ProcessSnapshot::default()
        };
        let a = UnitRef {
            manager: Manager::System,
            name: "a.service".into(),
        };
        let q = SysTableQuery {
            sort: SysSortSpec {
                column: SysColumnId::Services(ServiceColumn::Name),
                ascending: false,
            },
            expanded: Arc::from([
                SysNodeId::ServiceGroup(Manager::System),
                SysNodeId::Unit(a.clone()),
            ]),
            ..query(SysTable::Services, 1)
        };
        let (out, counts) = run_with(&s, &processes, &q);
        assert_eq!(unit_names(&out), ["b.service", "a.service"]);
        let unit_row = &out.rows[2];
        assert!(unit_row.expanded && unit_row.has_children);
        let children: Vec<(u32, Option<u32>)> = out
            .rows
            .iter()
            .filter_map(|r| match &r.node {
                SysNodeId::UnitProcess(u, k) if *u == a => Some((k.pid, r.record)),
                _ => None,
            })
            .collect();
        assert_eq!(children, [(100, Some(1)), (200, Some(2)), (300, Some(0))]);
        assert!(out.rows[3..].iter().all(|r| r.depth == 2));
        assert_eq!(counts.total, 2, "child rows are not counted");
    }

    #[test]
    fn services_numeric_sort() {
        let mut units = Vec::new();
        for (name, pid) in [
            ("a.service", 1000),
            ("b.service", 0),
            ("c.service", 99),
            ("d.service", 5),
        ] {
            let mut u = sys(name);
            u.main_pid = pid;
            units.push(u);
        }
        let s = snapshot(units);
        let q = SysTableQuery {
            sort: SysSortSpec {
                column: SysColumnId::Services(ServiceColumn::Pid),
                ascending: true,
            },
            ..open_query()
        };
        let (out, _) = run(&s, &q);
        assert_eq!(
            unit_names(&out),
            ["b.service", "d.service", "c.service", "a.service"]
        );
    }

    #[test]
    fn services_invalid_values_sort_last() {
        let mut units = Vec::new();
        for (name, mem, validity) in [
            ("a.service", 10, Validity::Missing),
            ("b.service", 300, Validity::Valid),
            ("c.service", 20, Validity::WarmingUp),
            ("d.service", 100, Validity::Valid),
        ] {
            let mut u = sys(name);
            u.memory_bytes = Total {
                value: mem,
                validity,
            };
            units.push(u);
        }
        let s = snapshot(units);
        for (ascending, want) in [
            (true, ["d.service", "b.service", "a.service", "c.service"]),
            (false, ["b.service", "d.service", "a.service", "c.service"]),
        ] {
            let q = SysTableQuery {
                sort: SysSortSpec {
                    column: SysColumnId::Services(ServiceColumn::Memory),
                    ascending,
                },
                ..open_query()
            };
            assert_eq!(unit_names(&run(&s, &q).0), want);
        }
    }

    #[test]
    fn services_counts_rules() {
        let s = snapshot(vec![
            unit(Manager::System, "a.service", ServiceStatus::Running),
            unit(Manager::System, "b.service", ServiceStatus::Reloading),
            unit(Manager::System, "c.service", ServiceStatus::Starting),
            unit(Manager::User, "d.service", ServiceStatus::Stopped),
            unit(Manager::User, "e.service", ServiceStatus::Failed),
        ]);
        let (_, counts) = run(&s, &query(SysTable::Services, 1));
        assert_eq!(
            counts,
            ServiceCounts {
                total: 5,
                running: 2,
                stopped: 2,
            }
        );
    }

    #[test]
    fn sys_query_generation_stamped() {
        let services = snapshot(vec![sys("a.service")]);
        let startup = StartupSnapshot::default();
        let connections = ConnectionsSnapshot::default();
        let processes = ProcessSnapshot::default();
        let settings = EngineSettings::default();
        let input = SysTableInput {
            services: &services,
            startup: &startup,
            connections: &connections,
            processes: &processes,
            settings: &settings,
        };
        for table in [SysTable::Services, SysTable::Startup, SysTable::Connections] {
            assert_eq!(build(&input, &query(table, 7)).query_generation, 7);
        }
        assert_eq!(build(&input, &query(SysTable::Services, 7)).rows.len(), 1);
        assert!(build(&input, &query(SysTable::Startup, 7)).rows.is_empty());
    }

    #[test]
    fn services_projection_10k_units_under_5ms() {
        let mut units = Vec::with_capacity(10_000);
        for i in 0..10_000u32 {
            let manager = if i % 2 == 0 {
                Manager::System
            } else {
                Manager::User
            };
            let mut u = unit(
                manager,
                &format!("unit-{:05}.service", (i * 7919) % 10_000),
                ServiceStatus::Running,
            );
            u.description = format!("Synthetic Unit {i}").into();
            u.main_pid = i;
            units.push(u);
        }
        let s = snapshot(units);
        let q = open_query();
        let filtered = SysTableQuery {
            filter: Arc::from("unit 9"),
            ..open_query()
        };
        // Best of 5, so a busy build machine does not fail the check.
        let best = |q: &SysTableQuery| {
            (0..5)
                .map(|_| {
                    let start = Instant::now();
                    let out = run(&s, q);
                    (start.elapsed(), out.0.rows.len())
                })
                .min()
                .expect("5 runs")
        };
        let (plain, len) = best(&q);
        let (with_text, _) = best(&filtered);
        assert_eq!(len, 10_002);
        eprintln!("10k units: {plain:?} sorted by name, {with_text:?} with a text filter");
        if !cfg!(debug_assertions) {
            assert!(plain.as_millis() < 5, "{plain:?}");
            assert!(with_text.as_millis() < 5, "{with_text:?}");
        }
    }

    // ---- Startup Apps ----

    fn entry(name: &str, status: StartupStatus) -> StartupEntry {
        StartupEntry {
            id: StartupRef {
                source: StartupSource::AutostartUser,
                id: format!("{name}.desktop").into(),
            },
            name: name.into(),
            icon_path: None,
            publisher: None,
            status,
            impact: Impact::NotMeasured,
            impact_detail: None,
            state_code: None,
            kind: StartupKind::AutostartUser,
            location: "/home/u/.config/autostart".into(),
            command: name.into(),
            source_path: format!("/home/u/.config/autostart/{name}.desktop").into(),
            can_toggle: true,
            can_delete: true,
            can_run: true,
            overridden_by_user: false,
        }
    }

    fn run_startup(entries: Vec<StartupEntry>, q: &SysTableQuery) -> (Vec<String>, StartupCounts) {
        let services = ServicesSnapshot::default();
        let startup = StartupSnapshot {
            entries: entries.into(),
            ..StartupSnapshot::default()
        };
        let connections = ConnectionsSnapshot::default();
        let processes = ProcessSnapshot::default();
        let settings = EngineSettings::default();
        let input = SysTableInput {
            services: &services,
            startup: &startup,
            connections: &connections,
            processes: &processes,
            settings: &settings,
        };
        let (out, counts) = build_startup(&input, q);
        for r in out.rows.iter() {
            assert_eq!((r.depth, r.has_children, r.expanded), (0, false, false));
            let SysNodeId::Startup(id) = &r.node else {
                panic!("not a startup row: {:?}", r.node);
            };
            assert_eq!(startup.entries[r.record.expect("record") as usize].id, *id);
        }
        let names = out
            .rows
            .iter()
            .map(|r| {
                startup.entries[r.record.expect("record") as usize]
                    .name
                    .to_string()
            })
            .collect();
        (names, counts)
    }

    fn startup_sorted(column: StartupColumn) -> SysTableQuery {
        SysTableQuery {
            sort: SysSortSpec {
                column: SysColumnId::Startup(column),
                ascending: true,
            },
            ..query(SysTable::Startup, 1)
        }
    }

    #[test]
    fn startup_status_sort_order() {
        let entries = vec![
            entry("a", StartupStatus::MissingProgram),
            entry("b", StartupStatus::Enabled),
            entry("c", StartupStatus::Disabled),
            entry("d", StartupStatus::NotInThisDesktop),
            entry("e", StartupStatus::Masked),
        ];
        // The default sort is Status ascending (R-011).
        let (names, _) = run_startup(entries, &query(SysTable::Startup, 1));
        assert_eq!(names, ["b", "c", "e", "d", "a"]);
    }

    #[test]
    fn startup_status_tie_break_name() {
        let entries = vec![
            entry("beta", StartupStatus::Enabled),
            entry("Disabled one", StartupStatus::Disabled),
            entry("Alpha", StartupStatus::Enabled),
        ];
        let (names, _) = run_startup(entries, &startup_sorted(StartupColumn::Status));
        assert_eq!(names, ["Alpha", "beta", "Disabled one"]);
    }

    #[test]
    fn startup_impact_sort_order() {
        let mut entries = Vec::new();
        for (name, impact) in [
            ("a", Impact::NotMeasured),
            ("b", Impact::Low),
            ("c", Impact::High),
            ("d", Impact::None),
            ("e", Impact::Medium),
        ] {
            let mut e = entry(name, StartupStatus::Enabled);
            e.impact = impact;
            entries.push(e);
        }
        let (names, _) = run_startup(entries, &startup_sorted(StartupColumn::Impact));
        assert_eq!(names, ["c", "e", "b", "d", "a"]);
    }

    #[test]
    fn startup_filter_location_and_counts() {
        let mut sys_a = entry("a", StartupStatus::Enabled);
        sys_a.location = "/etc/xdg/autostart".into();
        let mut sys_b = entry("b", StartupStatus::Disabled);
        sys_b.location = "/etc/xdg/autostart".into();
        let entries = vec![
            sys_a,
            entry("c", StartupStatus::Enabled),
            sys_b,
            entry("d", StartupStatus::Enabled),
        ];
        let q = SysTableQuery {
            filter: Arc::from("/etc/xdg"),
            ..query(SysTable::Startup, 1)
        };
        let (names, counts) = run_startup(entries, &q);
        assert_eq!(names, ["a", "b"]);
        assert_eq!(
            counts,
            StartupCounts {
                total: 2,
                enabled: 1,
                disabled: 1,
            }
        );
    }

    #[test]
    fn startup_text_filter_columns() {
        let mut e = entry("app", StartupStatus::NotInThisDesktop);
        e.publisher = Some("Vendor 1.2".into());
        e.impact = Impact::Medium;
        e.kind = StartupKind::PlasmaLogout;
        e.command = "/usr/bin/run --flag".into();
        for text in [
            "vendor",
            "not in this",
            "medium",
            "logout script",
            "--flag",
            "/.config/auto",
        ] {
            let q = SysTableQuery {
                filter: Arc::from(text),
                ..query(SysTable::Startup, 1)
            };
            assert_eq!(
                run_startup(vec![e.clone()], &q).0.len(),
                1,
                "filter {text:?}"
            );
        }
        let q = SysTableQuery {
            filter: Arc::from("nomatch"),
            ..query(SysTable::Startup, 1)
        };
        assert!(run_startup(vec![e], &q).0.is_empty());
    }

    #[test]
    fn startup_counts_disabled_is_not_enabled() {
        let entries = vec![
            entry("a", StartupStatus::Enabled),
            entry("b", StartupStatus::Enabled),
            entry("c", StartupStatus::Enabled),
            entry("d", StartupStatus::Masked),
            entry("e", StartupStatus::NotInThisDesktop),
        ];
        let (_, counts) = run_startup(entries, &query(SysTable::Startup, 1));
        assert_eq!(
            counts,
            StartupCounts {
                total: 5,
                enabled: 3,
                disabled: 2,
            }
        );
    }

    #[test]
    fn startup_publisher_blank_last() {
        let mut a = entry("a", StartupStatus::Enabled);
        a.publisher = Some("zeta".into());
        let b = entry("b", StartupStatus::Enabled);
        let mut c = entry("c", StartupStatus::Enabled);
        c.publisher = Some("Alpha".into());
        for (ascending, want) in [(true, ["c", "a", "b"]), (false, ["a", "c", "b"])] {
            let q = SysTableQuery {
                sort: SysSortSpec {
                    column: SysColumnId::Startup(StartupColumn::Publisher),
                    ascending,
                },
                ..query(SysTable::Startup, 1)
            };
            let entries = vec![a.clone(), b.clone(), c.clone()];
            assert_eq!(run_startup(entries, &q).0, want);
        }
    }

    #[test]
    fn default_sorts_match_r011() {
        let services = default_sort(SysTable::Services);
        assert_eq!(services.column.as_str(), "name");
        assert_eq!(services.column.table(), SysTable::Services);
        assert!(services.ascending);

        let startup = default_sort(SysTable::Startup);
        assert_eq!(startup.column.as_str(), "status");
        assert_eq!(startup.column.table(), SysTable::Startup);
        assert!(startup.ascending);

        let connections = default_sort(SysTable::Connections);
        assert_eq!(connections.column.as_str(), "proto");
        assert_eq!(connections.column.table(), SysTable::Connections);
        assert!(connections.ascending);
    }
}
