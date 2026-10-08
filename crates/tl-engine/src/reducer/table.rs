//! Process table builder: filter, sort and flatten (ADR-009, PROC-014, PROC-044, PROC-047).
//!
//! The engine builds the rows; the UI never sorts or filters. `build` turns the current records
//! and the UI's `TableQuery` into a `ProcessTableView`. This file holds flat mode
//! (`TableMode::Flat`), tree mode (`TableMode::Tree`, E09-010) and grouped mode
//! (`TableMode::Grouped`, E09-011), each with the pseudo rows Interrupts and Memory compression
//! (zram) (E09-023).

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, DeviceKind, NetKind};
use tl_model::ids::{ColumnId, DeviceIdx, GroupKey, NodeId, PseudoKind};
use tl_model::process::{
    AppCgroupSample, GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcessExtras, ProcessRecord,
};
use tl_model::settings::{EngineSettings, NetworkUnit};
use tl_model::snapshot::SystemSnapshot;
use tl_model::system::{GpuTotals, Metric};
use tl_model::table::{
    Manager, ProcessTableView, SortSpec, TableMode, TableQuery, TableRow, TableTotals, Total,
    UnitRef,
};
use tl_model::units;
use tl_model::validity::{FieldValidity, ProcessField, Validity};

use crate::update::{CgroupStats, ZramStat};

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
    /// `UnitRecord.user` by unit, for the User column of unit rows.
    pub unit_users: &'a FxHashMap<UnitRef, Arc<str>>,
    /// Values of the pseudo rows (E09-023). `None` builds the table without pseudo rows.
    pub pseudo: Option<&'a PseudoInput>,
}

/// What the pseudo rows show (PROC-012): the latest CPU and memory sample values the reducer
/// keeps for the next table build.
#[derive(Clone, Debug, Default)]
pub struct PseudoInput {
    /// Δ(irq + softirq) / Δtotal in percent. `None` when the kernel does not account interrupt
    /// time on its own (U-21).
    pub irq_pct: Option<f32>,
    /// zram devices of the last memory sample.
    pub zram: Vec<ZramStat>,
}

/// The Interrupts row note when `PseudoInput::irq_pct` is `None` (PROC-012).
pub const IRQ_NOT_ACCOUNTED: &str = "Interrupt time is included in task time on this kernel";

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

const KIB: f64 = 1024.0;
const MIB: f64 = 1024.0 * 1024.0;

/// The key the order compares (PROC-047): the step the cell shows, not the raw value, so two
/// rows whose cells read the same keep their `ProcessKey` order across ticks. Percent columns
/// step by 0.1, memory columns by the shown IEC unit's 0.01, rate columns by the shown unit's
/// 0.1 (`units::percent`, `units::bytes`, `units::rate_mib_fixed`, `units::net_rate`). Other
/// keys pass through. `sort_key` itself stays raw, because the process graphs read it.
fn shown_key(key: SortKey, col: ColumnId, settings: &EngineSettings) -> SortKey {
    use ColumnId as C;
    match (col, key) {
        (C::Cpu | C::CpuKernel | C::CpuUser | C::CpuRelative | C::Gpu, SortKey::Float(v)) => {
            SortKey::Int(tenths(f64::from(v)))
        }
        (
            C::Memory
            | C::WsTotal
            | C::WsPeak
            | C::WsShared
            | C::Commit
            | C::Vsize
            | C::VsizePeak
            | C::Swap
            | C::GpuMemDedicated,
            SortKey::Int(n),
        ) => SortKey::Int(iec_shown(n)),
        (C::Disk | C::DiskReadRate | C::DiskWriteRate | C::DiskTotalRate, SortKey::Float(v)) => {
            SortKey::Int(tenths(f64::from(v) / MIB))
        }
        (C::Network | C::NetSendRate | C::NetRecvRate, SortKey::Float(v)) => {
            SortKey::Int(net_shown(f64::from(v), settings))
        }
        (_, key) => key,
    }
}

/// `v` in tenths, as one decimal prints it. Zero, negative and NaN are 0, like the zero form.
fn tenths(v: f64) -> i128 {
    if v.is_nan() || v <= 0.0 {
        0
    } else {
        // Saturates for values far beyond any rate or percent.
        (v * 10.0).round() as i128
    }
}

/// A byte count at the step `units::bytes` prints: exact below 1 KiB, else two decimals of the
/// chosen unit, scaled back to bytes so the order holds across units.
fn iec_shown(n: i128) -> i128 {
    if n < 1024 {
        return n;
    }
    let mut v = n as f64 / KIB;
    let mut scale: i128 = 1024;
    // The same unit choice as `units::bytes`, TiB at most.
    let mut i = 0;
    while v >= KIB - 0.005 && i < 3 {
        v /= KIB;
        scale *= 1024;
        i += 1;
    }
    (v * 100.0).round() as i128 * scale / 100
}

/// A network rate at the step `units::net_rate` prints with `units.network` and
/// `units.network_auto_scale`.
fn net_shown(bytes_per_s: f64, settings: &EngineSettings) -> i128 {
    match (settings.network, settings.network_auto_scale) {
        (NetworkUnit::Bits, false) => tenths(bytes_per_s * 8.0 / 1e6),
        (NetworkUnit::Bytes, false) => tenths(bytes_per_s / MIB),
        (NetworkUnit::Bits, true) => scaled_tenths(bytes_per_s * 8.0 / 1e3, 1000, 3),
        (NetworkUnit::Bytes, true) => scaled_tenths(bytes_per_s / KIB, 1024, 3),
    }
}

/// `v`, in the first of `units` units, at the step auto scale prints: one decimal of the unit
/// chosen like `units::net_rate`, scaled back to tenths of the first unit so the order holds
/// across units.
fn scaled_tenths(mut v: f64, base: u32, units: u32) -> i128 {
    let b = f64::from(base);
    let mut scale: i128 = 1;
    let mut i = 1;
    while v >= b - 0.05 && i < units {
        v /= b;
        scale *= i128::from(base);
        i += 1;
    }
    tenths(v) * scale
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

    /// True when every term is a substring of a string in `texts`. For rows without a record.
    pub fn matches_text(&self, texts: &[&str]) -> bool {
        self.terms
            .iter()
            .all(|t| texts.iter().any(|e| contains_lower(e, &t.lower)))
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

/// The record indexes that pass `filter` and are not hidden by `hide_system`, in record order.
pub(crate) fn filtered(input: &TableInput<'_>, filter: &Filter) -> Vec<u32> {
    let mut out = Vec::with_capacity(input.records.len());
    for (i, r) in input.records.iter().enumerate() {
        if hidden(input, i) {
            continue;
        }
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

/// Sorts record indexes by `sort`: the column's key at the shown step in the chosen direction,
/// ties by `ProcessKey` ascending, `Absent` last. A `NOT_SORTABLE` column sorts by PID ascending.
pub(crate) fn sort_indices(input: &TableInput<'_>, indexes: &mut [u32], sort: SortSpec) {
    let (column, ascending) = effective_sort(sort);
    let mut keyed: Vec<(SortKey, u32)> = indexes
        .iter()
        .map(|&i| (record_key(input, i, column), i))
        .collect();
    let records = input.records;
    keyed.sort_unstable_by(|(ka, ia), (kb, ib)| {
        cmp_keys(ka, kb, ascending)
            .then_with(|| records[*ia as usize].key.cmp(&records[*ib as usize].key))
    });
    for (slot, (_, i)) in indexes.iter_mut().zip(keyed) {
        *slot = i;
    }
}

/// The column and direction actually sorted by: a `NOT_SORTABLE` column sorts by PID ascending.
fn effective_sort(sort: SortSpec) -> (ColumnId, bool) {
    if NOT_SORTABLE.contains(&sort.column.as_str()) {
        (ColumnId::Pid, true)
    } else {
        (sort.column, sort.ascending)
    }
}

/// Order of two sort keys in the chosen direction, `Absent` last in both directions.
fn cmp_keys(a: &SortKey, b: &SortKey, ascending: bool) -> Ordering {
    match (a.is_absent(), b.is_absent()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) if ascending => a.cmp_present(b),
        (false, false) => b.cmp_present(a),
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
    let descendants = descendant_counts(records, f);
    let mut emitted = vec![false; descendants.len()];
    let tree = TreeEmit {
        records,
        f,
        descendants: &descendants,
        order,
        expanded,
    };
    tree.emit(&f.roots, depth, &mut emitted, out);
}

/// `descendants_count` of every record of `f` in one pass: a pre-order walk from the roots,
/// then the counts summed bottom up. Records not under a root count 0.
fn descendant_counts(records: &[ProcessRecord], f: &Forest) -> Vec<u32> {
    let n = f.children.len().min(records.len());
    let in_range = |i: u32| (i as usize) < n;

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
    descendants
}

/// What `TreeEmit::emit` needs to write the rows of some roots of one forest.
struct TreeEmit<'a> {
    records: &'a [ProcessRecord],
    f: &'a Forest,
    /// From `descendant_counts`; its length bounds every index.
    descendants: &'a [u32],
    order: &'a dyn Fn(&[u32]) -> Vec<u32>,
    expanded: &'a FxHashSet<NodeId>,
}

impl TreeEmit<'_> {
    /// Appends `roots` (sorted by `order`) and their expanded subtrees to `out`, roots at
    /// `depth`. `emitted` marks written records, so a record is never written twice, even across
    /// calls that share it.
    fn emit(&self, roots: &[u32], depth: u8, emitted: &mut [bool], out: &mut Vec<TableRow>) {
        let n = self.descendants.len().min(emitted.len());
        let in_range = |i: u32| (i as usize) < n;
        let mut stack: Vec<(u32, u8)> = (self.order)(roots)
            .into_iter()
            .rev()
            .filter(|&i| in_range(i))
            .map(|i| (i, depth))
            .collect();
        while let Some((i, d)) = stack.pop() {
            if std::mem::replace(&mut emitted[i as usize], true) {
                continue;
            }
            let node = NodeId::Process(self.records[i as usize].key);
            let kids = &self.f.children[i as usize];
            let is_expanded = self.expanded.contains(&node);
            out.push(TableRow {
                node,
                depth: d,
                expanded: is_expanded,
                has_children: !kids.is_empty(),
                record: Some(i),
                group: None,
                descendants: self.descendants[i as usize],
                subtree: None,
            });
            if is_expanded && !kids.is_empty() {
                let next = d.saturating_add(1);
                for c in (self.order)(kids)
                    .into_iter()
                    .rev()
                    .filter(|&c| in_range(c))
                {
                    stack.push((c, next));
                }
            }
        }
    }
}

/// Tree mode visibility: a record is visible when it is in `matches` or, with
/// `include_children`, when any ancestor in the full ppid forest is (PROC-045).
/// Records hidden by `hide_system` never become visible this way.
fn tree_visible(input: &TableInput<'_>, matches: &[u32], include_children: bool) -> Vec<bool> {
    let records = input.records;
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
        for (i, (v, r)) in visible.iter_mut().zip(reached).enumerate() {
            *v |= r && !hidden(input, i);
        }
    }
    visible
}

/// Tree mode rows: the forest of the visible records, siblings sorted by `query.sort`, the
/// group rows (the shown pseudo rows, top level and sorted with the roots) and the subtree
/// totals the rows index with `TableRow.subtree`.
///
/// With `subtree_totals` on, every row with children gets a subtree total. With it off, only
/// collapsed parents do (ADR-009: the collapsed parent shows its subtree sum).
fn tree_rows(
    input: &TableInput<'_>,
    query: &TableQuery,
    filter: &Filter,
) -> (Vec<TableRow>, Vec<GroupRow>, Vec<GroupRow>, u32) {
    let matches = filtered(input, filter);
    let visible = tree_visible(input, &matches, query.include_children);
    let shown = visible.iter().filter(|v| **v).count() as u32;
    let f = forest(input.records, &visible);
    let expanded: FxHashSet<NodeId> = query.expanded.iter().cloned().collect();
    let order = |ids: &[u32]| {
        let mut v = ids.to_vec();
        sort_indices(input, &mut v, query.sort);
        v
    };
    let pseudo = shown_pseudo_rows(input, filter, "");
    let mut rows = Vec::with_capacity(shown as usize + pseudo.len());
    if pseudo.is_empty() {
        flatten_tree(input.records, &f, &order, &expanded, &mut rows, 0);
    } else {
        let descendants = descendant_counts(input.records, &f);
        let tree = TreeEmit {
            records: input.records,
            f: &f,
            descendants: &descendants,
            order: &order,
            expanded: &expanded,
        };
        let mut emitted = vec![false; descendants.len()];
        let mut items: Vec<Item> = f.roots.iter().map(|&i| Item::Record(i)).collect();
        items.extend((0..pseudo.len()).map(|g| Item::Group(g as u32)));
        sort_items(input, &pseudo, &mut items, query.sort);
        for item in items {
            match item {
                Item::Record(i) => tree.emit(&[i], 0, &mut emitted, &mut rows),
                Item::Group(g) => rows.push(pseudo_table_row(&pseudo, g, 0)),
            }
        }
    }

    let all = input.settings.subtree_totals;
    let wanted: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.has_children && (all || !row.expanded))
        .map(|(k, _)| k)
        .collect();
    let indexes: Vec<u32> = wanted.iter().filter_map(|&k| rows[k].record).collect();
    let subtree = subtree_totals(&f, input.records, &indexes);
    // Every wanted row has a record with children, so `subtree` lines up with `wanted`.
    for (s, &k) in wanted.iter().enumerate() {
        rows[k].subtree = Some(s as u32);
    }
    (rows, pseudo, subtree, shown)
}

