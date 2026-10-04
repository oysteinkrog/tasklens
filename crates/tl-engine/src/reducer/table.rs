//! Process table builder: filter, sort and flatten (ADR-009, PROC-014, PROC-044, PROC-047).
//!
//! The engine builds the rows; the UI never sorts or filters. `build` turns the current records
//! and the UI's `TableQuery` into a `ProcessTableView`. This file holds flat mode
//! (`TableMode::Flat`) and tree mode (`TableMode::Tree`, E09-010); grouped mode (E09-011) extends
//! it.

use std::cmp::Ordering;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_model::devices::DeviceCatalog;
use tl_model::ids::{ColumnId, NodeId};
use tl_model::process::{AppCgroupSample, GroupKind, ProcFlags, ProcessExtras, ProcessRecord};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::SystemSnapshot;
use tl_model::table::{
    ProcessTableView, SortSpec, TableMode, TableQuery, TableRow, TableTotals, UnitRef,
};
use tl_model::validity::{ProcessField, Validity};

use crate::update::CgroupStats;

/// Everything `build` reads. Borrowed from the reducer's current state.
pub struct TableInput<'a> {
    /// Sorted by `ProcessKey`.
    pub records: &'a [ProcessRecord],
    /// Index aligned with `records` (R-141 item 1).
    pub extras: &'a [ProcessExtras],
    /// App cgroup samples (R-141 item 2).
    pub app_stats: &'a [AppCgroupSample],
    pub unit_stats: &'a [CgroupStats],
    pub system: &'a SystemSnapshot,
    pub devices: &'a DeviceCatalog,
    pub settings: &'a EngineSettings,
    /// Unit Description by unit (E09-025). Empty in P1.
    pub unit_names: &'a FxHashMap<UnitRef, Arc<str>>,
}

/// Columns measured for visible rows only (PROC-047). Sorting on one of them falls back to PID
/// ascending.
pub const NOT_SORTABLE: &[&str] = &[
    "pss",
    "uss",
    "cycles",
    "cycles_delta",
    "ctx_switches",
    "ctx_switches_delta",
];

/// The value a row sorts by in one column.
#[derive(Clone, Debug, PartialEq)]
pub enum SortKey {
    Int(i128),
    Float(f32),
    Text(Arc<str>),
    /// No value to sort by. Sorts last in both directions.
    Absent,
}

impl SortKey {
    /// `Float` for a number, `Absent` for NaN.
    fn float(v: f32) -> SortKey {
        if v.is_nan() {
            SortKey::Absent
        } else {
            SortKey::Float(v)
        }
    }

    fn text(v: Option<&Arc<str>>) -> SortKey {
        v.map_or(SortKey::Absent, |s| SortKey::Text(s.clone()))
    }

    fn is_absent(&self) -> bool {
        matches!(self, SortKey::Absent)
    }

    /// Order of two present keys. Keys of one column share a variant; mixed variants order
    /// Int, Float, Text so the sort stays total.
    fn cmp_present(&self, other: &SortKey) -> Ordering {
        match (self, other) {
            (SortKey::Int(a), SortKey::Int(b)) => a.cmp(b),
            (SortKey::Float(a), SortKey::Float(b)) => a.total_cmp(b),
            (SortKey::Text(a), SortKey::Text(b)) => cmp_text(a, b),
            _ => self.rank().cmp(&other.rank()),
        }
    }

    fn rank(&self) -> u8 {
        match self {
            SortKey::Int(_) => 0,
            SortKey::Float(_) => 1,
            SortKey::Text(_) => 2,
            SortKey::Absent => 3,
        }
    }
}

/// Case-insensitive text order (text columns sort case-insensitively), without allocating.
/// Ties break by byte order, so the order stays total.
fn cmp_text(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
        .then_with(|| a.cmp(b))
}

/// True when the field's validity lets its value take part in the order.
fn usable(r: &ProcessRecord, field: ProcessField) -> bool {
    matches!(r.validity.get(field), Validity::Valid | Validity::Stale)
}

