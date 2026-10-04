//! Process table builder: filter, sort and flatten (ADR-009, PROC-014, PROC-044, PROC-047).
//!
//! The engine builds the rows; the UI never sorts or filters. `build` turns the current records
//! and the UI's `TableQuery` into a `ProcessTableView`. This file holds flat mode
//! (`TableMode::Flat`); tree mode (E09-010) and grouped mode (E09-011) extend it.

use std::cmp::Ordering;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_model::devices::DeviceCatalog;
use tl_model::ids::{ColumnId, NodeId};
use tl_model::process::{AppCgroupSample, GroupKind, ProcFlags, ProcessExtras, ProcessRecord};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::SystemSnapshot;
use tl_model::table::{ProcessTableView, SortSpec, TableQuery, TableRow, TableTotals, UnitRef};
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
            (SortKey::Text(a), SortKey::Text(b)) => a.cmp(b),
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

/// Builds the process table for `query` (ADR-009). Flat mode only so far.
pub fn build(input: &TableInput<'_>, query: &TableQuery) -> ProcessTableView {
    let filter = Filter::parse(&query.filter);
    let mut indexes = filtered(input, &filter);
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
        shown: rows.len() as u32,
        restricted: restricted as u32,
        rows: Arc::from(rows),
        groups: Arc::from([]),
        totals: TableTotals::default(),
    }
}