/// The three fixed groups of grouped mode (PROC-004). Every process sits under exactly one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GroupClass {
    Apps,
    Background,
    System,
}

/// Header order in grouped mode, whatever the sort (PROC-004).
pub const GROUP_ORDER: [GroupClass; 3] =
    [GroupClass::Apps, GroupClass::Background, GroupClass::System];

impl GroupClass {
    /// Header text without the count.
    pub const fn name(self) -> &'static str {
        match self {
            GroupClass::Apps => "Apps",
            GroupClass::Background => "Background processes",
            GroupClass::System => "System processes",
        }
    }

    /// `name` in lowercase, for the filter.
    const fn name_lower(self) -> &'static str {
        match self {
            GroupClass::Apps => "apps",
            GroupClass::Background => "background processes",
            GroupClass::System => "system processes",
        }
    }

    /// Key of the header row, `NodeId::Group(key)`. Background and System use the key of their
    /// `GroupKind`. Apps uses the key of an app with an empty id, which no real app has.
    pub fn key(self) -> GroupKey {
        match self {
            GroupClass::Apps => GroupKind::App {
                app_id: Arc::from(""),
            }
            .key(),
            GroupClass::Background => GroupKind::Background.key(),
            GroupClass::System => GroupKind::System.key(),
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// The group a record sits under (PROC-004): kernel threads, System and Ungrouped under System,
/// Background under Background, every `GroupKind::App` under Apps.
pub fn group_class(r: &ProcessRecord) -> GroupClass {
    if r.flags.contains(ProcFlags::KERNEL_THREAD) {
        GroupClass::System
    } else if r.group == GroupKind::Background.key() {
        GroupClass::Background
    } else if r.group == GroupKind::System.key() || r.group == GroupKind::Ungrouped.key() {
        GroupClass::System
    } else {
        GroupClass::Apps
    }
}

/// User slices whose units are desktop infrastructure, hidden by `hide_system` (PROC-046).
pub const INFRA_SLICES: [&str; 2] = ["session.slice", "background.slice"];

/// Units in `INFRA_SLICES` that `hide_system` keeps visible (PROC-046).
pub const HIDE_SYSTEM_EXCEPTIONS: [&str; 3] = [
    "plasma-plasmashell.service",
    "plasma-kwin_wayland.service",
    "org.gnome.Shell@wayland.service",
];

/// true when the record is hidden by `settings.hide_system` (PROC-046, R-031). `class` is
/// `group_class(r)`.
///
/// Hidden: every System record (kernel threads, other users, system units, PID 1), and a
/// Background record whose cgroup path has a unit in `INFRA_SLICES` that is not in
/// `HIDE_SYSTEM_EXCEPTIONS`, or ends in `user@<uid>.service/init.scope` (`systemd --user`).
/// Apps and the rest of Background, `app.slice` included, stay. Pseudo rows are not records;
/// the pseudo row builder hides them.
pub fn hidden_by_hide_system(r: &ProcessRecord, class: GroupClass) -> bool {
    match class {
        GroupClass::System => true,
        GroupClass::Apps => false,
        GroupClass::Background => r.meta.cgroup.as_deref().is_some_and(is_infra_cgroup),
    }
}

/// The desktop infrastructure test of `hidden_by_hide_system` on a cgroup path relative to
/// `/sys/fs/cgroup`. One pass over the path components.
fn is_infra_cgroup(path: &str) -> bool {
    let mut parts = path.split('/').filter(|c| !c.is_empty()).peekable();
    let mut prev: Option<&str> = None;
    while let Some(c) = parts.next() {
        if INFRA_SLICES.contains(&c) && prev.is_some() {
            // The unit is the component right under the slice. A path that ends at the slice
            // has no unit and is not hidden.
            return parts
                .peek()
                .is_some_and(|unit| !HIDE_SYSTEM_EXCEPTIONS.contains(unit));
        }
        if c == "init.scope" && parts.peek().is_none() {
            return prev.is_some_and(is_user_manager);
        }
        prev = Some(c);
    }
    false
}

/// true for `user@<digits>.service`.
fn is_user_manager(c: &str) -> bool {
    c.strip_prefix("user@")
        .and_then(|rest| rest.strip_suffix(".service"))
        .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()))
}

/// `hidden_by_hide_system` of record `i`, false when `hide_system` is off.
fn hidden(input: &TableInput<'_>, i: usize) -> bool {
    input.settings.hide_system && {
        let r = &input.records[i];
        hidden_by_hide_system(r, group_class(r))
    }
}

/// How bad a validity is, for worst-of: a value that is there beats one that is not.
const fn severity(v: Validity) -> u8 {
    match v {
        Validity::Valid => 0,
        Validity::Stale => 1,
        Validity::WarmingUp => 2,
        Validity::Restricted => 3,
        Validity::Unsupported => 4,
        Validity::Missing => 5,
        Validity::Failed => 6,
    }
}

fn worse(a: Validity, b: Validity) -> Validity {
    if severity(b) > severity(a) { b } else { a }
}

/// Aggregates `members` (record indexes) into one group row. Each member counts once
/// (ADR-009).
///
/// CPU, disk, network, GPU, threads and the byte fields are sums over the members. Each field's
/// validity is the worst of its members, so one `Restricted` member makes the field `Restricted`.
/// Memory is the cgroup `anon` of `stats` (app) or `unit` when given, with the cgroup's
/// validity, else the RssAnon sum (PROC-008, R-137). When a member's disk is `Restricted`, an
/// app row takes disk from its cgroup `io.stat` rate instead (R-103). `pids` is the cgroup's
/// process count, else the member count.
///
/// The identity fields are a starting point the grouped builder overwrites: node
/// `NodeId::Group` of the cgroup's (else the first member's) group key, kind `Unit` when `unit`
/// is given and `App` otherwise, class, name and icon of the first member that has an app id,
/// else of the first member.
pub fn aggregate(
    records: &[ProcessRecord],
    members: &[u32],
    stats: Option<&AppCgroupSample>,
    unit: Option<&CgroupStats>,
) -> GroupRow {
    use ProcessField as F;

    let mut validity = FieldValidity::all(Validity::Valid);
    let (mut cpu, mut io_read, mut io_write) = (0f32, 0f32, 0f32);
    let (mut net_rx, mut net_tx, mut gpu) = (0f32, 0f32, 0f32);
    let (mut rss_anon, mut rss, mut pss, mut uss) = (0u64, 0u64, 0u64, 0u64);
    let (mut swap, mut gpu_mem, mut threads) = (0u64, 0u64, 0u32);
    let mut disk_restricted = false;
    let mut first: Option<&ProcessRecord> = None;
    for &i in members {
        let Some(r) = records.get(i as usize) else {
            continue;
        };
        if first.is_none_or(|f| f.meta.app_id.is_none() && r.meta.app_id.is_some()) {
            first = Some(r);
        }
        cpu += r.cpu_pct;
        io_read += r.io_read_bps;
        io_write += r.io_write_bps;
        net_rx += r.net_rx_bps;
        net_tx += r.net_tx_bps;
        gpu += r.gpu_util_pct;
        rss_anon = rss_anon.saturating_add(r.rss_anon_bytes);
        rss = rss.saturating_add(r.rss_bytes);
        pss = pss.saturating_add(r.pss_bytes);
        uss = uss.saturating_add(r.uss_bytes);
        swap = swap.saturating_add(r.swap_bytes);
        gpu_mem = gpu_mem.saturating_add(r.gpu_mem_bytes);
        threads = threads.saturating_add(r.threads);
        for f in ProcessField::ALL {
            validity.set(f, worse(validity.get(f), r.validity.get(f)));
        }
        disk_restricted |= r.validity.get(F::IoRead) == Validity::Restricted
            || r.validity.get(F::IoWrite) == Validity::Restricted;
    }

    if let Some(s) = stats {
        rss_anon = s.anon_bytes;
        validity.set(F::RssAnon, s.validity.get(F::RssAnon));
        if disk_restricted {
            io_read = s.io_read_bps;
            io_write = s.io_write_bps;
            validity.set(F::IoRead, s.validity.get(F::IoRead));
            validity.set(F::IoWrite, s.validity.get(F::IoWrite));
        }
    } else if let Some(u) = unit {
        rss_anon = u.anon_bytes;
        validity.set(F::RssAnon, u.validity);
    }

    // A group never holds more than u32::MAX records.
    let pids = stats
        .map(|s| s.pids)
        .or(unit.map(|u| u.pids_current))
        .unwrap_or(members.len() as u32);
    let key = stats
        .map(|s| s.group)
        .or(unit.map(|u| u.group))
        .or(first.map(|r| r.group))
        .unwrap_or(GroupKey(0));
    let class = match first.map(|r| (group_class(r), r)) {
        Some((GroupClass::Apps, r)) => GroupKind::App {
            app_id: r.meta.app_id.clone().unwrap_or_else(|| Arc::from("")),
        },
        Some((GroupClass::Background, _)) => GroupKind::Background,
        Some((GroupClass::System, _)) | None => GroupKind::System,
    };

    GroupRow {
        node: NodeId::Group(key),
        key,
        kind: if unit.is_some() {
            GroupRowKind::Unit
        } else {
            GroupRowKind::App
        },
        class,
        name: first.map_or_else(|| Arc::from(""), |r| r.meta.name.clone()),
        icon: first.and_then(|r| r.meta.icon.clone()),
        members: members.len() as u32,
        pids,
        note: None,
        memory_bytes: rss_anon,
        disk_bps: io_read + io_write,
        net_bps: net_rx + net_tx,
        gpu_pct: gpu,
        cpu_pct: cpu,
        rss_anon_bytes: rss_anon,
        rss_bytes: rss,
        pss_bytes: pss,
        uss_bytes: uss,
        swap_bytes: swap,
        io_read_bps: io_read,
        io_write_bps: io_write,
        net_rx_bps: net_rx,
        net_tx_bps: net_tx,
        gpu_util_pct: gpu,
        gpu_mem_bytes: gpu_mem,
        threads,
        cpu_kernel_pct: 0.0,
        cpu_user_pct: 0.0,
        cpu_relative_pct: 0.0,
        utime_ticks: 0,
        stime_ticks: 0,
        commit_bytes: 0,
        page_faults: 0,
        page_faults_delta: 0,
        major_faults: 0,
        handles: None,
        desktop_path: if unit.is_none() {
            first.and_then(|r| r.meta.desktop_path.clone())
        } else {
            None
        },
        description: None,
        user: None,
        validity,
    }
}