/// The sort value of one record in column `col`: the field the column displays.
///
/// A field whose validity is not `Valid` or `Stale` is `Absent`. These columns are always
/// `Absent`, because their data is in neither `ProcessRecord` nor `ProcessExtras` (R-007,
/// PROC-047, R-141 item 6):
/// - visible-row-only: `pss`, `uss`, `cycles`, `cycles_delta`, `ctx_switches`,
///   `ctx_switches_delta`, `windows`, `x11_resources`;
/// - package and file (P2, P4): `version`, `timestamp`, `file_mtime`, `file_size`, `company`,
///   `signer`, `signature`, `architecture`;
/// - security and session (P2): `session`, `security_label`, `elevation`, `sandbox`;
/// - other P2 and P4 values: `commit_peak`, `commit_delta`, `page_tables`,
///   `major_faults_delta`, `disk_read_bytes`, `disk_write_bytes`, `disk_total_bytes`,
///   `disk_read_delta`, `disk_write_delta`, `disk_total_delta`, `disk_reads`, `disk_writes`,
///   `disk_reads_delta`, `disk_writes_delta`, every `io_*` column, `gpu_mem_shared`,
///   `gpu_engine`, `gpu_encode`, `gpu_decode`, `gpu_0` to `gpu_15`, `power`, `power_trend`,
///   `io_priority`, `oom_score`, `oom_score_adj`, `tty`, `cwd`, `pgrp`, `sid`, `last_cpu`,
///   `container`.
pub fn sort_key(r: &ProcessRecord, x: &ProcessExtras, col: ColumnId) -> SortKey {
    use ColumnId as C;
    use ProcessField as F;

    let int = |v: i128| SortKey::Int(v);
    let checked = |field: F, key: SortKey| {
        if usable(r, field) {
            key
        } else {
            SortKey::Absent
        }
    };
    let rate_pair = |a: F, b: F, sum: f32| {
        if usable(r, a) && usable(r, b) {
            SortKey::float(sum)
        } else {
            SortKey::Absent
        }
    };

    match col {
        // PROC-027 identity and image
        C::Name => SortKey::Text(r.meta.name.clone()),
        C::NameDisplay => {
            // Desktop Name for app primaries, else comm.
            if r.meta.app_id.is_some() {
                SortKey::Text(r.meta.name.clone())
            } else {
                SortKey::Text(r.meta.comm.clone())
            }
        }
        C::ImageName => SortKey::Text(x.exe_name.clone()),
        C::ImagePath => SortKey::Text(x.exe_path.clone()),
        C::Cmdline => SortKey::text(r.meta.cmdline.as_ref()),
        C::Description => SortKey::text(r.meta.description.as_ref()),
        C::Type => SortKey::Text(Arc::from(type_label(r))),
        C::StartTime => int(r.key.start_ticks.into()),
        // PROC-028 ownership
        C::User => SortKey::Text(r.meta.user.clone()),
        C::Pid => int(r.key.pid.into()),
        C::Ppid => int(r.ppid.into()),
        // PROC-029 CPU and scheduling
        C::Cpu => checked(F::Cpu, SortKey::float(r.cpu_pct)),
        C::CpuKernel => checked(F::Cpu, SortKey::float(x.cpu_kernel_pct)),
        C::CpuUser => checked(F::Cpu, SortKey::float(x.cpu_user_pct)),
        C::CpuRelative => checked(F::Cpu, SortKey::float(x.cpu_relative_pct)),
        C::CpuTime => int(i128::from(x.utime_ticks) + i128::from(x.stime_ticks)),
        C::CpuTimeKernel => int(x.stime_ticks.into()),
        C::CpuTimeUser => int(x.utime_ticks.into()),
        C::Priority => int(priority_rank(r, x)),
        C::Affinity => int(x.affinity.count().into()),
        // PROC-030 memory
        C::Memory => checked(F::RssAnon, int(r.rss_anon_bytes.into())),
        C::WsTotal => checked(F::Rss, int(r.rss_bytes.into())),
        C::WsPeak => int(x.rss_peak_bytes.into()),
        C::WsShared => int(x.rss_shared_bytes.into()),
        C::Commit => int(x.commit_bytes.into()),
        C::Vsize => int(x.vsize_bytes.into()),
        C::VsizePeak => int(x.vsize_peak_bytes.into()),
        C::PageFaults => int(x.page_faults.into()),
        C::PageFaultsDelta => int(x.page_faults_delta.into()),
        C::MajorFaults => int(x.major_faults.into()),
        C::Swap => checked(F::Swap, int(r.swap_bytes.into())),
        // PROC-031 disk rates
        C::Disk => rate_pair(F::IoRead, F::IoWrite, r.io_read_bps + r.io_write_bps),
        C::DiskReadRate => checked(F::IoRead, SortKey::float(r.io_read_bps)),
        C::DiskWriteRate => checked(F::IoWrite, SortKey::float(x.disk_write_raw_bps)),
        C::DiskTotalRate => rate_pair(F::IoRead, F::IoWrite, r.io_read_bps + x.disk_write_raw_bps),
        // PROC-032 network
        C::Network => rate_pair(F::NetRx, F::NetTx, r.net_rx_bps + r.net_tx_bps),
        C::NetSendRate => checked(F::NetTx, SortKey::float(r.net_tx_bps)),
        C::NetRecvRate => checked(F::NetRx, SortKey::float(r.net_rx_bps)),
        // PROC-033 GPU
        C::Gpu => checked(F::GpuUtil, SortKey::float(r.gpu_util_pct)),
        C::GpuMemDedicated => checked(F::GpuMem, int(r.gpu_mem_bytes.into())),
        // PROC-035 object counts
        C::Handles => x.handles.map_or(SortKey::Absent, |h| int(h.into())),
        C::Threads => checked(F::Threads, int(r.threads.into())),
        // PROC-036 Linux extras
        C::State => int(x.state_letter.into()),
        C::Nice => int(r.nice.into()),
        C::SchedPolicy => int(x.policy as i128),
        C::Unit => x
            .unit
            .as_ref()
            .map_or(SortKey::Absent, |u| SortKey::Text(u.name.clone())),
        C::Cgroup => SortKey::text(r.meta.cgroup.as_ref()),
        C::AppId => SortKey::text(r.meta.app_id.as_ref()),
        C::Version
        | C::Timestamp
        | C::FileMtime
        | C::FileSize
        | C::Company
        | C::Signer
        | C::Signature
        | C::Architecture
        | C::Session
        | C::SecurityLabel
        | C::Elevation
        | C::Sandbox
        | C::Cycles
        | C::CyclesDelta
        | C::CtxSwitches
        | C::CtxSwitchesDelta
        | C::CommitPeak
        | C::CommitDelta
        | C::PageTables
        | C::MajorFaultsDelta
        | C::Pss
        | C::Uss
        | C::DiskReadBytes
        | C::DiskWriteBytes
        | C::DiskTotalBytes
        | C::DiskReadDelta
        | C::DiskWriteDelta
        | C::DiskTotalDelta
        | C::DiskReads
        | C::DiskWrites
        | C::DiskReadsDelta
        | C::DiskWritesDelta
        | C::IoReadBytes
        | C::IoWriteBytes
        | C::IoReadDelta
        | C::IoWriteDelta
        | C::IoReadRate
        | C::IoWriteRate
        | C::IoReadOps
        | C::IoWriteOps
        | C::GpuMemShared
        | C::GpuEngine
        | C::GpuEncode
        | C::GpuDecode
        | C::Gpu0
        | C::Gpu1
        | C::Gpu2
        | C::Gpu3
        | C::Gpu4
        | C::Gpu5
        | C::Gpu6
        | C::Gpu7
        | C::Gpu8
        | C::Gpu9
        | C::Gpu10
        | C::Gpu11
        | C::Gpu12
        | C::Gpu13
        | C::Gpu14
        | C::Gpu15
        | C::Power
        | C::PowerTrend
        | C::Windows
        | C::X11Resources
        | C::IoPriority
        | C::OomScore
        | C::OomScoreAdj
        | C::Tty
        | C::Cwd
        | C::Pgrp
        | C::Sid
        | C::LastCpu
        | C::Container => SortKey::Absent,
    }
}