/// Adds the `ProcessExtras` sums of `members` into `g` (PROC-029, PROC-030, PROC-035). Leaves
/// them as they are when `extras` is not index aligned with the `records_len` records yet.
fn add_extras_sums(
    g: &mut GroupRow,
    extras: &[ProcessExtras],
    records_len: usize,
    members: &[u32],
) {
    if extras.len() != records_len {
        return;
    }
    let mut handles = Some(0u32);
    for &i in members {
        let Some(x) = extras.get(i as usize) else {
            continue;
        };
        g.cpu_kernel_pct += x.cpu_kernel_pct;
        g.cpu_user_pct += x.cpu_user_pct;
        g.cpu_relative_pct += x.cpu_relative_pct;
        g.utime_ticks = g.utime_ticks.saturating_add(x.utime_ticks);
        g.stime_ticks = g.stime_ticks.saturating_add(x.stime_ticks);
        g.commit_bytes = g.commit_bytes.saturating_add(x.commit_bytes);
        g.page_faults = g.page_faults.saturating_add(x.page_faults);
        g.page_faults_delta = g.page_faults_delta.saturating_add(x.page_faults_delta);
        g.major_faults = g.major_faults.saturating_add(x.major_faults);
        handles = handles.zip(x.handles).map(|(a, b)| a.saturating_add(b));
    }
    g.handles = handles;
}

/// Adds the sums of `from` into `into`. Each field's validity becomes the worse of the two.
fn add_sums(into: &mut GroupRow, from: &GroupRow) {
    into.members = into.members.saturating_add(from.members);
    into.pids = into.pids.saturating_add(from.pids);
    into.memory_bytes = into.memory_bytes.saturating_add(from.memory_bytes);
    into.disk_bps += from.disk_bps;
    into.net_bps += from.net_bps;
    into.gpu_pct += from.gpu_pct;
    into.cpu_pct += from.cpu_pct;
    into.rss_anon_bytes = into.rss_anon_bytes.saturating_add(from.rss_anon_bytes);
    into.rss_bytes = into.rss_bytes.saturating_add(from.rss_bytes);
    into.pss_bytes = into.pss_bytes.saturating_add(from.pss_bytes);
    into.uss_bytes = into.uss_bytes.saturating_add(from.uss_bytes);
    into.swap_bytes = into.swap_bytes.saturating_add(from.swap_bytes);
    into.io_read_bps += from.io_read_bps;
    into.io_write_bps += from.io_write_bps;
    into.net_rx_bps += from.net_rx_bps;
    into.net_tx_bps += from.net_tx_bps;
    into.gpu_util_pct += from.gpu_util_pct;
    into.gpu_mem_bytes = into.gpu_mem_bytes.saturating_add(from.gpu_mem_bytes);
    into.threads = into.threads.saturating_add(from.threads);
    into.cpu_kernel_pct += from.cpu_kernel_pct;
    into.cpu_user_pct += from.cpu_user_pct;
    into.cpu_relative_pct += from.cpu_relative_pct;
    into.utime_ticks = into.utime_ticks.saturating_add(from.utime_ticks);
    into.stime_ticks = into.stime_ticks.saturating_add(from.stime_ticks);
    into.commit_bytes = into.commit_bytes.saturating_add(from.commit_bytes);
    into.page_faults = into.page_faults.saturating_add(from.page_faults);
    into.page_faults_delta = into
        .page_faults_delta
        .saturating_add(from.page_faults_delta);
    into.major_faults = into.major_faults.saturating_add(from.major_faults);
    into.handles = into
        .handles
        .zip(from.handles)
        .map(|(a, b)| a.saturating_add(b));
    for f in ProcessField::ALL {
        into.validity
            .set(f, worse(into.validity.get(f), from.validity.get(f)));
    }
}

/// Subtree totals (PROC-019, R-005): one row per entry of `visible` (record indexes, in that
/// order) that has children in `f`, summing the record and all its descendants in `f`.
///
/// Each process counts once. Memory is the RssAnon sum. Each field's validity is the worst over
/// the subtree. `f` holds only the visible records, so the sums cover visible descendants only.
/// A row's `node` is the record's `NodeId::Process`, its name and icon the record's. The sums
/// are computed bottom up for every record in one pass, O(n).
pub fn subtree_totals(f: &Forest, records: &[ProcessRecord], visible: &[u32]) -> Vec<GroupRow> {
    let n = f.children.len().min(records.len());
    let in_range = |i: u32| (i as usize) < n;

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

    let mut sums: Vec<Option<GroupRow>> = (0..n).map(|_| None).collect();
    for &i in pre.iter().rev() {
        let r = &records[i as usize];
        let mut row = aggregate(records, &[i], None, None);
        row.node = NodeId::Process(r.key);
        row.name = r.meta.name.clone();
        row.icon = r.meta.icon.clone();
        for &c in &f.children[i as usize] {
            // Children come later in pre-order, so they are summed already. In an acyclic
            // forest that is every child.
            if in_range(c)
                && c != i
                && let Some(child) = &sums[c as usize]
            {
                add_sums(&mut row, child);
            }
        }
        sums[i as usize] = Some(row);
    }

    visible
        .iter()
        .filter(|&&i| in_range(i) && !f.children[i as usize].is_empty())
        .filter_map(|&i| sums[i as usize].clone())
        .collect()
}

/// The sort value of a group row in column `col`: the aggregated value it displays. Columns a
/// group row leaves blank (PID and the rest) are `Absent`.
fn group_sort_key(g: &GroupRow, col: ColumnId) -> SortKey {
    use ColumnId as C;
    use ProcessField as F;

    let usable = |field: F| matches!(g.validity.get(field), Validity::Valid | Validity::Stale);
    let checked = |field: F, key: SortKey| {
        if usable(field) { key } else { SortKey::Absent }
    };
    let rate_pair = |a: F, b: F, sum: f32| {
        if usable(a) && usable(b) {
            SortKey::float(sum)
        } else {
            SortKey::Absent
        }
    };
    let int = |v: u64| SortKey::Int(v.into());

    match col {
        C::Name | C::NameDisplay => SortKey::Text(g.name.clone()),
        C::Type => SortKey::Text(Arc::from(match g.kind {
            GroupRowKind::App => "App",
            GroupRowKind::Unit => "Unit",
            GroupRowKind::KernelThreads => "Kernel thread",
            GroupRowKind::Pseudo => "Pseudo",
        })),
        C::Cpu => checked(F::Cpu, SortKey::float(g.cpu_pct)),
        C::Memory => checked(F::RssAnon, int(g.memory_bytes)),
        C::WsTotal => checked(F::Rss, int(g.rss_bytes)),
        C::Swap => checked(F::Swap, int(g.swap_bytes)),
        C::Disk | C::DiskTotalRate => rate_pair(F::IoRead, F::IoWrite, g.disk_bps),
        C::DiskReadRate => checked(F::IoRead, SortKey::float(g.io_read_bps)),
        C::DiskWriteRate => checked(F::IoWrite, SortKey::float(g.io_write_bps)),
        C::Network => rate_pair(F::NetRx, F::NetTx, g.net_bps),
        C::NetSendRate => checked(F::NetTx, SortKey::float(g.net_tx_bps)),
        C::NetRecvRate => checked(F::NetRx, SortKey::float(g.net_rx_bps)),
        C::Gpu => checked(F::GpuUtil, SortKey::float(g.gpu_pct)),
        C::GpuMemDedicated => checked(F::GpuMem, int(g.gpu_mem_bytes)),
        C::Threads => checked(F::Threads, int(g.threads.into())),
        C::CpuKernel => checked(F::Cpu, SortKey::float(g.cpu_kernel_pct)),
        C::CpuUser => checked(F::Cpu, SortKey::float(g.cpu_user_pct)),
        C::CpuRelative => checked(F::Cpu, SortKey::float(g.cpu_relative_pct)),
        C::CpuTime => SortKey::Int(i128::from(g.utime_ticks) + i128::from(g.stime_ticks)),
        C::CpuTimeKernel => int(g.stime_ticks),
        C::CpuTimeUser => int(g.utime_ticks),
        C::Commit => int(g.commit_bytes),
        C::PageFaults => int(g.page_faults),
        C::PageFaultsDelta => int(g.page_faults_delta),
        C::MajorFaults => int(g.major_faults),
        C::Handles => g.handles.map_or(SortKey::Absent, |h| int(h.into())),
        C::Unit => match &g.node {
            NodeId::Unit(u) => SortKey::Text(u.name.clone()),
            _ => SortKey::Absent,
        },
        C::Description => SortKey::text(g.description.as_ref()),
        C::User if g.kind == GroupRowKind::Pseudo => SortKey::Text(Arc::from("root")),
        C::User => SortKey::text(g.user.as_ref()),
        _ => SortKey::Absent,
    }
}

/// The order key of record `i`: its sort value at the shown step.
fn record_key(input: &TableInput<'_>, i: u32, column: ColumnId) -> SortKey {
    let r = &input.records[i as usize];
    let key = match input.extras.get(i as usize) {
        Some(x) => sort_key(r, x, column),
        None => record_only_key(r, column),
    };
    shown_key(key, column, input.settings)
}

/// One row directly under a group header: a process or an index into the group rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Record(u32),
    Group(u32),
}

/// Sorts the rows under one header by `sort`. Group rows sort by their aggregated value and mix
/// with process rows. Ties: group rows by name, then key, before process rows, which go by
/// `ProcessKey`.
fn sort_items(input: &TableInput<'_>, groups: &[GroupRow], items: &mut [Item], sort: SortSpec) {
    let (column, ascending) = effective_sort(sort);
    let mut keyed: Vec<(SortKey, Item)> = items
        .iter()
        .map(|&it| {
            let key = match it {
                Item::Record(i) => record_key(input, i, column),
                Item::Group(g) => shown_key(
                    group_sort_key(&groups[g as usize], column),
                    column,
                    input.settings,
                ),
            };
            (key, it)
        })
        .collect();
    let records = input.records;
    keyed.sort_unstable_by(|(ka, a), (kb, b)| {
        cmp_keys(ka, kb, ascending).then_with(|| match (a, b) {
            (Item::Record(x), Item::Record(y)) => {
                records[*x as usize].key.cmp(&records[*y as usize].key)
            }
            (Item::Group(x), Item::Group(y)) => {
                let (gx, gy) = (&groups[*x as usize], &groups[*y as usize]);
                cmp_text(&gx.name, &gy.name)
                    .then_with(|| gx.key.cmp(&gy.key))
                    .then_with(|| x.cmp(y))
            }
            (Item::Group(_), Item::Record(_)) => Ordering::Less,
            (Item::Record(_), Item::Group(_)) => Ordering::Greater,
        })
    });
    for (slot, (_, it)) in items.iter_mut().zip(keyed) {
        *slot = it;
    }
}

/// The unit's cgroup sample: the one whose path ends in the unit name, under `user@` for a
/// user-manager unit and outside it for a system unit.
fn unit_stat<'a>(stats: &'a [CgroupStats], unit: &UnitRef) -> Option<&'a CgroupStats> {
    stats.iter().find(|s| {
        let path: &str = &s.cgroup.rel_path;
        path.rsplit('/').next() == Some(&*unit.name)
            && path.contains("/user@") == (unit.manager == Manager::User)
    })
}

/// A process row with no children shown, as in flat mode.
fn leaf_row(input: &TableInput<'_>, i: u32, depth: u8) -> TableRow {
    TableRow {
        node: NodeId::Process(input.records[i as usize].key),
        depth,
        expanded: false,
        has_children: false,
        record: Some(i),
        group: None,
        descendants: 0,
        subtree: None,
    }
}

/// A group row (app, unit or Kernel threads) at depth 1 under its header.
fn group_row(groups: &[GroupRow], g: u32, expanded: &FxHashSet<NodeId>) -> TableRow {
    let gr = &groups[g as usize];
    TableRow {
        node: gr.node.clone(),
        depth: 1,
        expanded: expanded.contains(&gr.node),
        has_children: gr.members > 0,
        record: None,
        group: Some(g),
        descendants: gr.members,
        subtree: None,
    }
}

/// A pseudo row with every value 0 and every field `Unsupported`.
fn pseudo_base(kind: PseudoKind, name: &str) -> GroupRow {
    GroupRow {
        node: NodeId::Pseudo(kind),
        key: GroupKind::System.key(),
        kind: GroupRowKind::Pseudo,
        class: GroupKind::System,
        name: Arc::from(name),
        icon: None,
        members: 0,
        pids: 0,
        note: None,
        memory_bytes: 0,
        disk_bps: 0.0,
        net_bps: 0.0,
        gpu_pct: 0.0,
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
        threads: 0,
        cpu_kernel_pct: 0.0,
        cpu_user_pct: 0.0,
        cpu_relative_pct: 0.0,
        utime_ticks: 0,
        stime_ticks: 0,
        commit_bytes: 0,
        page_faults: 0,
        page_faults_delta: 0,
        major_faults: 0,
        handles: None,
        desktop_path: None,
        description: None,
        user: None,
        validity: FieldValidity::all(Validity::Unsupported),
    }
}

/// The pseudo rows (PROC-012): "Interrupts" always, then "Memory compression (zram)" when a
/// zram device has `disksize > 0`. Fields a row does not define are `Unsupported`.
///
/// Interrupts: CPU is `irq_pct`, or `Unsupported` with the `IRQ_NOT_ACCOUNTED` note when it is
/// `None`. Memory compression: Memory is Σ `mem_used_total`, disk read and write are Σ of the
/// zram rates over the set-up devices, and the note gives the original and compressed sizes and
/// their ratio.
pub fn pseudo_rows(input: &PseudoInput) -> Vec<GroupRow> {
    use ProcessField as F;

    let mut out = Vec::with_capacity(2);
    let mut irq = pseudo_base(PseudoKind::Interrupts, "Interrupts");
    match input.irq_pct {
        Some(pct) => {
            irq.cpu_pct = pct;
            irq.validity.set(F::Cpu, Validity::Valid);
        }
        None => irq.note = Some(Arc::from(IRQ_NOT_ACCOUNTED)),
    }
    out.push(irq);

    let mut set_up = input.zram.iter().filter(|z| z.disksize > 0).peekable();
    if set_up.peek().is_some() {
        let mut z = pseudo_base(PseudoKind::MemoryCompression, "Memory compression (zram)");
        let (mut orig, mut compr) = (0u64, 0u64);
        for d in set_up {
            z.rss_anon_bytes = z.rss_anon_bytes.saturating_add(d.mem_used_total);
            z.io_read_bps += d.read_bps;
            z.io_write_bps += d.write_bps;
            orig = orig.saturating_add(d.orig_bytes);
            compr = compr.saturating_add(d.compr_bytes);
        }
        z.memory_bytes = z.rss_anon_bytes;
        z.disk_bps = z.io_read_bps + z.io_write_bps;
        for f in [F::RssAnon, F::IoRead, F::IoWrite] {
            z.validity.set(f, Validity::Valid);
        }
        let mut note = String::from("Original ");
        units::bytes(&mut note, orig);
        note.push_str(" · Compressed ");
        units::bytes(&mut note, compr);
        if compr == 0 {
            note.push_str(" · Ratio n/a");
        } else {
            // The casts lose precision only above 2^53 bytes, far below two decimals.
            let _ = write!(note, " · Ratio {:.2}", orig as f64 / compr as f64);
        }
        z.note = Some(Arc::from(note));
        out.push(z);
    }
    out
}

/// The pseudo rows that pass `hide_system` and `filter`. Pseudo rows sit in System, so
/// `hide_system` hides both (PROC-046). The filter matches the row name, the user "root" and
/// `extra`.
fn shown_pseudo_rows(input: &TableInput<'_>, filter: &Filter, extra: &str) -> Vec<GroupRow> {
    let Some(pseudo) = input.pseudo else {
        return Vec::new();
    };
    if input.settings.hide_system {
        return Vec::new();
    }
    let mut rows = pseudo_rows(pseudo);
    rows.retain(|g| filter.is_empty() || filter.matches_text(&[&g.name, "root", extra]));
    rows
}

/// The table row of pseudo row `g` (an index into the view's group rows).
fn pseudo_table_row(groups: &[GroupRow], g: u32, depth: u8) -> TableRow {
    TableRow {
        node: groups[g as usize].node.clone(),
        depth,
        expanded: false,
        has_children: false,
        record: None,
        group: Some(g),
        descendants: 0,
        subtree: None,
    }
}

/// Marks a record that belongs to no app or unit row.
const NO_OWNER: u32 = u32::MAX;

/// Cuts every forest link between records of different owners, so each app or unit holds only
/// its own members' subtrees. The cut child becomes a root.
fn restrict_to_owners(f: &mut Forest, owner: &[u32]) {
    for i in 0..f.parent.len() {
        let Some(p) = f.parent[i] else {
            continue;
        };
        if owner.get(i) != owner.get(p as usize) {
            f.parent[i] = None;
            f.children[p as usize].retain(|c| *c as usize != i);
            // The snapshot never holds more than u32::MAX records.
            f.roots.push(i as u32);
        }
    }
}

/// Builds grouped mode (PROC-004, PROC-008, PROC-009): the rows and the group rows they index.
///
/// With `hide_system`, the records of `hidden_by_hide_system` are dropped first and the System
/// header is left out. The other headers come in `GROUP_ORDER` at depth 0, `descendants` set to the number of
/// processes under the header after filtering. A header's rows follow only when it is in
/// `query.expanded`. Under Apps: one row per app at depth 1, sorted by its aggregated value,
/// holding the app's primary processes at depth 2, each with its subtree among the app's
/// members. Under Background and System: the processes of the class at depth 1, except that the
/// processes of a systemd unit with more than one process fold under a unit row at depth 1 and
/// sit at depth 2. Kernel threads fold under one "Kernel threads" row at depth 1, first in
/// System, with the threads as a flat sorted list at depth 2 (UI-PRC-002). The pseudo rows are
/// the last System rows at depth 1, Interrupts first, and the header count leaves them out
/// (PROC-012). A filter term also matches a group, app or unit name, which matches every member
/// of it. Each process counts once.
pub fn build_grouped(
    input: &TableInput<'_>,
    query: &TableQuery,
    filter: &Filter,
) -> (Vec<TableRow>, Vec<GroupRow>) {
    let (rows, groups, _) = grouped_rows(input, query, filter);
    (rows, groups)
}

/// `build_grouped` plus the number of processes shown.
fn grouped_rows(
    input: &TableInput<'_>,
    query: &TableQuery,
    filter: &Filter,
) -> (Vec<TableRow>, Vec<GroupRow>, u32) {
    let records = input.records;
    let n = records.len();
    let class: Vec<GroupClass> = records.iter().map(group_class).collect();
    // Applied before grouping, so a hidden record counts nowhere (PROC-046).
    let hidden: Vec<bool> = if input.settings.hide_system {
        records
            .iter()
            .zip(&class)
            .map(|(r, c)| hidden_by_hide_system(r, *c))
            .collect()
    } else {
        vec![false; n]
    };
    let is_kthread = |i: usize| records[i].flags.contains(ProcFlags::KERNEL_THREAD);
    let unit_of = |i: usize| input.extras.get(i).and_then(|x| x.unit.as_ref());

    // Lowercase app names for the filter: the name of the app's first record with an app id,
    // else of its first record.
    let mut app_names: FxHashMap<GroupKey, (bool, String)> = FxHashMap::default();
    for (i, r) in records.iter().enumerate() {
        if class[i] != GroupClass::Apps {
            continue;
        }
        let primary = r.meta.app_id.is_some();
        match app_names.get_mut(&r.group) {
            None => {
                app_names.insert(r.group, (primary, r.meta.name.to_lowercase()));
            }
            Some(entry) if primary && !entry.0 => *entry = (true, r.meta.name.to_lowercase()),
            Some(_) => {}
        }
    }

    // Processes per (class, unit) before filtering: a unit folds when it has more than one.
    let mut unit_total: FxHashMap<(GroupClass, &UnitRef), u32> = FxHashMap::default();
    for (i, c) in class.iter().enumerate() {
        if *c != GroupClass::Apps
            && !hidden[i]
            && !is_kthread(i)
            && let Some(u) = unit_of(i)
        {
            *unit_total.entry((*c, u)).or_default() += 1;
        }
    }

    // One pass to filter and bucket.
    let mut counts = [0u32; 3];
    let mut apps: Vec<(GroupKey, Vec<u32>)> = Vec::new();
    let mut app_index: FxHashMap<GroupKey, usize> = FxHashMap::default();
    let mut units: Vec<(GroupClass, &UnitRef, Vec<u32>)> = Vec::new();
    let mut unit_index: FxHashMap<(GroupClass, &UnitRef), usize> = FxHashMap::default();
    let mut loose: [Vec<u32>; 3] = Default::default();
    let mut kthreads: Vec<u32> = Vec::new();
    for (i, r) in records.iter().enumerate() {
        let c = class[i];
        if hidden[i] {
            continue;
        }
        let pass = filter.is_empty() || {
            let [unit, description] = extra_strings(input, input.extras.get(i));
            let app = if c == GroupClass::Apps {
                app_names.get(&r.group).map_or("", |a| a.1.as_str())
            } else {
                ""
            };
            filter.matches(r, &[c.name_lower(), app, unit, description])
        };
        if !pass {
            continue;
        }
        counts[c.index()] += 1;
        // The snapshot never holds more than u32::MAX records.
        let idx = i as u32;
        if is_kthread(i) {
            kthreads.push(idx);
        } else if c == GroupClass::Apps {
            let a = *app_index.entry(r.group).or_insert_with(|| {
                apps.push((r.group, Vec::new()));
                apps.len() - 1
            });
            apps[a].1.push(idx);
        } else {
            match unit_of(i) {
                Some(u) if unit_total.get(&(c, u)).copied().unwrap_or(0) > 1 => {
                    let k = *unit_index.entry((c, u)).or_insert_with(|| {
                        units.push((c, u, Vec::new()));
                        units.len() - 1
                    });
                    units[k].2.push(idx);
                }
                _ => loose[c.index()].push(idx),
            }
        }
    }

    // One forest over every app and unit member, links cut between owners.
    let mut owner = vec![NO_OWNER; n];
    let members = apps
        .iter()
        .map(|(_, m)| m)
        .chain(units.iter().map(|(_, _, m)| m));
    for (o, list) in members.enumerate() {
        for &i in list {
            owner[i as usize] = o as u32;
        }
    }
    let visible: Vec<bool> = owner.iter().map(|&o| o != NO_OWNER).collect();
    let mut f = forest(records, &visible);
    restrict_to_owners(&mut f, &owner);
    let descendants = descendant_counts(records, &f);
    let mut roots_of: Vec<Vec<u32>> = vec![Vec::new(); apps.len() + units.len()];
    for &r in &f.roots {
        if let Some(list) = roots_of.get_mut(owner[r as usize] as usize) {
            list.push(r);
        }
    }

    // Group rows: apps, then units, then Kernel threads, so a group's index is its owner.
    let app_stats: FxHashMap<GroupKey, &AppCgroupSample> =
        input.app_stats.iter().map(|s| (s.group, s)).collect();
    let mut groups: Vec<GroupRow> = Vec::with_capacity(apps.len() + units.len() + 1);
    for (key, members) in &apps {
        let mut g = aggregate(records, members, app_stats.get(key).copied(), None);
        add_extras_sums(&mut g, input.extras, n, members);
        g.node = NodeId::Group(*key);
        g.key = *key;
        g.kind = GroupRowKind::App;
        groups.push(g);
    }
    for (c, unit, members) in &units {
        let mut g = aggregate(records, members, None, unit_stat(input.unit_stats, unit));
        add_extras_sums(&mut g, input.extras, n, members);
        g.node = NodeId::Unit((*unit).clone());
        g.kind = GroupRowKind::Unit;
        g.class = if *c == GroupClass::Background {
            GroupKind::Background
        } else {
            GroupKind::System
        };
        g.description = input.unit_names.get(*unit).cloned();
        g.user = input.unit_users.get(*unit).cloned();
        g.name = g.description.clone().unwrap_or_else(|| unit.name.clone());
        g.icon = None;
        groups.push(g);
    }
    let kernel_group = (!kthreads.is_empty()).then(|| {
        let mut g = aggregate(records, &kthreads, None, None);
        add_extras_sums(&mut g, input.extras, n, &kthreads);
        g.node = NodeId::KernelThreads;
        g.key = GroupKind::System.key();
        g.kind = GroupRowKind::KernelThreads;
        g.class = GroupKind::System;
        g.name = Arc::from("Kernel threads");
        g.icon = None;
        groups.push(g);
        (groups.len() - 1) as u32
    });
    // Pseudo rows come last in `groups` and close the System header (PROC-012).
    let pseudo_from = groups.len();
    groups.extend(shown_pseudo_rows(
        input,
        filter,
        GroupClass::System.name_lower(),
    ));

    // Rows, header by header.
    let expanded: FxHashSet<NodeId> = query.expanded.iter().cloned().collect();
    let order = |ids: &[u32]| {
        let mut v = ids.to_vec();
        sort_indices(input, &mut v, query.sort);
        v
    };
    let tree = TreeEmit {
        records,
        f: &f,
        descendants: &descendants,
        order: &order,
        expanded: &expanded,
    };
    let mut emitted = vec![false; descendants.len()];
    let shown: u32 = counts.iter().sum();
    let mut rows = Vec::with_capacity(shown as usize + groups.len() + GROUP_ORDER.len());
    for c in GROUP_ORDER {
        if c == GroupClass::System && input.settings.hide_system {
            // `hide_system` removes the whole group, header included (PROC-046).
            continue;
        }
        let node = NodeId::Group(c.key());
        // Pseudo rows are not processes: they are not in the count (PROC-012).
        let count = counts[c.index()];
        let pseudo = if c == GroupClass::System {
            pseudo_from..groups.len()
        } else {
            0..0
        };
        let is_expanded = expanded.contains(&node);
        rows.push(TableRow {
            node,
            depth: 0,
            expanded: is_expanded,
            has_children: count > 0 || !pseudo.is_empty(),
            record: None,
            group: None,
            descendants: count,
            subtree: None,
        });
        if !is_expanded {
            continue;
        }

        if c == GroupClass::System
            && let Some(g) = kernel_group
        {
            let row = group_row(&groups, g, &expanded);
            let open = row.expanded;
            rows.push(row);
            if open {
                for i in order(&kthreads) {
                    rows.push(leaf_row(input, i, 2));
                }
            }
        }

        let mut items: Vec<Item> = loose[c.index()].iter().map(|&i| Item::Record(i)).collect();
        if c == GroupClass::Apps {
            items.extend((0..apps.len()).map(|a| Item::Group(a as u32)));
        } else {
            items.extend(
                units
                    .iter()
                    .enumerate()
                    .filter(|(_, (uc, _, _))| *uc == c)
                    .map(|(u, _)| Item::Group((apps.len() + u) as u32)),
            );
        }
        sort_items(input, &groups, &mut items, query.sort);
        for item in items {
            match item {
                Item::Record(i) => rows.push(leaf_row(input, i, 1)),
                Item::Group(g) => {
                    let row = group_row(&groups, g, &expanded);
                    let open = row.expanded;
                    rows.push(row);
                    if open {
                        tree.emit(&roots_of[g as usize], 2, &mut emitted, &mut rows);
                    }
                }
            }
        }
        for g in pseudo {
            // `groups` never holds more than u32::MAX rows.
            rows.push(pseudo_table_row(&groups, g as u32, 1));
        }
    }
    (rows, groups, shown)
}