/// The PROC-004 class text of the Type column (PROC-027).
fn type_label(r: &ProcessRecord) -> &'static str {
    if r.flags.contains(ProcFlags::KERNEL_THREAD) {
        "Kernel thread"
    } else if r.group == GroupKind::Background.key() {
        "Background process"
    } else if r.group == GroupKind::System.key() || r.group == GroupKind::Ungrouped.key() {
        "System process"
    } else {
        "App"
    }
}

/// Lower is higher priority: realtime first (higher `rt_prio` first), then nice -20..=19, then
/// SCHED_IDLE (PROC-029 Priority).
fn priority_rank(r: &ProcessRecord, x: &ProcessExtras) -> i128 {
    use tl_model::actions::SchedPolicy;
    match x.policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => -200 - i128::from(x.rt_prio),
        SchedPolicy::Idle => 100,
        SchedPolicy::Other | SchedPolicy::Batch => i128::from(r.nice),
    }
}

/// One search term of the filter box.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    lower: String,
    /// All ASCII digits, so it also matches a PID by prefix.
    digits: bool,
}

/// The parsed filter text (PROC-044). Every term must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    terms: Vec<Term>,
}

impl Filter {
    /// Splits on spaces and lowercases each term.
    pub fn parse(s: &str) -> Self {
        let terms = s
            .split(' ')
            .filter(|t| !t.is_empty())
            .map(|t| Term {
                lower: t.to_lowercase(),
                digits: t.bytes().all(|b| b.is_ascii_digit()),
            })
            .collect();
        Filter { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// True when every term is a substring of `meta.search_lower` or of a string in `extra`
    /// (group name, unit name), or, for an all-digit term, a prefix of the PID.
    pub fn matches(&self, r: &ProcessRecord, extra: &[&str]) -> bool {
        let mut pid_buf = [0u8; 10];
        let pid = pid_text(r.key.pid, &mut pid_buf);
        self.terms.iter().all(|t| {
            (t.digits && pid.starts_with(&t.lower))
                || r.meta.search_lower.contains(t.lower.as_str())
                || extra.iter().any(|e| contains_lower(e, &t.lower))
        })
    }
}

/// Decimal text of `pid` in `buf`, without allocating.
fn pid_text(pid: u32, buf: &mut [u8; 10]) -> &str {
    let mut n = pid;
    let mut i = buf.len();
    loop {
        i -= 1;
        // n % 10 is below 10, so the cast cannot truncate.
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    std::str::from_utf8(&buf[i..]).unwrap_or("")
}

/// Case-insensitive substring test; `needle` is already lowercase.
fn contains_lower(hay: &str, needle: &str) -> bool {
    if hay.chars().any(char::is_uppercase) {
        hay.to_lowercase().contains(needle)
    } else {
        hay.contains(needle)
    }
}

/// The filter's `extra` strings of one record: the unit name and its Description.
fn extra_strings<'a>(input: &TableInput<'a>, x: Option<&'a ProcessExtras>) -> [&'a str; 2] {
    let Some(unit) = x.and_then(|x| x.unit.as_ref()) else {
        return ["", ""];
    };
    let description = input.unit_names.get(unit).map_or("", |d| d.as_ref());
    [unit.name.as_ref(), description]
}

/// The record indexes that pass `filter`, in record order.
pub(crate) fn filtered(input: &TableInput<'_>, filter: &Filter) -> Vec<u32> {
    let mut out = Vec::with_capacity(input.records.len());
    for (i, r) in input.records.iter().enumerate() {
        let pass = filter.is_empty() || {
            let extra = extra_strings(input, input.extras.get(i));
            filter.matches(r, &extra)
        };
        if pass {
            // The snapshot never holds more than u32::MAX records.
            out.push(i as u32);
        }
    }
    out
}

/// Sorts record indexes by `sort`: the column's key in the chosen direction, ties by
/// `ProcessKey` ascending, `Absent` last. A `NOT_SORTABLE` column sorts by PID ascending.
pub(crate) fn sort_indices(input: &TableInput<'_>, indexes: &mut [u32], sort: SortSpec) {
    let (column, ascending) = if NOT_SORTABLE.contains(&sort.column.as_str()) {
        (ColumnId::Pid, true)
    } else {
        (sort.column, sort.ascending)
    };
    let mut keyed: Vec<(SortKey, u32)> = indexes
        .iter()
        .map(|&i| {
            let r = &input.records[i as usize];
            let key = match input.extras.get(i as usize) {
                Some(x) => sort_key(r, x, column),
                None => record_only_key(r, column),
            };
            (key, i)
        })
        .collect();
    let records = input.records;
    keyed.sort_unstable_by(|(ka, ia), (kb, ib)| {
        let by_key = match (ka.is_absent(), kb.is_absent()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) if ascending => ka.cmp_present(kb),
            (false, false) => kb.cmp_present(ka),
        };
        by_key.then_with(|| records[*ia as usize].key.cmp(&records[*ib as usize].key))
    });
    for (slot, (_, i)) in indexes.iter_mut().zip(keyed) {
        *slot = i;
    }
}

/// Sort key when the extras slice is shorter than the records (it should never be). Only the
/// PID column keeps its order; every other column is `Absent`.
fn record_only_key(r: &ProcessRecord, column: ColumnId) -> SortKey {
    match column {
        ColumnId::Pid => SortKey::Int(r.key.pid.into()),
        _ => SortKey::Absent,
    }
}

/// The ppid forest of the visible records (PROC-014, ADR-009).
///
/// Every index is a record index. `children` and `parent` have one entry per record; a record
/// that is not visible has no parent, no children and is not a root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Forest {
    pub children: Vec<SmallVec<[u32; 4]>>,
    pub parent: Vec<Option<u32>>,
    pub roots: Vec<u32>,
}