/// Flat mode rows: every match and the shown pseudo rows at depth 0, sorted by `query.sort`.
/// The group rows are the pseudo rows.
fn flat_rows(
    input: &TableInput<'_>,
    query: &TableQuery,
    filter: &Filter,
) -> (Vec<TableRow>, Vec<GroupRow>, u32) {
    let mut indexes = filtered(input, filter);
    let shown = indexes.len() as u32;
    let pseudo = shown_pseudo_rows(input, filter, "");
    if pseudo.is_empty() {
        sort_indices(input, &mut indexes, query.sort);
        let rows = indexes.iter().map(|&i| leaf_row(input, i, 0)).collect();
        return (rows, pseudo, shown);
    }

    let mut items: Vec<Item> = indexes.into_iter().map(Item::Record).collect();
    items.extend((0..pseudo.len()).map(|g| Item::Group(g as u32)));
    sort_items(input, &pseudo, &mut items, query.sort);
    let rows = items
        .into_iter()
        .map(|it| match it {
            Item::Record(i) => leaf_row(input, i, 0),
            Item::Group(g) => pseudo_table_row(&pseudo, g, 0),
        })
        .collect();
    (rows, pseudo, shown)
}

/// Builds the process table for `query` (ADR-009).
pub fn build(input: &TableInput<'_>, query: &TableQuery) -> ProcessTableView {
    let filter = Filter::parse(&query.filter);
    let (rows, groups, subtree, shown) = match query.mode {
        TableMode::Tree => tree_rows(input, query, &filter),
        TableMode::Flat => {
            let (rows, groups, shown) = flat_rows(input, query, &filter);
            (rows, groups, Vec::new(), shown)
        }
        TableMode::Grouped => {
            let (rows, groups, shown) = grouped_rows(input, query, &filter);
            (rows, groups, Vec::new(), shown)
        }
    };

    let restricted = input
        .records
        .iter()
        .filter(|r| r.validity.0.contains(&Validity::Restricted))
        .count();

    // Records left after `hide_system`, before the text filter (R-280).
    let total = (0..input.records.len())
        .filter(|&i| !hidden(input, i))
        .count();

    // Counts fit u32: the snapshot never holds more than u32::MAX records.
    ProcessTableView {
        header: Default::default(),
        query_generation: query.generation,
        total: total as u32,
        shown,
        restricted: restricted as u32,
        rows: Arc::from(rows),
        groups: Arc::from(groups),
        subtree: Arc::from(subtree),
        totals: TableTotals::default(),
    }
}

/// The process sweep numbers the header totals need (E08-003).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SweepTotals {
    pub system_threads: u32,
    /// `/proc/sys/fs/file-nr` field 1. `None` when the file could not be read (R-166 B3-7).
    pub file_nr: Option<u64>,
}

/// Bytes per second in 1 Mbit/s.
const BYTES_PER_MBIT: u64 = 125_000;

/// A value that may be read: `Valid` or `Stale`.
const fn readable(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

fn total<T: Copy + Default>(m: Metric<T>) -> Total<T> {
    Total {
        value: m.value,
        validity: m.validity,
    }
}

fn state<T: Default>(validity: Validity) -> Total<T> {
    Total {
        value: T::default(),
        validity,
    }
}

/// The worst validity of the values a device provides. A device that does not provide the
/// metric (`Unsupported`) is skipped; `Unsupported` when no device provides it.
fn combine(vs: impl Iterator<Item = Validity>) -> Validity {
    vs.filter(|&v| v != Validity::Unsupported)
        .reduce(worse)
        .unwrap_or(Validity::Unsupported)
}

/// Highest readable value, with the combined validity. `empty` when there is no device.
fn max_total(ms: &[Metric<f32>], empty: Validity) -> Total<f32> {
    if ms.is_empty() {
        return state(empty);
    }
    let validity = combine(ms.iter().map(|m| m.validity));
    if !readable(validity) {
        return state(validity);
    }
    let value = ms
        .iter()
        .filter(|m| readable(m.validity))
        .map(|m| m.value)
        .fold(0.0, f32::max);
    Total { value, validity }
}

/// The present catalog entry of `idx` when it is of `kind`.
fn present(devices: &DeviceCatalog, idx: DeviceIdx, kind: DeviceKind) -> Option<&DeviceEntry> {
    devices
        .get(idx)
        .filter(|e| e.present && e.identity.kind() == kind)
}

/// Physical interfaces carry the machine's own traffic. VPN tunnels and virtual interfaces
/// repeat traffic that a physical interface also counts, so they are left out.
fn physical_net(e: &DeviceEntry) -> bool {
    matches!(
        e.net_kind,
        Some(NetKind::Wired | NetKind::Wireless | NetKind::Other)
    )
}

/// `domain:bus:device.function` as numbers, so `0000:2d:00.0` and NVML's `00000000:2D:00.0`
/// order the same way.
fn pci_order(bus_id: &str) -> Option<(u32, u8, u8, u8)> {
    let (rest, function) = bus_id.rsplit_once('.')?;
    let mut parts = rest.split(':');
    let domain = u32::from_str_radix(parts.next()?, 16).ok()?;
    let bus = u8::from_str_radix(parts.next()?, 16).ok()?;
    let device = u8::from_str_radix(parts.next()?, 16).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((domain, bus, device, u8::from_str_radix(function, 16).ok()?))
}

/// The present GPUs in the order of the `gpu_0` to `gpu_15` columns: by PCI bus id, with
/// unparsable ids last by text. Each entry is the GPU's identity and its totals.
pub fn gpu_columns<'a>(
    system: &'a SystemSnapshot,
    devices: &'a DeviceCatalog,
) -> SmallVec<[(&'a DeviceIdentity, &'a GpuTotals); 4]> {
    let mut gpus: SmallVec<[(&str, &DeviceIdentity, &GpuTotals); 4]> = system
        .gpus
        .iter()
        .filter_map(|(idx, g)| {
            let identity = &present(devices, *idx, DeviceKind::Gpu)?.identity;
            match identity {
                DeviceIdentity::Gpu { pci_bus_id, .. } => Some((&**pci_bus_id, identity, g)),
                _ => None,
            }
        })
        .collect();
    gpus.sort_by(|a, b| {
        let (ka, kb) = (pci_order(a.0), pci_order(b.0));
        // Unparsable bus ids sort last, by text.
        (ka.is_none(), ka, a.0).cmp(&(kb.is_none(), kb, b.0))
    });
    gpus.into_iter().map(|(_, id, g)| (id, g)).collect()
}