/// Builds the forest of the records marked in `visible` (index aligned with `records`).
///
/// A record's parent is the visible record whose `key.pid` equals its `ppid`. With no such record
/// (parent filtered, hidden or gone, or `ppid` 0) the record is a root (ADR-009). Kernel threads
/// have `ppid` 2, so they sit under `kthreadd`. When two visible records share a PID (a lingering
/// `EXITED` record and its successor), the later record in key order, the newer one, is the
/// parent. A `ppid` cycle, impossible on Linux but possible in a fixture, is cut so every visible
/// record still appears once. O(n) with one PID map.
pub fn forest(records: &[ProcessRecord], visible: &[bool]) -> Forest {
    let n = records.len();
    let is_visible = |i: usize| visible.get(i).copied().unwrap_or(false);

    let mut by_pid: FxHashMap<u32, u32> =
        FxHashMap::with_capacity_and_hasher(n, Default::default());
    for (i, r) in records.iter().enumerate() {
        if is_visible(i) {
            // The snapshot never holds more than u32::MAX records.
            by_pid.insert(r.key.pid, i as u32);
        }
    }

    let mut f = Forest {
        children: vec![SmallVec::new(); n],
        parent: vec![None; n],
        roots: Vec::new(),
    };
    for (i, r) in records.iter().enumerate() {
        if !is_visible(i) {
            continue;
        }
        let i = i as u32;
        match by_pid.get(&r.ppid) {
            Some(&p) if p != i => {
                f.parent[i as usize] = Some(p);
                f.children[p as usize].push(i);
            }
            _ => f.roots.push(i),
        }
    }
    cut_cycles(&mut f, visible);
    f
}