/// The two-line header numbers (PROC-038) from the latest system snapshot and sweep.
///
/// Every total carries its own validity. A device that does not provide a metric is skipped;
/// otherwise the worst device validity wins, so the header shows "collecting" until every
/// device has a rate. Disk and network use present physical devices only: disks with no parent
/// (E04 already drops `loop`, `ram`, `zram` and `fd`) and wired, wireless and other
/// interfaces. O(devices).
pub fn totals(
    system: &SystemSnapshot,
    devices: &DeviceCatalog,
    sweep: &SweepTotals,
) -> TableTotals {
    let mem = &system.memory;
    let (memory_pct, swap_pct) = if !readable(mem.validity) {
        (state(mem.validity), state(mem.validity))
    } else {
        let pct = |used: u64, all: u64| -> Total<f32> {
            if all == 0 {
                return state(Validity::Unsupported);
            }
            Total {
                value: (used as f64 / all as f64 * 100.0) as f32,
                validity: mem.validity,
            }
        };
        (
            pct(mem.total.saturating_sub(mem.available), mem.total),
            pct(mem.swap_total.saturating_sub(mem.swap_free), mem.swap_total),
        )
    };

    let disks: SmallVec<[Metric<f32>; 8]> = system
        .disks
        .iter()
        .filter(|(idx, _)| {
            present(devices, *idx, DeviceKind::Disk).is_some_and(|e| e.parent.is_none())
        })
        .map(|(_, d)| d.active_pct)
        .collect();

    let mut net_validity: SmallVec<[Validity; 8]> = SmallVec::new();
    let mut net_sum = 0.0;
    let mut link_mbps = Some(0u64);
    for (idx, n) in system.nets.iter() {
        if !present(devices, *idx, DeviceKind::Net).is_some_and(physical_net) {
            continue;
        }
        for m in [n.rx_bps, n.tx_bps] {
            net_validity.push(m.validity);
            if readable(m.validity) {
                net_sum += m.value;
            }
        }
        link_mbps = link_mbps.and_then(|sum| {
            readable(n.link_speed_mbps.validity).then(|| sum + u64::from(n.link_speed_mbps.value))
        });
    }
    let (net_bps, net_link_bps) = if net_validity.is_empty() {
        (state(Validity::Missing), None)
    } else {
        let validity = combine(net_validity.iter().copied());
        let net_bps = if readable(validity) {
            Total {
                value: net_sum,
                validity,
            }
        } else {
            state(validity)
        };
        (net_bps, link_mbps.map(|mbps| mbps * BYTES_PER_MBIT))
    };

    let gpus = gpu_columns(system, devices);
    let utils: SmallVec<[Metric<f32>; 4]> = gpus.iter().map(|(_, g)| g.util_pct).collect();
    let gpu_pct = max_total(&utils, Validity::Unsupported);
    let mut gpu_adapter_pct = [state(Validity::Unsupported); 16];
    for (slot, m) in gpu_adapter_pct.iter_mut().zip(&utils) {
        *slot = total(*m);
    }
    let vram_validity = combine(
        gpus.iter()
            .flat_map(|(_, g)| [g.mem_used.validity, g.mem_total.validity]),
    );
    let (vram_used, vram_all) = gpus
        .iter()
        .filter(|(_, g)| readable(g.mem_used.validity) && readable(g.mem_total.validity))
        .fold((0u64, 0u64), |(u, t), (_, g)| {
            (u + g.mem_used.value, t + g.mem_total.value)
        });
    let vram_pct = if !readable(vram_validity) {
        state(vram_validity)
    } else if vram_all == 0 {
        state(Validity::Unsupported)
    } else {
        Total {
            value: (vram_used as f64 / vram_all as f64 * 100.0) as f32,
            validity: vram_validity,
        }
    };

    TableTotals {
        cpu_pct: total(system.cpu.util_pct),
        memory_pct,
        swap_pct,
        disk_pct: max_total(&disks, Validity::Missing),
        net_bps,
        net_link_bps,
        gpu_pct,
        gpu_adapter_pct,
        vram_pct,
        // One source per mode: the CPU worker's /proc/loadavg count whenever it is readable, so
        // the header matches Performance and Information. The sweep's sum is the fallback for
        // Low Memory mode, which runs no CPU worker, and before the first CPU slow tick.
        threads: if readable(system.cpu.threads.validity) {
            Total {
                value: u64::from(system.cpu.threads.value),
                validity: system.cpu.threads.validity,
            }
        } else {
            Total {
                value: u64::from(sweep.system_threads),
                validity: Validity::Valid,
            }
        },
        handles: sweep
            .file_nr
            .map_or(state(Validity::Unsupported), |n| Total {
                value: n,
                validity: Validity::Valid,
            }),
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
                desktop_path: None,
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
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
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
            unit_users: &unit_names,
            pseudo: None,
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

    /// PROC-047: the order compares the step the cell shows, then `ProcessKey`.
    #[test]
    fn cpu_sort_on_shown_step() {
        let cpu_desc = SortSpec {
            column: ColumnId::Cpu,
            ascending: false,
        };
        let order = |a: f32, b: f32| -> Vec<u32> {
            let records = vec![rec(1, 0, "a", a), rec(2, 0, "b", b)];
            tree(&records, "", false, &[], cpu_desc)
                .iter()
                .map(|r| r.0)
                .collect()
        };
        // Both cells read "5.0%": PID order, whichever is higher underneath.
        assert_eq!(order(5.01, 5.04), [1, 2]);
        assert_eq!(order(5.04, 5.01), [1, 2]);
        // "5.0%" against "5.1%": the shown step reorders.
        assert_eq!(order(5.01, 5.11), [2, 1]);
        // A measured zero and a value that prints "0%" tie.
        assert_eq!(order(0.0, 0.04), [1, 2]);
    }

    #[test]
    fn memory_and_rate_keys_on_shown_step() {
        const KIB_U: i128 = 1024;
        const MIB_U: i128 = 1024 * 1024;
        const GIB_U: i128 = 1024 * 1024 * 1024;
        // Below 1 KiB the bytes show exactly.
        assert_eq!(iec_shown(512), 512);
        assert_ne!(iec_shown(1022), iec_shown(1023));
        // "1.00 GiB" for both.
        assert_eq!(
            iec_shown(GIB_U + GIB_U / 1000),
            iec_shown(GIB_U + GIB_U / 400)
        );
        // "1.00 GiB" against "1.01 GiB".
        assert!(iec_shown(GIB_U) < iec_shown(GIB_U + GIB_U / 100));
        // The order holds across a unit change: "1023.99 KiB" < "1.00 MiB".
        assert!(iec_shown(1023 * KIB_U + 1013) < iec_shown(MIB_U));
        assert!(iec_shown(1023) < iec_shown(KIB_U));

        let mut settings = EngineSettings {
            network: NetworkUnit::Bits,
            ..EngineSettings::default()
        };
        // 0.1 Mbps is 12 500 bytes per second.
        assert_eq!(
            net_shown(13_000.0, &settings),
            net_shown(12_000.0, &settings)
        );
        assert!(net_shown(12_000.0, &settings) < net_shown(19_000.0, &settings));
        settings.network = NetworkUnit::Bytes;
        assert_eq!(net_shown(0.0, &settings), 0);
        assert_eq!(
            net_shown(1.04 * MIB, &settings),
            net_shown(1.0 * MIB, &settings)
        );

        // Auto scale: "0.4 Kbps" and "0.5 Kbps" differ though both read "0 Mbps" fixed.
        settings.network = NetworkUnit::Bits;
        settings.network_auto_scale = true;
        assert!(net_shown(50.0, &settings) < net_shown(62.0, &settings));
        // Both "1.0 Mbps"; and "992.0 Kbps" sorts below "1.0 Mbps".
        assert_eq!(
            net_shown(125_000.0, &settings),
            net_shown(128_000.0, &settings)
        );
        assert!(net_shown(124_000.0, &settings) < net_shown(125_000.0, &settings));
        settings.network = NetworkUnit::Bytes;
        // "0.5 KiB/s" below "1.0 KiB/s"; "1023.9 KiB/s" below "1.0 MiB/s".
        assert!(net_shown(512.0, &settings) < net_shown(1024.0, &settings));
        assert!(net_shown(1023.9 * KIB, &settings) < net_shown(MIB, &settings));
        settings.network_auto_scale = false;

        // Disk rates step by 0.1 MiB/s, then the key falls back to ProcessKey.
        let disk = |v: f32| shown_key(SortKey::Float(v), ColumnId::DiskReadRate, &settings);
        assert_eq!(disk(1_000.0), disk(2_000.0));
        assert_ne!(disk(1_000.0), disk(200_000.0));
        // Columns without a shown step keep their raw key.
        assert_eq!(
            shown_key(SortKey::Int(7), ColumnId::Pid, &settings),
            SortKey::Int(7)
        );
    }

    // Grouped mode (E09-011).

    const GIB: u64 = 1 << 30;

    const CPU_DESC: SortSpec = SortSpec {
        column: ColumnId::Cpu,
        ascending: false,
    };

    /// A record of app `app_id`. A primary carries the app id and the app name.
    fn in_app(mut r: ProcessRecord, app_id: &str, primary: bool) -> ProcessRecord {
        r.group = GroupKind::App {
            app_id: app_id.into(),
        }
        .key();
        if primary {
            Arc::make_mut(&mut r.meta).app_id = Some(app_id.into());
        }
        r
    }

    fn background(mut r: ProcessRecord) -> ProcessRecord {
        r.group = GroupKind::Background.key();
        r
    }

    fn system(mut r: ProcessRecord) -> ProcessRecord {
        r.group = GroupKind::System.key();
        r
    }

    fn kthread(pid: u32) -> ProcessRecord {
        let mut r = system(rec(pid, 2, &format!("kworker/{pid}"), 0.0));
        r.flags |= ProcFlags::KERNEL_THREAD;
        r
    }

    fn user_unit(name: &str) -> UnitRef {
        UnitRef {
            manager: Manager::User,
            name: name.into(),
        }
    }

    /// The input of one grouped build: records with their unit, in any order, and the app
    /// cgroup samples.
    struct Fixture {
        records: Vec<ProcessRecord>,
        extras: Vec<ProcessExtras>,
        app_stats: Vec<AppCgroupSample>,
        /// Grouped unless a test sets it.
        mode: TableMode,
        hide_system: bool,
        include_children: bool,
    }

    impl Fixture {
        fn new(entries: Vec<(ProcessRecord, Option<UnitRef>)>) -> Self {
            let mut entries = entries;
            entries.sort_by_key(|(r, _)| r.key);
            let (records, units): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
            let extras = units
                .into_iter()
                .map(|unit| ProcessExtras { unit, ..extras() })
                .collect();
            Fixture {
                records,
                extras,
                app_stats: Vec::new(),
                mode: TableMode::Grouped,
                hide_system: false,
                include_children: false,
            }
        }

        fn build(&self, filter: &str, expanded: &[NodeId], sort: SortSpec) -> ProcessTableView {
            let system = SystemSnapshot::default();
            let devices = DeviceCatalog::default();
            let settings = EngineSettings {
                hide_system: self.hide_system,
                ..EngineSettings::default()
            };
            let unit_names = FxHashMap::default();
            let input = TableInput {
                records: &self.records,
                extras: &self.extras,
                app_stats: &self.app_stats,
                unit_stats: &[],
                system: &system,
                devices: &devices,
                settings: &settings,
                unit_names: &unit_names,
                unit_users: &unit_names,
                pseudo: None,
            };
            let query = TableQuery {
                mode: self.mode,
                sort,
                filter: Arc::from(filter),
                expanded: Arc::from(expanded.to_vec()),
                include_children: self.include_children,
                ..TableQuery::default()
            };
            build(&input, &query)
        }

        fn node(&self, pid: u32) -> NodeId {
            let r = self.records.iter().find(|r| r.key.pid == pid).unwrap();
            NodeId::Process(r.key)
        }

        /// Every header, group and process node: the build with this list shows every row.
        fn all_nodes(&self) -> Vec<NodeId> {
            let mut nodes: Vec<NodeId> =
                GROUP_ORDER.iter().map(|c| NodeId::Group(c.key())).collect();
            nodes.extend(self.records.iter().map(|r| NodeId::Process(r.key)));
            nodes.extend(self.records.iter().map(|r| NodeId::Group(r.group)));
            nodes.extend(
                self.extras
                    .iter()
                    .filter_map(|x| x.unit.clone())
                    .map(NodeId::Unit),
            );
            nodes.push(NodeId::KernelThreads);
            nodes
        }
    }

    /// (label, depth) per row: "Header (N)" for headers, "Name (N)" for group rows, the PID for
    /// process rows.
    fn labels(view: &ProcessTableView, records: &[ProcessRecord]) -> Vec<(String, u8)> {
        view.rows
            .iter()
            .map(|row| {
                let label = match (row.record, row.group) {
                    (Some(i), _) => records[i as usize].key.pid.to_string(),
                    (None, Some(g)) => {
                        let g = &view.groups[g as usize];
                        format!("{} ({})", g.name, g.members)
                    }
                    (None, None) => {
                        let class = GROUP_ORDER
                            .iter()
                            .find(|c| row.node == NodeId::Group(c.key()))
                            .unwrap();
                        format!("{} ({})", class.name(), row.descendants)
                    }
                };
                (label, row.depth)
            })
            .collect()
    }

    fn headers() -> Vec<NodeId> {
        GROUP_ORDER.iter().map(|c| NodeId::Group(c.key())).collect()
    }

    fn header_counts(view: &ProcessTableView) -> Vec<u32> {
        view.rows
            .iter()
            .filter(|row| row.record.is_none() && row.group.is_none())
            .map(|row| row.descendants)
            .collect()
    }

    /// 2 app processes, 3 background, 4 system, the system ones busiest.
    fn mixed() -> Fixture {
        Fixture::new(vec![
            (in_app(rec(100, 1, "firefox", 1.0), "firefox", true), None),
            (
                in_app(rec(101, 100, "firefox", 2.0), "firefox", false),
                None,
            ),
            (background(rec(200, 1, "bash", 30.0)), None),
            (background(rec(201, 1, "zsh", 20.0)), None),
            (background(rec(202, 1, "fish", 10.0)), None),
            (system(rec(300, 1, "sshd", 90.0)), None),
            (system(rec(301, 1, "cron", 80.0)), None),
            (system(rec(302, 1, "dbus", 70.0)), None),
            (system(rec(303, 1, "udevd", 60.0)), None),
        ])
    }

    #[test]
    fn group_order_fixed() {
        let fx = mixed();
        for sort in [CPU_DESC, PID_ASC] {
            let view = fx.build("", &[], sort);
            assert_eq!(
                labels(&view, &fx.records),
                [
                    ("Apps (2)".to_string(), 0),
                    ("Background processes (3)".to_string(), 0),
                    ("System processes (4)".to_string(), 0),
                ]
            );
        }
        let view = fx.build("", &headers(), CPU_DESC);
        let names: Vec<String> = labels(&view, &fx.records)
            .into_iter()
            .map(|l| l.0)
            .collect();
        assert_eq!(
            names,
            [
                "Apps (2)",
                "firefox (2)",
                "Background processes (3)",
                "200",
                "201",
                "202",
                "System processes (4)",
                "300",
                "301",
                "302",
                "303",
            ]
        );
        assert_eq!(view.shown, 9);
    }

    fn app_pc() -> Fixture {
        let mut p = in_app(rec(100, 1, "Firefox", 10.0), "firefox", true);
        p.rss_anon_bytes = GIB;
        let mut c = in_app(rec(101, 100, "Web Content", 5.0), "firefox", false);
        c.rss_anon_bytes = GIB / 2;
        Fixture::new(vec![(p, None), (c, None)])
    }

    fn app_key() -> GroupKey {
        GroupKind::App {
            app_id: "firefox".into(),
        }
        .key()
    }

    #[test]
    fn app_row_aggregates_once() {
        let fx = app_pc();
        let mut expanded = headers();
        expanded.push(NodeId::Group(app_key()));
        expanded.push(fx.node(100));
        let view = fx.build("", &expanded, CPU_DESC);

        assert_eq!(view.groups.len(), 1);
        let g = &view.groups[0];
        assert_eq!(g.cpu_pct, 15.0);
        assert_eq!(g.members, 2);
        assert_eq!(g.kind, GroupRowKind::App);
        assert_eq!(g.node, NodeId::Group(app_key()));
        assert_eq!(&*g.name, "Firefox");
        assert_eq!(
            labels(&view, &fx.records),
            [
                ("Apps (2)".to_string(), 0),
                ("Firefox (2)".to_string(), 1),
                ("100".to_string(), 2),
                ("101".to_string(), 3),
                ("Background processes (0)".to_string(), 0),
                ("System processes (0)".to_string(), 0),
            ]
        );
        assert!(!view.rows[4].has_children);
    }

    #[test]
    fn app_memory_from_cgroup_anon() {
        let mut fx = app_pc();
        let view = fx.build("", &[], PID_ASC);
        assert_eq!(view.groups[0].memory_bytes, GIB + GIB / 2);

        fx.app_stats.push(AppCgroupSample {
            group: app_key(),
            anon_bytes: GIB,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            rios: 0,
            wios: 0,
            pids: 2,
            cgroup: tl_model::actions::CgroupRef {
                rel_path: "user.slice/app.slice/app-firefox.scope".into(),
                cgroup_id: 1,
            },
            validity: FieldValidity::all(Validity::Valid),
        });
        let view = fx.build("", &[], PID_ASC);
        assert_eq!(view.groups[0].memory_bytes, GIB);
        assert_eq!(view.groups[0].rss_anon_bytes, GIB);
    }

    #[test]
    fn unit_rows_fold_multi_process_units() {
        let pw = user_unit("pipewire.service");
        let fx = Fixture::new(vec![
            (background(rec(10, 1, "pipewire", 1.0)), Some(pw.clone())),
            (background(rec(11, 1, "wireplumber", 1.0)), Some(pw.clone())),
            (
                background(rec(12, 1, "pipewire-pulse", 1.0)),
                Some(pw.clone()),
            ),
            (
                background(rec(20, 1, "kded6", 1.0)),
                Some(user_unit("plasma-kded6.service")),
            ),
            (background(rec(30, 1, "bash", 1.0)), None),
        ]);
        let mut expanded = headers();
        expanded.push(NodeId::Unit(pw.clone()));
        let view = fx.build("", &expanded, PID_ASC);

        let unit_rows: Vec<&GroupRow> = view
            .groups
            .iter()
            .filter(|g| g.kind == GroupRowKind::Unit)
            .collect();
        assert_eq!(unit_rows.len(), 1);
        assert_eq!(unit_rows[0].node, NodeId::Unit(pw));
        assert_eq!(unit_rows[0].class, GroupKind::Background);
        assert_eq!(
            labels(&view, &fx.records),
            [
                ("Apps (0)".to_string(), 0),
                ("Background processes (5)".to_string(), 0),
                // A unit row has no PID, so the PID sort puts it after the processes.
                ("20".to_string(), 1),
                ("30".to_string(), 1),
                ("pipewire.service (3)".to_string(), 1),
                ("10".to_string(), 2),
                ("11".to_string(), 2),
                ("12".to_string(), 2),
                ("System processes (0)".to_string(), 0),
            ]
        );
    }

    #[test]
    fn kernel_threads_row_first_in_system() {
        let mut entries: Vec<(ProcessRecord, Option<UnitRef>)> =
            (1000..1050).map(|pid| (kthread(pid), None)).collect();
        entries.push((system(rec(1, 0, "systemd", 0.0)), None));
        entries.push((system(rec(5000, 1, "sshd", 0.0)), None));
        let fx = Fixture::new(entries);

        let view = fx.build("", &headers(), PID_ASC);
        let rows = labels(&view, &fx.records);
        assert_eq!(
            rows[2..],
            [
                ("System processes (52)".to_string(), 0),
                ("Kernel threads (50)".to_string(), 1),
                ("1".to_string(), 1),
                ("5000".to_string(), 1),
            ]
        );
        let kernel = &view.rows[3];
        assert_eq!(kernel.node, NodeId::KernelThreads);
        assert!(!kernel.expanded);
        assert!(kernel.has_children);
        assert_eq!(kernel.descendants, 50);

        let mut expanded = headers();
        expanded.push(NodeId::KernelThreads);
        let view = fx.build("", &expanded, PID_ASC);
        let rows = labels(&view, &fx.records);
        assert_eq!(rows.len(), 6 + 50);
        assert_eq!(rows[3], ("Kernel threads (50)".to_string(), 1));
        assert_eq!(rows[4], ("1000".to_string(), 2));
        assert_eq!(rows[53], ("1049".to_string(), 2));
        assert_eq!(rows[54..], [("1".to_string(), 1), ("5000".to_string(), 1)]);
    }

    #[test]
    fn group_name_filter_matches_members() {
        let fx = app_pc();
        let mut entries = vec![
            (background(rec(200, 1, "bash", 0.0)), None),
            (system(rec(300, 1, "systemd", 0.0)), None),
        ];
        entries.extend(fx.records.into_iter().map(|r| (r, None)));
        let fx = Fixture::new(entries);

        let view = fx.build("apps", &[], PID_ASC);
        assert_eq!(header_counts(&view), [2, 0, 0]);
        assert_eq!(view.shown, 2);

        // The app name matches the child named "Web Content" too.
        let view = fx.build("firefox", &[], PID_ASC);
        assert_eq!(header_counts(&view), [2, 0, 0]);

        let view = fx.build("system processes", &[], PID_ASC);
        assert_eq!(header_counts(&view), [0, 0, 1]);

        // Headers stay with count 0 when nothing matches.
        let view = fx.build("nomatch", &headers(), PID_ASC);
        assert_eq!(header_counts(&view), [0, 0, 0]);
        assert_eq!(view.rows.len(), 3);
        assert!(view.rows.iter().all(|r| !r.has_children));
    }

    #[test]
    fn counts_sum_to_visible_records() {
        let pw = user_unit("pipewire.service");
        let mut entries: Vec<(ProcessRecord, Option<UnitRef>)> = vec![
            (in_app(rec(100, 1, "firefox", 1.0), "firefox", true), None),
            (
                in_app(rec(101, 100, "firefox", 2.0), "firefox", false),
                None,
            ),
            (
                in_app(rec(102, 101, "firefox", 2.0), "firefox", false),
                None,
            ),
            (in_app(rec(110, 1, "konsole", 1.0), "konsole", true), None),
            // A child of an app process in another app: it starts its own subtree.
            (in_app(rec(111, 110, "kate", 1.0), "kate", true), None),
            (background(rec(10, 1, "pipewire", 1.0)), Some(pw.clone())),
            (
                background(rec(11, 10, "pipewire-pulse", 1.0)),
                Some(pw.clone()),
            ),
            (background(rec(30, 1, "bash", 1.0)), None),
            (
                system(rec(40, 1, "sshd", 1.0)),
                Some(UnitRef {
                    manager: Manager::System,
                    name: "sshd.service".into(),
                }),
            ),
            (system(rec(1, 0, "systemd", 1.0)), None),
        ];
        entries.extend((1000..1010).map(|pid| (kthread(pid), None)));
        let fx = Fixture::new(entries);
        let all = fx.all_nodes();

        for filter in ["", "fire", "pipe", "system processes", "kworker", "1"] {
            let view = fx.build(filter, &all, CPU_DESC);
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
        let view = fx.build("", &all, CPU_DESC);
        assert_eq!(view.shown as usize, fx.records.len());
    }

    fn in_cgroup(mut r: ProcessRecord, cgroup: &str) -> ProcessRecord {
        Arc::make_mut(&mut r.meta).cgroup = Some(cgroup.into());
        r
    }

    const USER: &str = "user.slice/user-1000.slice/user@1000.service";

    /// pipewire and plasmashell in session.slice, firefox in app.slice, cron as a system unit,
    /// the user manager, PID 1 and two kernel threads.
    fn desktop() -> Fixture {
        let bg = |pid: u32, ppid: u32, name: &str, path: &str| {
            let r = background(rec(pid, ppid, name, 0.0));
            (in_cgroup(r, &format!("{USER}/{path}")), None)
        };
        Fixture::new(vec![
            (
                in_cgroup(system(rec(1, 0, "systemd", 0.0)), "init.scope"),
                None,
            ),
            bg(200, 1, "systemd", "init.scope"),
            bg(201, 204, "pipewire", "session.slice/pipewire.service"),
            bg(
                202,
                200,
                "plasmashell",
                "session.slice/plasma-plasmashell.service",
            ),
            bg(203, 200, "kded6", "background.slice/plasma-kded6.service"),
            bg(204, 200, "bash", "app.slice/app-konsole@1.service"),
            (
                in_cgroup(
                    in_app(rec(100, 200, "firefox", 0.0), "firefox", true),
                    &format!("{USER}/app.slice/app-firefox@1.service"),
                ),
                None,
            ),
            (
                in_cgroup(
                    system(rec(300, 1, "cron", 0.0)),
                    "system.slice/cron.service",
                ),
                None,
            ),
            (kthread(1000), None),
            (kthread(1001), None),
        ])
    }

    fn pids(view: &ProcessTableView, records: &[ProcessRecord]) -> Vec<u32> {
        let mut v: Vec<u32> = view
            .rows
            .iter()
            .filter_map(|r| r.record)
            .map(|i| records[i as usize].key.pid)
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn hide_system_flat_keeps_apps_and_exceptions() {
        let mut fx = desktop();
        fx.mode = TableMode::Flat;
        fx.hide_system = true;
        let view = fx.build("", &[], PID_ASC);
        assert_eq!(pids(&view, &fx.records), [100, 202, 204]);
        assert_eq!(view.shown, 3);
    }

    #[test]
    fn hide_system_total_and_shown() {
        // desktop() has 10 records; hide_system leaves 100, 202 and 204. "bash" matches 204.
        for mode in [TableMode::Flat, TableMode::Tree, TableMode::Grouped] {
            let mut fx = desktop();
            fx.mode = mode;
            let all = fx.all_nodes();
            for (hide, filter, total, shown) in [
                (false, "", 10, 10),
                (false, "bash", 10, 1),
                (true, "", 3, 3),
                (true, "bash", 3, 1),
            ] {
                fx.hide_system = hide;
                let view = fx.build(filter, &all, PID_ASC);
                let case = format!("{mode:?} hide {hide} filter {filter:?}");
                assert_eq!(view.total, total, "{case}");
                assert_eq!(view.shown, shown, "{case}");
            }
        }
    }

    #[test]
    fn hide_system_off_is_noop() {
        for mode in [TableMode::Flat, TableMode::Tree, TableMode::Grouped] {
            let mut fx = desktop();
            fx.mode = mode;
            let all = fx.all_nodes();
            let view = fx.build("", &all, PID_ASC);
            assert_eq!(view.shown as usize, fx.records.len(), "{mode:?}");
        }
    }

    #[test]
    fn hide_system_grouped_drops_system_header() {
        let mut fx = desktop();
        fx.hide_system = true;
        let all = fx.all_nodes();
        let view = fx.build("", &all, PID_ASC);
        assert!(
            !view
                .rows
                .iter()
                .any(|r| r.node == NodeId::Group(GroupClass::System.key())
                    || r.node == NodeId::KernelThreads)
        );
        assert_eq!(header_counts(&view), [1, 2]);
        assert_eq!(pids(&view, &fx.records), [100, 202, 204]);
        assert_eq!(view.shown, 3);
    }

    #[test]
    fn hide_system_tree_hidden_parent_makes_child_root() {
        let mut fx = desktop();
        fx.mode = TableMode::Tree;
        fx.hide_system = true;
        let all = fx.all_nodes();
        // firefox's parent is the hidden user manager (PID 200).
        let view = fx.build("", &all, PID_ASC);
        assert_eq!(pids(&view, &fx.records), [100, 202, 204]);
        assert!(view.rows.iter().all(|r| r.depth == 0));
        // pipewire (hidden) is bash's child: include children must not bring it back.
        fx.include_children = true;
        let view = fx.build("bash", &all, PID_ASC);
        assert_eq!(pids(&view, &fx.records), [204]);
        assert!(!view.rows[0].has_children);
    }

    #[test]
    fn hide_system_rules() {
        let bg = |path: &str| in_cgroup(background(rec(5, 1, "x", 0.0)), path);
        let hidden = |path: &str| hidden_by_hide_system(&bg(path), GroupClass::Background);
        assert!(hidden(&format!("{USER}/session.slice/pipewire.service")));
        assert!(hidden(&format!(
            "{USER}/background.slice/plasma-kded6.service"
        )));
        assert!(hidden(&format!("{USER}/init.scope")));
        assert!(hidden(
            "user.slice/user-42.slice/user@42.service/init.scope"
        ));
        for unit in HIDE_SYSTEM_EXCEPTIONS {
            assert!(!hidden(&format!("{USER}/session.slice/{unit}")), "{unit}");
        }
        assert!(!hidden(&format!("{USER}/app.slice/app-firefox@1.service")));
        assert!(!hidden(&format!("{USER}/app.slice/init.scope")));
        assert!(!hidden(
            "user.slice/user-1000.slice/user@x.service/init.scope"
        ));
        assert!(!hidden("user.slice/user-1000.slice/session-2.scope"));
        assert!(!hidden_by_hide_system(
            &background(rec(5, 1, "x", 0.0)),
            GroupClass::Background
        ));
        let pid1 = in_cgroup(system(rec(1, 0, "systemd", 0.0)), "init.scope");
        assert_eq!(group_class(&pid1), GroupClass::System);
        assert!(hidden_by_hide_system(&pid1, GroupClass::System));
        assert!(!hidden_by_hide_system(&bg("x"), GroupClass::Apps));
    }

    mod totals {
        use super::super::*;

        use tl_model::system::{DiskTotals, LinkState, MemoryTotals, NetTotals};

        const GIB: u64 = 1 << 30;

        fn entry(idx: u16, identity: DeviceIdentity, parent: Option<u16>) -> DeviceEntry {
            let net_kind = matches!(identity, DeviceIdentity::Net { .. }).then_some(NetKind::Wired);
            DeviceEntry {
                idx: DeviceIdx(idx),
                identity,
                generation: 0,
                display_name: Arc::from("device"),
                present: true,
                parent: parent.map(DeviceIdx),
                persistent_key: Arc::from("key"),
                net_kind,
            }
        }

        fn catalog(entries: Vec<DeviceEntry>) -> DeviceCatalog {
            DeviceCatalog {
                version: 1,
                entries: entries.into(),
            }
        }

        fn disk_id(name: &str) -> DeviceIdentity {
            DeviceIdentity::Disk {
                major: 8,
                minor: 0,
                name: Arc::from(name),
                wwn_or_serial: None,
            }
        }

        fn disk(idx: u16, busy: f32) -> (DeviceIdx, DiskTotals) {
            let d = DiskTotals {
                device: DeviceIdx(idx),
                active_pct: Metric::valid(busy),
                read_bps: Metric::valid(0.0),
                write_bps: Metric::valid(0.0),
                response_ms: Metric::valid(0.0),
                read_total: 0,
                written_total: 0,
                capacity: 0,
                temp_c: Metric::with(Validity::Unsupported),
            };
            (DeviceIdx(idx), d)
        }

        fn net(idx: u16, rx: f64, tx: f64, speed: Metric<u32>) -> (DeviceIdx, NetTotals) {
            let n = NetTotals {
                device: DeviceIdx(idx),
                rx_bps: Metric::valid(rx),
                tx_bps: Metric::valid(tx),
                rx_total: 0,
                tx_total: 0,
                link_speed_mbps: speed,
                state: LinkState::Up,
            };
            (DeviceIdx(idx), n)
        }

        fn net_id(ifindex: u32) -> DeviceIdentity {
            DeviceIdentity::Net {
                netns_ino: 1,
                ifindex,
                name: Arc::from("eno1"),
            }
        }

        fn gpu(idx: u16, util: f32, used: u64, all: u64) -> (DeviceIdx, GpuTotals) {
            let g = GpuTotals {
                device: DeviceIdx(idx),
                util_pct: Metric::valid(util),
                mem_util_pct: Metric::valid(0.0),
                mem_used: Metric::valid(used),
                mem_total: Metric::valid(all),
                shared_used: Metric::with(Validity::Unsupported),
                shared_total: Metric::with(Validity::Unsupported),
                enc_pct: Metric::with(Validity::Unsupported),
                dec_pct: Metric::with(Validity::Unsupported),
                mem_reserved: Metric::with(Validity::Unsupported),
                enc_sessions: Metric::with(Validity::Unsupported),
                enc_dec_shared: false,
                temp_c: Metric::with(Validity::Unsupported),
                temp_edge_c: Metric::with(Validity::Unsupported),
                temp_junction_c: Metric::with(Validity::Unsupported),
                temp_mem_c: Metric::with(Validity::Unsupported),
                power_w: Metric::with(Validity::Unsupported),
                power_limit_w: Metric::with(Validity::Unsupported),
                clock_gfx_mhz: Metric::with(Validity::Unsupported),
                clock_mem_mhz: Metric::with(Validity::Unsupported),
                fan_pct: Metric::with(Validity::Unsupported),
                pcie_rx_bps: Metric::with(Validity::Unsupported),
                pcie_tx_bps: Metric::with(Validity::Unsupported),
            };
            (DeviceIdx(idx), g)
        }

        fn gpu_id(bus: &str) -> DeviceIdentity {
            DeviceIdentity::Gpu {
                uuid: [0; 16],
                pci_bus_id: Arc::from(bus),
            }
        }

        fn memory(total: u64, available: u64, swap_total: u64, swap_free: u64) -> MemoryTotals {
            MemoryTotals {
                validity: Validity::Valid,
                total,
                available,
                swap_total,
                swap_free,
                ..Default::default()
            }
        }

        const SWEEP: SweepTotals = SweepTotals {
            system_threads: 1234,
            file_nr: Some(5678),
        };

        #[test]
        fn totals_memory_and_swap() {
            let mut sys = SystemSnapshot {
                memory: memory(32 * GIB, 8 * GIB, 0, 0),
                ..Default::default()
            };
            let t = totals(&sys, &DeviceCatalog::default(), &SWEEP);
            assert_eq!(t.memory_pct.value, 75.0);
            assert_eq!(t.memory_pct.validity, Validity::Valid);
            assert_eq!(t.swap_pct.validity, Validity::Unsupported);

            sys.memory = memory(32 * GIB, 8 * GIB, 8 * GIB, 6 * GIB);
            let t = totals(&sys, &DeviceCatalog::default(), &SWEEP);
            assert_eq!(
                t.swap_pct,
                Total {
                    value: 25.0,
                    validity: Validity::Valid
                }
            );

            // Before the first meminfo read, swap is still collecting, not unsupported.
            sys.memory = MemoryTotals::default();
            let t = totals(&sys, &DeviceCatalog::default(), &SWEEP);
            assert_eq!(t.memory_pct.validity, Validity::WarmingUp);
            assert_eq!(t.swap_pct.validity, Validity::WarmingUp);
        }

        #[test]
        fn totals_disk_max_physical_only() {
            let devices = catalog(vec![
                entry(0, disk_id("sda"), None),
                entry(1, disk_id("nvme0n1"), None),
                entry(2, disk_id("sda1"), Some(0)),
            ]);
            let sys = SystemSnapshot {
                disks: vec![disk(0, 40.0), disk(1, 70.0), disk(2, 90.0)].into(),
                ..Default::default()
            };
            let t = totals(&sys, &devices, &SWEEP);
            assert_eq!(
                t.disk_pct,
                Total {
                    value: 70.0,
                    validity: Validity::Valid
                }
            );

            let none = totals(&SystemSnapshot::default(), &devices, &SWEEP);
            assert_eq!(none.disk_pct.validity, Validity::Missing);

            // A disk without a rate yet keeps the total collecting.
            let mut warming = disk(0, 0.0);
            warming.1.active_pct = Metric::with(Validity::WarmingUp);
            let sys = SystemSnapshot {
                disks: vec![warming, disk(1, 70.0)].into(),
                ..Default::default()
            };
            assert_eq!(
                totals(&sys, &devices, &SWEEP).disk_pct.validity,
                Validity::WarmingUp
            );
        }

        #[test]
        fn totals_net_pct_or_rate() {
            let mut vpn = entry(2, net_id(3), None);
            vpn.net_kind = Some(NetKind::VpnTunnel);
            let mut veth = entry(3, net_id(4), None);
            veth.net_kind = Some(NetKind::Virtual);
            let devices = catalog(vec![
                entry(0, net_id(1), None),
                entry(1, net_id(2), None),
                vpn,
                veth,
            ]);
            // 350 Mbit/s = 43 750 000 bytes per second, split over two interfaces.
            let sys = SystemSnapshot {
                nets: vec![
                    net(0, 20_000_000.0, 5_000_000.0, Metric::valid(1000)),
                    net(1, 15_000_000.0, 3_750_000.0, Metric::valid(2500)),
                    net(
                        2,
                        9_000_000.0,
                        9_000_000.0,
                        Metric::with(Validity::Unsupported),
                    ),
                    net(3, 9_000_000.0, 9_000_000.0, Metric::valid(10_000)),
                ]
                .into(),
                ..Default::default()
            };
            let t = totals(&sys, &devices, &SWEEP);
            assert_eq!(
                t.net_bps,
                Total {
                    value: 43_750_000.0,
                    validity: Validity::Valid
                }
            );
            assert_eq!(t.net_link_bps, Some(437_500_000));

            let mut nets = sys.nets.to_vec();
            nets[1].1.link_speed_mbps = Metric::with(Validity::Unsupported);
            let sys = SystemSnapshot {
                nets: nets.into(),
                ..Default::default()
            };
            let t = totals(&sys, &devices, &SWEEP);
            assert_eq!(t.net_bps.value, 43_750_000.0);
            assert_eq!(t.net_link_bps, None);
        }

        #[test]
        fn totals_gpu_order_by_pci() {
            let devices = catalog(vec![
                entry(0, gpu_id("0000:2d:00.0"), None),
                entry(1, gpu_id("00000000:01:00.0"), None),
            ]);
            let sys = SystemSnapshot {
                gpus: vec![
                    gpu(0, 80.0, 6 * GIB, 8 * GIB),
                    gpu(1, 20.0, 2 * GIB, 8 * GIB),
                ]
                .into(),
                ..Default::default()
            };
            let t = totals(&sys, &devices, &SWEEP);
            assert_eq!(
                t.gpu_pct,
                Total {
                    value: 80.0,
                    validity: Validity::Valid
                }
            );
            assert_eq!(t.gpu_adapter_pct[0].value, 20.0);
            assert_eq!(t.gpu_adapter_pct[1].value, 80.0);
            assert!(
                t.gpu_adapter_pct[2..]
                    .iter()
                    .all(|g| g.validity == Validity::Unsupported)
            );
            assert_eq!(
                t.vram_pct,
                Total {
                    value: 50.0,
                    validity: Validity::Valid
                }
            );

            let none = totals(&SystemSnapshot::default(), &devices, &SWEEP);
            assert_eq!(none.gpu_pct.validity, Validity::Unsupported);
            assert_eq!(none.vram_pct.validity, Validity::Unsupported);
        }

        #[test]
        fn totals_handles_unsupported_without_file_nr() {
            let sys = SystemSnapshot::default();
            let devices = DeviceCatalog::default();
            let t = totals(&sys, &devices, &SWEEP);
            assert_eq!(
                t.threads,
                Total {
                    value: 1234,
                    validity: Validity::Valid
                }
            );
            assert_eq!(
                t.handles,
                Total {
                    value: 5678,
                    validity: Validity::Valid
                }
            );
            let sweep = SweepTotals {
                file_nr: None,
                ..SWEEP
            };
            assert_eq!(
                totals(&sys, &devices, &sweep).handles.validity,
                Validity::Unsupported
            );
        }
    }
}