/// Makes every visible record reachable from a root. A record not reached from the roots lies in
/// or under a `ppid` cycle: walk up from it to the first record seen twice, which is on the
/// cycle, and make that record a root.
fn cut_cycles(f: &mut Forest, visible: &[bool]) {
    let n = f.children.len();
    let mut reached = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    for &root in &f.roots {
        mark_subtree(f, root, &mut reached, &mut stack);
    }
    // Which start record last walked through each record, so the marks need no reset.
    let mut walked_from = vec![u32::MAX; n];
    for start in 0..n {
        if reached[start] || !visible.get(start).copied().unwrap_or(false) {
            continue;
        }
        let mut cur = start as u32;
        while walked_from[cur as usize] != start as u32 {
            walked_from[cur as usize] = start as u32;
            match f.parent[cur as usize] {
                Some(p) => cur = p,
                // Unreached records always have a parent; stop if a hand-built forest has none.
                None => break,
            }
        }
        if let Some(p) = f.parent[cur as usize].take() {
            f.children[p as usize].retain(|c| *c != cur);
        }
        f.roots.push(cur);
        mark_subtree(f, cur, &mut reached, &mut stack);
    }
}

/// Marks `root` and every record under it in `reached`, with an explicit stack.
fn mark_subtree(f: &Forest, root: u32, reached: &mut [bool], stack: &mut Vec<u32>) {
    stack.clear();
    stack.push(root);
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut reached[i as usize], true) {
            continue;
        }
        stack.extend(f.children[i as usize].iter().copied());
    }
}

/// All descendants of `i` in `f`: the N of the collapsed "PID +N" form (PROC-018).
///
/// Walks with an explicit stack and a visited bitset, so a cycle in a hand-built forest cannot
/// loop.
pub fn descendants_count(f: &Forest, i: u32) -> u32 {
    let n = f.children.len();
    if i as usize >= n {
        return 0;
    }
    let mut visited = vec![0u64; n.div_ceil(64)];
    let mut test_and_set = |j: u32| {
        let (word, bit) = (j as usize / 64, 1u64 << (j % 64));
        let was = visited[word] & bit != 0;
        visited[word] |= bit;
        was
    };
    test_and_set(i);
    let mut stack = vec![i];
    let mut count = 0u32;
    while let Some(j) = stack.pop() {
        for &c in &f.children[j as usize] {
            if (c as usize) < n && !test_and_set(c) {
                count = count.saturating_add(1);
                stack.push(c);
            }
        }
    }
    count
}

/// Appends the tree rows of `f` to `out` in display order, roots at `depth`.
///
/// `order` sorts one sibling list (the E09-009 sort) and is applied to the roots and to every
/// expanded record's children. A row's children follow it only when its `NodeId` is in
/// `expanded`. `depth` grows by 1 per level and saturates at 255. `descendants` equals
/// `descendants_count`, computed for all records in one pass. `records` gives each row's
/// `ProcessKey`.
pub fn flatten_tree(
    records: &[ProcessRecord],
    f: &Forest,
    order: &dyn Fn(&[u32]) -> Vec<u32>,
    expanded: &FxHashSet<NodeId>,
    out: &mut Vec<TableRow>,
    depth: u8,
) {
    let n = f.children.len().min(records.len());
    let in_range = |i: u32| (i as usize) < n;

    // Pre-order of every record under the roots, then sum the counts bottom up.
    let mut seen = vec![false; n];
    let mut pre: Vec<u32> = Vec::with_capacity(n);
    let mut stack: Vec<u32> = f.roots.iter().copied().filter(|&i| in_range(i)).collect();
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut seen[i as usize], true) {
            continue;
        }
        pre.push(i);
        stack.extend(
            f.children[i as usize]
                .iter()
                .copied()
                .filter(|&c| in_range(c)),
        );
    }
    let mut descendants = vec![0u32; n];
    let mut counted = vec![false; n];
    for &i in pre.iter().rev() {
        counted[i as usize] = true;
        let mut sum = 0u32;
        for &c in &f.children[i as usize] {
            // Only children already counted: in an acyclic forest that is every child.
            if in_range(c) && counted[c as usize] && c != i {
                sum = sum
                    .saturating_add(descendants[c as usize])
                    .saturating_add(1);
            }
        }
        descendants[i as usize] = sum;
    }

    let mut emitted = vec![false; n];
    let mut stack: Vec<(u32, u8)> = order(&f.roots)
        .into_iter()
        .rev()
        .filter(|&i| in_range(i))
        .map(|i| (i, depth))
        .collect();
    while let Some((i, d)) = stack.pop() {
        if std::mem::replace(&mut emitted[i as usize], true) {
            continue;
        }
        let node = NodeId::Process(records[i as usize].key);
        let kids = &f.children[i as usize];
        let is_expanded = expanded.contains(&node);
        out.push(TableRow {
            node,
            depth: d,
            expanded: is_expanded,
            has_children: !kids.is_empty(),
            record: Some(i),
            group: None,
            descendants: descendants[i as usize],
            subtree: None,
        });
        if is_expanded && !kids.is_empty() {
            let next = d.saturating_add(1);
            for c in order(kids).into_iter().rev().filter(|&c| in_range(c)) {
                stack.push((c, next));
            }
        }
    }
}

/// Tree mode visibility: a record is visible when it is in `matches` or, with
/// `include_children`, when any ancestor in the full ppid forest is (PROC-045).
fn tree_visible(records: &[ProcessRecord], matches: &[u32], include_children: bool) -> Vec<bool> {
    let mut visible = vec![false; records.len()];
    for &i in matches {
        visible[i as usize] = true;
    }
    if include_children && matches.len() < records.len() {
        let all = forest(records, &vec![true; records.len()]);
        let mut reached = vec![false; records.len()];
        let mut stack = Vec::new();
        for &i in matches {
            mark_subtree(&all, i, &mut reached, &mut stack);
        }
        for (v, r) in visible.iter_mut().zip(reached) {
            *v |= r;
        }
    }
    visible
}

/// Tree mode rows: the forest of the visible records, siblings sorted by `query.sort`.
fn tree_rows(input: &TableInput<'_>, query: &TableQuery, filter: &Filter) -> (Vec<TableRow>, u32) {
    let matches = filtered(input, filter);
    let visible = tree_visible(input.records, &matches, query.include_children);
    let shown = visible.iter().filter(|v| **v).count() as u32;
    let f = forest(input.records, &visible);
    let expanded: FxHashSet<NodeId> = query.expanded.iter().cloned().collect();
    let order = |ids: &[u32]| {
        let mut v = ids.to_vec();
        sort_indices(input, &mut v, query.sort);
        v
    };
    let mut rows = Vec::with_capacity(shown as usize);
    flatten_tree(input.records, &f, &order, &expanded, &mut rows, 0);
    (rows, shown)
}

/// Flat mode rows: every match at depth 0, sorted by `query.sort`.
fn flat_rows(input: &TableInput<'_>, query: &TableQuery, filter: &Filter) -> (Vec<TableRow>, u32) {
    let mut indexes = filtered(input, filter);
    sort_indices(input, &mut indexes, query.sort);

    let rows: Vec<TableRow> = indexes
        .iter()
        .map(|&i| TableRow {
            node: NodeId::Process(input.records[i as usize].key),
            depth: 0,
            expanded: false,
            has_children: false,
            record: Some(i),
            group: None,
            descendants: 0,
            subtree: None,
        })
        .collect();
    let shown = rows.len() as u32;
    (rows, shown)
}

/// Builds the process table for `query` (ADR-009). Grouped mode (E09-011) still builds flat
/// rows.
pub fn build(input: &TableInput<'_>, query: &TableQuery) -> ProcessTableView {
    let filter = Filter::parse(&query.filter);
    let (rows, shown) = match query.mode {
        TableMode::Tree => tree_rows(input, query, &filter),
        TableMode::Flat | TableMode::Grouped => flat_rows(input, query, &filter),
    };

    let restricted = input
        .records
        .iter()
        .filter(|r| r.validity.0.contains(&Validity::Restricted))
        .count();

    // Counts fit u32: the snapshot never holds more than u32::MAX records.
    ProcessTableView {
        header: Default::default(),
        query_generation: query.generation,
        total: input.records.len() as u32,
        shown,
        restricted: restricted as u32,
        rows: Arc::from(rows),
        groups: Arc::from([]),
        totals: TableTotals::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_model::actions::SchedPolicy;
    use tl_model::ids::{GroupKey, ProcessKey};
    use tl_model::process::{ProcState, ProcessMeta};
    use tl_model::system::CpuSet;
    use tl_model::time::BootId;
    use tl_model::validity::FieldValidity;

    fn rec(pid: u32, ppid: u32, name: &str, cpu: f32) -> ProcessRecord {
        ProcessRecord {
            key: ProcessKey {
                start_ticks: u64::from(pid),
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
                search_lower: name.to_lowercase().into(),
            }),
            group: GroupKey(0),
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

    fn extras() -> ProcessExtras {
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
        }
    }

    /// Builds `records` in tree mode and returns (pid, depth, has_children, descendants) per row.
    fn tree(
        records: &[ProcessRecord],
        filter: &str,
        include_children: bool,
        expanded: &[u32],
        sort: SortSpec,
    ) -> Vec<(u32, u8, bool, u32)> {
        let mut records = records.to_vec();
        records.sort_by_key(|r| r.key);
        let extras: Vec<ProcessExtras> = records.iter().map(|_| extras()).collect();
        let system = SystemSnapshot::default();
        let devices = DeviceCatalog::default();
        let settings = EngineSettings::default();
        let unit_names = FxHashMap::default();
        let input = TableInput {
            records: &records,
            extras: &extras,
            app_stats: &[],
            unit_stats: &[],
            system: &system,
            devices: &devices,
            settings: &settings,
            unit_names: &unit_names,
        };
        let expanded: Vec<NodeId> = records
            .iter()
            .filter(|r| expanded.contains(&r.key.pid))
            .map(|r| NodeId::Process(r.key))
            .collect();
        let query = TableQuery {
            mode: TableMode::Tree,
            sort,
            filter: Arc::from(filter),
            expanded: Arc::from(expanded),
            include_children,
            ..TableQuery::default()
        };
        let view = build(&input, &query);
        view.rows
            .iter()
            .map(|row| {
                let r = &records[row.record.unwrap() as usize];
                (r.key.pid, row.depth, row.has_children, row.descendants)
            })
            .collect()
    }

    const PID_ASC: SortSpec = SortSpec {
        column: ColumnId::Pid,
        ascending: true,
    };

    fn abc() -> Vec<ProcessRecord> {
        vec![
            rec(1, 0, "alpha", 0.0),
            rec(2, 1, "bravo", 0.0),
            rec(3, 2, "charlie", 0.0),
        ]
    }

    #[test]
    fn tree_depths_and_has_children() {
        let rows = tree(&abc(), "", false, &[1, 2], PID_ASC);
        assert_eq!(rows, [(1, 0, true, 2), (2, 1, true, 1), (3, 2, false, 0)]);
    }

    #[test]
    fn collapsed_parent_descendant_count() {
        let rows = tree(&abc(), "", false, &[1], PID_ASC);
        assert_eq!(rows, [(1, 0, true, 2), (2, 1, true, 1)]);
    }

    #[test]
    fn filtered_child_becomes_root() {
        let rows = tree(&abc(), "charlie", false, &[1, 2], PID_ASC);
        assert_eq!(rows, [(3, 0, false, 0)]);
    }

    #[test]
    fn include_children_pulls_descendants() {
        let rows = tree(&abc(), "alpha", true, &[1, 2], PID_ASC);
        assert_eq!(rows, [(1, 0, true, 2), (2, 1, true, 1), (3, 2, false, 0)]);
        let rows = tree(&abc(), "alpha", false, &[1, 2], PID_ASC);
        assert_eq!(rows, [(1, 0, false, 0)]);
    }

    #[test]
    fn exited_parent_keeps_place_then_child_reroots() {
        let mut records = abc();
        records[1].flags |= ProcFlags::EXITED;
        records[1].state = ProcState::Zombie;
        let rows = tree(&records, "", false, &[1, 2], PID_ASC);
        assert_eq!(rows, [(1, 0, true, 2), (2, 1, true, 1), (3, 2, false, 0)]);
        records.remove(1);
        let rows = tree(&records, "", false, &[1, 2], PID_ASC);
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
        let cpu_desc = SortSpec {
            column: ColumnId::Cpu,
            ascending: false,
        };
        let pids: Vec<u32> = tree(&records, "", false, &[10, 20], cpu_desc)
            .iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(pids, [20, 22, 21, 10, 12, 11]);
    }

    #[test]
    fn kernel_threads_sit_under_kthreadd() {
        let records = vec![
            rec(1, 0, "systemd", 0.0),
            rec(2, 0, "kthreadd", 0.0),
            rec(3, 2, "rcu_gp", 0.0),
        ];
        let rows = tree(&records, "", false, &[2], PID_ASC);
        assert_eq!(rows, [(1, 0, false, 0), (2, 0, true, 1), (3, 1, false, 0)]);
    }

    #[test]
    fn ppid_cycle_does_not_loop() {
        // 5 -> 6 -> 7 -> 5, and 8 under 7. Every record still shows once.
        let records = vec![
            rec(5, 7, "a", 0.0),
            rec(6, 5, "b", 0.0),
            rec(7, 6, "c", 0.0),
            rec(8, 7, "d", 0.0),
        ];
        let rows = tree(&records, "", false, &[5, 6, 7, 8], PID_ASC);
        let mut pids: Vec<u32> = rows.iter().map(|r| r.0).collect();
        pids.sort_unstable();
        assert_eq!(pids, [5, 6, 7, 8]);

        // A hand-built cycle: descendants_count still ends.
        let f = Forest {
            children: vec![SmallVec::from_slice(&[1]), SmallVec::from_slice(&[0])],
            parent: vec![Some(1), Some(0)],
            roots: Vec::new(),
        };
        assert_eq!(descendants_count(&f, 0), 1);
    }

    #[test]
    fn depth_saturates_at_255() {
        let records: Vec<ProcessRecord> = (1..=300u32)
            .map(|pid| rec(pid, pid - 1, "p", 0.0))
            .collect();
        let all: Vec<u32> = (1..=300).collect();
        let rows = tree(&records, "", false, &all, PID_ASC);
        assert_eq!(rows.len(), 300);
        assert_eq!(rows[0], (1, 0, true, 299));
        assert_eq!(rows[255].1, 255);
        assert_eq!(rows[299].1, 255);
    }

    #[test]
    fn text_sort_case_insensitive() {
        let mut names = ["bash", "Bash", "zsh", "Xorg"];
        names.sort_by(|a, b| cmp_text(a, b));
        // Equal ignoring case: byte order puts "Bash" before "bash".
        assert_eq!(names, ["Bash", "bash", "Xorg", "zsh"]);
    }
}
