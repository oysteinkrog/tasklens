//! Process records, metadata, flags and groups (ADR-003).
//!
//! Numeric fields are fixed-size and text lives in a shared `ProcessMeta` behind an `Arc`, so a
//! 10 Hz CPU publish copies no strings (ADR-012 constraint 3).
//!
//! Privacy: `ProcessMeta` may hold user data (cmdline, exe, cgroup). Never implement `Display`
//! for these types and never log `ProcessMeta` at any level (ADR-015). `Debug` is for tests only.

use std::hash::Hasher;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::actions::{CgroupRef, IoPriority, SchedPolicy};
use crate::ids::{GroupKey, NodeId, ProcessKey};
use crate::system::CpuSet;
use crate::table::{Manager, UnitRef};
use crate::time::TimeNs;
use crate::validity::FieldValidity;

/// Text of one process. Shared by every record of that process until a field changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessMeta {
    /// /proc/<pid>/comm, sanitized.
    pub comm: Arc<str>,
    /// Display name: desktop entry Name, else exe basename, else comm.
    pub name: Arc<str>,
    /// readlink /proc/<pid>/exe; `None` if restricted.
    pub exe: Option<Arc<str>>,
    /// Bounded to 32 KiB, NULs to spaces, sanitized (ADR-015).
    pub cmdline: Option<Arc<str>>,
    /// User name for the uid.
    pub user: Arc<str>,
    /// cgroup v2 path relative to /sys/fs/cgroup.
    pub cgroup: Option<Arc<str>>,
    /// From the app scope or desktop entry.
    pub app_id: Option<Arc<str>>,
    /// Resolved icon file path.
    pub icon: Option<Arc<str>>,
    /// Desktop entry Comment or GenericName.
    pub description: Option<Arc<str>>,
    /// Path of the matched desktop file. Shares the `Arc` of the collector's desktop entry.
    pub desktop_path: Option<Arc<str>>,
    /// Lowercase filter text: display name, image name, cmdline, user, description, app name,
    /// unit name (R-043).
    pub search_lower: Arc<str>,
}

/// Scheduler state from /proc/<pid>/stat field 3.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum ProcState {
    Running,
    Sleeping,
    DiskSleep,
    Stopped,
    Zombie,
    Idle,
    Other,
}

impl ProcState {
    /// From /proc/<pid>/stat field 3: b'R' Running, b'S' Sleeping, b'D' DiskSleep,
    /// b'T' and b't' Stopped, b'Z' Zombie, b'I' Idle, any other byte Other.
    pub const fn from_stat_char(c: u8) -> ProcState {
        match c {
            b'R' => ProcState::Running,
            b'S' => ProcState::Sleeping,
            b'D' => ProcState::DiskSleep,
            b'T' | b't' => ProcState::Stopped,
            b'Z' => ProcState::Zombie,
            b'I' => ProcState::Idle,
            _ => ProcState::Other,
        }
    }
}

bitflags::bitflags! {
    /// Per-process marks. Bit values are frozen.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
    pub struct ProcFlags: u16 {
        /// Appeared within the linger time (`linger_ms`).
        const NEW = 1;
        /// Exited within the linger time; kept for highlight, then removed.
        const EXITED = 2;
        const KERNEL_THREAD = 4;
        const OTHER_USER = 8;
        /// tasklens or its probe.
        const IS_SELF = 16;
        /// See ADR-006.
        const PROTECTED = 32;
        /// `meta.name` is the desktop entry Name of the app: the collector named this process as
        /// the app's primary process (PROC-008). An app row takes its name from such a member.
        const APP_NAME = 64;
    }
}

/// Linger time for NEW and EXITED: 2 × interval, clamped to 1_000..=3_000 ms.
pub const fn linger_ms(processes_interval_ms: u32) -> u32 {
    let twice = processes_interval_ms.saturating_mul(2);
    if twice < 1_000 {
        1_000
    } else if twice > 3_000 {
        3_000
    } else {
        twice
    }
}

/// One process in one sweep. Fixed-size apart from `meta`, which clones share.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub key: ProcessKey,
    pub ppid: u32,
    pub uid: u32,
    pub meta: Arc<ProcessMeta>,
    pub group: GroupKey,
    pub state: ProcState,
    pub flags: ProcFlags,
    pub nice: i8,
    pub threads: u32,
    /// Percent of whole machine capacity, 0..=100.
    pub cpu_pct: f32,
    /// RssAnon from /proc/<pid>/status; the default Memory column (D-004).
    pub rss_anon_bytes: u64,
    /// VmRSS; the "Working set total" column.
    pub rss_bytes: u64,
    /// Only for demanded keys (ADR-008).
    pub pss_bytes: u64,
    pub uss_bytes: u64,
    pub swap_bytes: u64,
    pub io_read_bps: f32,
    pub io_write_bps: f32,
    /// Attributed TCP only (ADR-016).
    pub net_rx_bps: f32,
    pub net_tx_bps: f32,
    pub gpu_util_pct: f32,
    pub gpu_mem_bytes: u64,
    /// The effective UID from the status `Uid:` line, `None` when it equals `uid` (PROC-028).
    pub euid: Option<u32>,
    /// At least one attributed socket is held by 2 or more processes (PROC-032).
    pub net_shared: bool,
    /// Cancelled write bytes per second from `/proc/<pid>/io` (PROC-031). Its validity is the
    /// `IoWrite` validity.
    pub cancelled_write_bps: f32,
    pub validity: FieldValidity,
}

/// Per-GPU values of one process. Inline up to 4 GPUs, so a record clone does not allocate.
pub type GpuVram = SmallVec<[u64; 4]>;

/// Group classification of a process (PROC-004). Exactly these four classes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GroupKind {
    /// cgroup app-*.scope or app-*.service under user@<uid>.service/app.slice.
    App { app_id: Arc<str> },
    /// Processes of the current uid not in an app scope.
    Background,
    /// Other uids, system.slice, init.scope, kernel threads.
    System,
    /// cgroup unreadable.
    Ungrouped,
}

impl GroupKind {
    /// FxHash (rustc_hash::FxHasher) of the value, as ADR-003 defines GroupKey (R-279): a
    /// discriminant byte (`App` 1, `Background` 2, `System` 3, `Ungrouped` 4), then the app id
    /// bytes for `App`. FxHasher has no random seed, so history (ADR-012) can store the value.
    pub fn key(&self) -> GroupKey {
        let mut hasher = rustc_hash::FxHasher::default();
        match self {
            GroupKind::App { app_id } => {
                hasher.write_u8(1);
                hasher.write(app_id.as_bytes());
            }
            GroupKind::Background => hasher.write_u8(2),
            GroupKind::System => hasher.write_u8(3),
            GroupKind::Ungrouped => hasher.write_u8(4),
        }
        GroupKey(hasher.finish())
    }
}

impl UnitRef {
    /// Row key of the unit's group row: FxHash of discriminant byte 5, the manager byte
    /// (`System` 1, `User` 2) and the unit name bytes. Byte 5 keeps it apart from every
    /// `GroupKind` key, so `ProcessRecord.group` classing is unchanged. Each unit gets its own key,
    /// so App Properties lists and focuses that unit only.
    pub fn key(&self) -> GroupKey {
        let mut hasher = rustc_hash::FxHasher::default();
        hasher.write_u8(5);
        hasher.write_u8(match self.manager {
            Manager::System => 1,
            Manager::User => 2,
        });
        hasher.write(self.name.as_bytes());
        GroupKey(hasher.finish())
    }
}

/// True when a record belongs to the group row with key `key`: an app row when
/// `record.group == key`, a unit row when the record's unit (`extras.unit`) has that key.
/// `extras` is the record's entry in `ProcessSnapshot.extras` (same index).
pub fn is_row_member(
    record: &ProcessRecord,
    extras: Option<&ProcessExtras>,
    key: GroupKey,
) -> bool {
    record.group == key
        || extras
            .and_then(|x| x.unit.as_ref())
            .is_some_and(|unit| unit.key() == key)
}

/// What a group row of the table is (R-141 item 3). GroupKind stays the four PROC-004 classes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum GroupRowKind {
    App,
    Unit,
    KernelThreads,
    Pseudo,
}

/// Aggregated values of one group row. Each member process is counted once (no double
/// counting of descendants, ADR-009). Field meanings match ProcessRecord.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupRow {
    /// Table identity of the row (R-141 item 3).
    pub node: NodeId,
    /// App row: the app's `GroupKind` key. Unit row: `UnitRef::key`. Kernel threads row: the
    /// `System` key.
    pub key: GroupKey,
    /// App, Unit, KernelThreads or Pseudo (R-141 item 3).
    pub kind: GroupRowKind,
    /// Classification the row sits under (App, Background, System, Ungrouped).
    pub class: GroupKind,
    /// Display name of the row.
    pub name: Arc<str>,
    /// Resolved icon file path.
    pub icon: Option<Arc<str>>,
    /// Member processes after filtering.
    pub members: u32,
    /// Processes in the cgroup (pids.current for App and Unit rows).
    pub pids: u32,
    /// Short row note, for example the pseudo-row explanation.
    pub note: Option<Arc<str>>,
    /// The value of the active Memory column for the row.
    pub memory_bytes: u64,
    /// io_read_bps + io_write_bps.
    pub disk_bps: f32,
    /// net_rx_bps + net_tx_bps.
    pub net_bps: f32,
    /// Same as gpu_util_pct.
    pub gpu_pct: f32,
    pub cpu_pct: f32,
    /// App rows: cgroup memory.stat `anon` (R-005).
    pub rss_anon_bytes: u64,
    pub rss_bytes: u64,
    pub pss_bytes: u64,
    pub uss_bytes: u64,
    pub swap_bytes: u64,
    pub io_read_bps: f32,
    pub io_write_bps: f32,
    pub net_rx_bps: f32,
    pub net_tx_bps: f32,
    pub gpu_util_pct: f32,
    pub gpu_mem_bytes: u64,
    pub threads: u32,
    /// Sums of the `ProcessExtras` columns over the members (PROC-029, PROC-030, PROC-035).
    /// Zero when the extras have not arrived; the UI shows WarmingUp then.
    pub cpu_kernel_pct: f32,
    pub cpu_user_pct: f32,
    pub cpu_relative_pct: f32,
    pub utime_ticks: u64,
    pub stime_ticks: u64,
    pub commit_bytes: u64,
    pub page_faults: u64,
    pub page_faults_delta: u64,
    pub major_faults: u64,
    /// Sums of the members' `ProcessExtras.counters` (PROC-030, PROC-031).
    pub counters: ProcCounters,
    /// App rows with a cgroup sample: `io.stat` values of the app's cgroup. `None` for other rows.
    pub cgroup_io: Option<CgroupIo>,
    /// `None` when any member's fd count was not read (Restricted).
    pub handles: Option<u32>,
    /// App rows: path of the matched desktop file, from the first member's `ProcessMeta`.
    pub desktop_path: Option<Arc<str>>,
    /// Unit rows: the unit's `Description=`, when systemd reported a non-empty one.
    pub description: Option<Arc<str>>,
    /// Unit rows: `UnitRecord.user` (owner of MainPID, else `User=`, SYS-003).
    pub user: Option<Arc<str>>,
    /// App and unit rows: the cgroup's `cgroup.events` has `frozen 1` (PROC-064). False when
    /// it was not read.
    pub frozen: bool,
    pub validity: FieldValidity,
}

impl GroupRow {
    /// A row with the given identity and every other field 0, `None` or `false`. For tests and
    /// for builders that fill the rest. There is no `Default`: a made-up identity would be a bug.
    pub fn empty(
        node: NodeId,
        key: GroupKey,
        kind: GroupRowKind,
        class: GroupKind,
        name: Arc<str>,
        validity: FieldValidity,
    ) -> GroupRow {
        GroupRow {
            node,
            key,
            kind,
            class,
            name,
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
            counters: ProcCounters::default(),
            cgroup_io: None,
            handles: None,
            desktop_path: None,
            description: None,
            user: None,
            frozen: false,
            validity,
        }
    }
}

/// Which expensive fields cover which keys (ADR-008). Keys not listed show WarmingUp for PSS and USS.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Coverage {
    /// Sorted ascending, so the UI can binary_search.
    pub pss_uss: Arc<[ProcessKey]>,
}

impl Coverage {
    pub fn has_pss_uss(&self, key: &ProcessKey) -> bool {
        self.pss_uss.binary_search(key).is_ok()
    }
}

/// P1 extra-column values, index aligned with `ProcessSnapshot.records` (same length, same
/// order) (R-141 item 1).
///
/// Privacy: `exe_path` is user data. Never log it above debug level (ADR-015).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessExtras {
    pub cpu_kernel_pct: f32,
    pub cpu_user_pct: f32,
    pub cpu_relative_pct: f32,
    pub utime_ticks: u64,
    pub stime_ticks: u64,
    /// start_ticks × 1e9 / ticks_per_sec, filled by E05-A1.
    pub start_boot_ns: u64,
    pub rss_peak_bytes: u64,
    pub rss_shared_bytes: u64,
    pub commit_bytes: u64,
    pub vsize_bytes: u64,
    pub vsize_peak_bytes: u64,
    pub page_faults: u64,
    pub page_faults_delta: u64,
    pub major_faults: u64,
    pub policy: SchedPolicy,
    pub rt_prio: u8,
    pub affinity: CpuSet,
    pub handles: Option<u32>,
    pub exe_name: Arc<str>,
    pub exe_path: Arc<str>,
    pub exe_deleted: bool,
    pub unit: Option<UnitRef>,
    pub state_letter: u8,
    pub frozen: bool,
    pub disk_read_bps: f32,
    pub disk_write_raw_bps: f32,
    pub flag_since_ns: TimeNs,
    /// /proc stat field 5: process group id. 0 when the kernel gave a negative value.
    pub pgrp: u32,
    /// /proc stat field 6: session id. 0 when the kernel gave a negative value.
    pub sid: u32,
    /// /proc stat field 39: the CPU the process last ran on. 0 when the kernel gave a negative value.
    pub last_cpu: u32,
    /// The Windows side of a Wine process, `None` for every other process. Shared by every
    /// sweep until the process's metadata is read again.
    pub wine: Option<Arc<WineProcess>>,
    /// Phase-2 memory and I/O counters (PROC-030, PROC-031).
    pub counters: ProcCounters,
}

/// `io.stat` of one cgroup, summed over its devices (PROC-031). Bytes and operation counts are
/// totals since the cgroup started. The `_delta` fields are the change since the last sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CgroupIo {
    /// Bytes read (`rbytes`).
    pub rbytes: u64,
    /// Bytes written (`wbytes`).
    pub wbytes: u64,
    /// Read operations (`rios`).
    pub rios: u64,
    /// Write operations (`wios`).
    pub wios: u64,
    /// Read operations since the last sample.
    pub rios_delta: u64,
    /// Write operations since the last sample.
    pub wios_delta: u64,
}

/// Per-process counters behind the phase-2 memory and I/O columns (PROC-030, PROC-031).
/// They sit next to their P1 siblings so they sort and sum like them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcCounters {
    /// Highest `commit_bytes` tasklens has seen for this key.
    pub commit_peak_bytes: u64,
    /// Signed change of `commit_bytes` since the previous sample.
    pub commit_delta_bytes: i64,
    /// `VmPTE` from `/proc/<pid>/status`.
    pub page_tables_bytes: u64,
    /// Change of `majflt` (stat field 12) since the previous sample.
    pub major_faults_delta: u64,
    /// True when `/proc/<pid>/io` was read this sweep.
    pub io_read: bool,
    /// `/proc/<pid>/io` `read_bytes`.
    pub read_bytes: u64,
    /// `/proc/<pid>/io` `write_bytes`.
    pub write_bytes: u64,
    /// Change of `read_bytes` since the previous sample.
    pub read_bytes_delta: u64,
    /// Change of `write_bytes` since the previous sample.
    pub write_bytes_delta: u64,
    /// `/proc/<pid>/io` `rchar`.
    pub rchar: u64,
    /// `/proc/<pid>/io` `wchar`.
    pub wchar: u64,
    /// Change of `rchar` since the previous sample.
    pub rchar_delta: u64,
    /// Change of `wchar` since the previous sample.
    pub wchar_delta: u64,
    /// `/proc/<pid>/io` `syscr`.
    pub syscr: u64,
    /// `/proc/<pid>/io` `syscw`.
    pub syscw: u64,
    /// `rchar` change per second.
    pub rchar_bps: f32,
    /// `wchar` change per second.
    pub wchar_bps: f32,
}

impl ProcCounters {
    /// Adds `other` into `self` for a group row. Sums saturate. `commit_peak_bytes` is summed
    /// too (the UI shows that column blank on group rows, PROC-030). `io_read` is true only
    /// when both sides were read.
    pub fn add(&mut self, other: &ProcCounters) {
        self.commit_peak_bytes = self
            .commit_peak_bytes
            .saturating_add(other.commit_peak_bytes);
        self.commit_delta_bytes = self
            .commit_delta_bytes
            .saturating_add(other.commit_delta_bytes);
        self.page_tables_bytes = self
            .page_tables_bytes
            .saturating_add(other.page_tables_bytes);
        self.major_faults_delta = self
            .major_faults_delta
            .saturating_add(other.major_faults_delta);
        self.io_read = self.io_read && other.io_read;
        self.read_bytes = self.read_bytes.saturating_add(other.read_bytes);
        self.write_bytes = self.write_bytes.saturating_add(other.write_bytes);
        self.read_bytes_delta = self.read_bytes_delta.saturating_add(other.read_bytes_delta);
        self.write_bytes_delta = self
            .write_bytes_delta
            .saturating_add(other.write_bytes_delta);
        self.rchar = self.rchar.saturating_add(other.rchar);
        self.wchar = self.wchar.saturating_add(other.wchar);
        self.rchar_delta = self.rchar_delta.saturating_add(other.rchar_delta);
        self.wchar_delta = self.wchar_delta.saturating_add(other.wchar_delta);
        self.syscr = self.syscr.saturating_add(other.syscr);
        self.syscw = self.syscw.saturating_add(other.syscw);
        self.rchar_bps += other.rchar_bps;
        self.wchar_bps += other.wchar_bps;
    }
}

/// The value for tests and for fields a sweep did not read. The sweep still writes every field.
impl Default for ProcessExtras {
    fn default() -> Self {
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
            affinity: CpuSet::default(),
            handles: None,
            exe_name: Arc::from(""),
            exe_path: Arc::from(""),
            exe_deleted: false,
            unit: None,
            state_letter: 0,
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
            pgrp: 0,
            sid: 0,
            last_cpu: 0,
            wine: None,
            counters: ProcCounters::default(),
        }
    }
}

/// What a Wine process runs (memory groups, D-036). Read with the process's metadata.
///
/// Privacy: both paths are user data. Never log them above debug level (ADR-015).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WineProcess {
    /// `WINEPREFIX` from the process environment, else `$HOME/.wine`, with trailing `/`
    /// removed. `None` when the environment cannot be read or has neither variable.
    pub prefix: Option<Arc<str>>,
    /// The first argv token that ends in `.exe`, as Wine wrote it:
    /// `C:\Program Files\Backblaze\bztransmit.exe`. `None` when argv has no such token.
    pub image: Option<Arc<str>>,
}

/// One app cgroup sample (R-141 item 2). Filled by the tl-collect sweep (E05-A1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppCgroupSample {
    pub group: GroupKey,
    pub anon_bytes: u64,
    pub swap_bytes: u64,
    pub io_read_bps: f32,
    pub io_write_bps: f32,
    pub rios: u64,
    pub wios: u64,
    /// Cumulative `rbytes` of `io.stat`, summed over devices. Used for "Disk read bytes" when a
    /// member is Restricted (PROC-008).
    pub rbytes: u64,
    /// Cumulative `wbytes` of `io.stat`, summed over devices.
    pub wbytes: u64,
    /// Change of `rios` since the previous sweep of this cgroup. 0 on the first sweep, after a
    /// resume, and when a counter went down.
    pub rios_delta: u64,
    /// Change of `wios` since the previous sweep of this cgroup, with the same zero rules.
    pub wios_delta: u64,
    pub pids: u32,
    /// `cgroup.events` has `frozen 1`. False when the file was not read.
    pub frozen: bool,
    pub cgroup: CgroupRef,
    pub validity: FieldValidity,
}

impl AppCgroupSample {
    /// A sample with the given identity and every other field 0 or `false`.
    pub fn empty(group: GroupKey, cgroup: CgroupRef, validity: FieldValidity) -> AppCgroupSample {
        AppCgroupSample {
            group,
            anon_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            rios: 0,
            wios: 0,
            rbytes: 0,
            wbytes: 0,
            rios_delta: 0,
            wios_delta: 0,
            pids: 0,
            frozen: false,
            cgroup,
            validity,
        }
    }
}

/// P2 per-process column values, filled only for `Demand.detail_keys` (ADR-008 visible-rows
/// rule) by E05-035. The reducer (E09-030) fills `power_w` and `power_trend_w` for every row
/// (R-163 items 2 and 9), and `gpu_vram_bytes` too. `None` means the value was not read.
///
/// Privacy: `security_label` and `cwd` are user data. Never log them above debug level (ADR-015).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ProcessExtrasP2 {
    /// Some for a filled entry, None for a row outside detail_keys.
    pub key: Option<ProcessKey>,
    /// statx mtime of the exe.
    pub exe_mtime_s: Option<i64>,
    /// statx size of the exe.
    pub exe_size_bytes: Option<u64>,
    /// 32 or 64.
    pub elf_class: Option<u8>,
    /// ELF e_machine.
    pub elf_machine: Option<u16>,
    /// /proc/<pid>/sessionid.
    pub session_id: Option<u32>,
    /// /proc/<pid>/attr/current.
    pub security_label: Option<Arc<str>>,
    /// PROC-028 elevation rule.
    pub elevation: Option<crate::details::Elevated>,
    /// PROC-028 sandbox rule.
    pub sandbox: Option<Arc<str>>,
    pub sched_policy_text: Option<Arc<str>>,
    pub oom_score: Option<i32>,
    pub oom_score_adj: Option<i16>,
    /// Decoded tty_nr.
    pub tty: Option<Arc<str>>,
    pub cwd: Option<Arc<str>>,
    /// Sum over task/*/status.
    pub ctx_switches_voluntary: Option<u64>,
    /// Sum over task/*/status.
    pub ctx_switches_involuntary: Option<u64>,
    /// PROC-034 estimate (E09-030).
    pub power_w: Option<f32>,
    /// 2-minute average of power_w (E09-030).
    pub power_trend_w: Option<f32>,
    /// Change since the previous fill of this key.
    pub ctx_switches_delta: Option<u64>,
    /// Owning package (E05-034 index), PROC-027.
    pub package: Option<Arc<PackageInfo>>,
    /// PROC-027 Package status column.
    pub package_status: Option<PackageStatus>,
    /// Dedicated GPU memory per GPU, in the order of the `gpu_0` to `gpu_15` columns (PROC-033),
    /// from the per-process GPU samples. Empty for a process with no GPU memory; shorter than
    /// the GPU count when the last GPUs hold none of it.
    pub gpu_vram_bytes: GpuVram,
    /// Shared (GTT) GPU memory: AMD `drm-memory-gtt`, Intel `drm-resident-system`. `None` for
    /// NVIDIA, which has no per-process value. The DRM rows fill it.
    pub gpu_mem_shared_bytes: Option<u64>,
    /// Encoder utilization, the max over the process's devices: NVML encUtil, or for AMD and
    /// Intel the fdinfo `drm-engine-video*` busy percent (PROC-033) and, by leader decision
    /// 2026-11-06, amdgpu `drm-engine-enc`. `None` when no device that supports process
    /// utilization has a sample for it (PROC-033 `n/a`).
    pub gpu_encode_pct: Option<f32>,
    /// Decoder utilization, as `gpu_encode_pct`: NVML decUtil, fdinfo `drm-engine-video*`, or
    /// amdgpu `drm-engine-dec`.
    pub gpu_decode_pct: Option<f32>,
    /// The busiest engine over all devices, as `gpu_encode_pct`.
    pub gpu_engine: Option<GpuEngineUse>,
    /// I/O priority of the main thread as set, read by the engine with `ioprio_get` (R-144).
    /// `IoPriority::None` is a thread with no class set, which follows nice. `None` when the read
    /// failed or the value was not read.
    pub io_priority: Option<IoPriority>,
    /// PROC-036 Container. `None` means not read.
    pub container: Option<ContainerKind>,
}

/// PROC-036 container kind. `Host` is a process in the same PID namespace as tasklens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerKind {
    Host,
    Podman,
    Docker,
    Nspawn,
    /// In a child PID namespace, with a cgroup path that names no known runtime.
    Other,
}

/// The busiest GPU engine of a process (PROC-033 GPU engine column).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpuEngineUse {
    /// The GPU N column slot (`gpu_0` is 0).
    pub gpu: u8,
    pub engine: GpuEngineKind,
    pub pct: f32,
}

/// Engine of a `GpuEngineUse`. On a tie the order is `Compute3d`, `Encode`, `Decode`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuEngineKind {
    Compute3d,
    Encode,
    Decode,
    /// A DRM engine by name.
    Drm(Arc<str>),
}

/// Owning package of an executable (R-163 item 1, PROC-027), from the E05-034 index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: Arc<str>,
    pub version: Arc<str>,
    pub description: Option<Arc<str>>,
    pub packager: Option<Arc<str>>,
    pub build_date_s: Option<i64>,
    pub url: Option<Arc<str>>,
}

/// PROC-027 Package status. `PackagedModified` only after a `pacman -Qkk` check (PROC-078).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageStatus {
    Packaged,
    PackagedModified,
    NotPackaged,
    DeletedUpgraded,
    Flatpak,
    Snap,
    AppImage,
    Wine,
}

/// One group of the low-memory sweep (R-150 item 2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LowMemGroup {
    pub group: GroupKey,
    pub name: Arc<str>,
    pub process_count: u32,
    pub uid: u32,
    pub cgroup: CgroupRef,
    pub memory_current: u64,
}

/// Low-memory sweep data (R-150 item 2), moved from E32's types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LowMemoryInfo {
    pub groups: Arc<[LowMemGroup]>,
    pub mem_total: u64,
    pub mem_available: u64,
    pub psi_full_pct: Option<f32>,
    pub critical: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::BootId;

    #[test]
    fn proc_counters_add_sums_each_field() {
        let a = ProcCounters {
            commit_peak_bytes: 10,
            commit_delta_bytes: 5,
            page_tables_bytes: 1,
            major_faults_delta: 2,
            io_read: true,
            read_bytes: 3,
            write_bytes: 4,
            read_bytes_delta: 5,
            write_bytes_delta: 6,
            rchar: 7,
            wchar: 8,
            rchar_delta: 9,
            wchar_delta: 10,
            syscr: 11,
            syscw: 12,
            rchar_bps: 1.5,
            wchar_bps: 2.5,
        };
        let mut sum = a;
        sum.add(&a);
        assert_eq!(sum.commit_peak_bytes, 20);
        assert_eq!(sum.commit_delta_bytes, 10);
        assert_eq!(sum.page_tables_bytes, 2);
        assert_eq!(sum.major_faults_delta, 4);
        assert!(sum.io_read);
        assert_eq!((sum.read_bytes, sum.write_bytes), (6, 8));
        assert_eq!((sum.read_bytes_delta, sum.write_bytes_delta), (10, 12));
        assert_eq!((sum.rchar, sum.wchar), (14, 16));
        assert_eq!((sum.rchar_delta, sum.wchar_delta), (18, 20));
        assert_eq!((sum.syscr, sum.syscw), (22, 24));
        assert_eq!((sum.rchar_bps, sum.wchar_bps), (3.0, 5.0));
    }

    #[test]
    fn proc_counters_add_negative_saturating_and_io_and() {
        let mut a = ProcCounters {
            commit_delta_bytes: 7,
            io_read: true,
            ..ProcCounters::default()
        };
        let b = ProcCounters {
            commit_delta_bytes: -10,
            rchar: u64::MAX,
            io_read: false,
            ..ProcCounters::default()
        };
        a.add(&b);
        assert_eq!(a.commit_delta_bytes, -3);
        assert!(!a.io_read);
        a.rchar = 5;
        a.add(&b);
        assert_eq!(a.rchar, u64::MAX);
        let mut m = ProcCounters {
            commit_delta_bytes: i64::MIN,
            ..ProcCounters::default()
        };
        m.add(&b);
        assert_eq!(m.commit_delta_bytes, i64::MIN);
    }

    fn pkey(start_ticks: u64, pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BootId(1),
            pid,
        }
    }

    #[test]
    fn proc_state_from_stat_char() {
        let cases = [
            (b'R', ProcState::Running),
            (b'S', ProcState::Sleeping),
            (b'D', ProcState::DiskSleep),
            (b'T', ProcState::Stopped),
            (b't', ProcState::Stopped),
            (b'Z', ProcState::Zombie),
            (b'I', ProcState::Idle),
            (b'X', ProcState::Other),
        ];
        for (c, want) in cases {
            assert_eq!(ProcState::from_stat_char(c), want, "{}", c as char);
        }
    }

    #[test]
    fn base_values_have_the_documented_defaults() {
        let x = ProcessExtras::default();
        assert_eq!(x.policy, SchedPolicy::Other);
        assert_eq!(x.handles, None);
        let key = GroupKey(7);
        let row = GroupRow::empty(
            NodeId::Group(key),
            key,
            GroupRowKind::App,
            GroupKind::Background,
            Arc::from("n"),
            FieldValidity::default(),
        );
        assert_eq!(row.node, NodeId::Group(key));
        assert_eq!(row.key, key);
        assert_eq!(row.kind, GroupRowKind::App);
        assert_eq!(&*row.name, "n");
        assert_eq!(row.members, 0);
        assert_eq!(row.handles, None);
        assert_eq!(row.counters, ProcCounters::default());
        assert_eq!(row.cgroup_io, None);
        let cg = CgroupRef {
            rel_path: Arc::from("a/b"),
            cgroup_id: 5,
        };
        let s = AppCgroupSample::empty(key, cg.clone(), FieldValidity::default());
        assert_eq!(s.group, key);
        assert_eq!(s.cgroup, cg);
    }

    #[test]
    fn linger_clamp() {
        assert_eq!(linger_ms(250), 1_000);
        assert_eq!(linger_ms(1_000), 2_000);
        assert_eq!(linger_ms(2_000), 3_000);
        assert_eq!(linger_ms(10_000), 3_000);
        assert_eq!(linger_ms(u32::MAX), 3_000);
    }

    #[test]
    fn proc_flags_bits_frozen() {
        assert_eq!(ProcFlags::NEW.bits(), 1);
        assert_eq!(ProcFlags::EXITED.bits(), 2);
        assert_eq!(ProcFlags::KERNEL_THREAD.bits(), 4);
        assert_eq!(ProcFlags::OTHER_USER.bits(), 8);
        assert_eq!(ProcFlags::IS_SELF.bits(), 16);
        assert_eq!(ProcFlags::PROTECTED.bits(), 32);
    }

    #[test]
    fn coverage_lookup() {
        let keys: Arc<[ProcessKey]> = vec![pkey(10, 1), pkey(20, 2), pkey(30, 3)].into();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        let coverage = Coverage { pss_uss: keys };
        assert!(coverage.has_pss_uss(&pkey(20, 2)));
        assert!(!coverage.has_pss_uss(&pkey(25, 2)));
        assert!(!Coverage::default().has_pss_uss(&pkey(10, 1)));
    }

    #[test]
    fn record_clone_shares_meta() {
        let meta = Arc::new(ProcessMeta {
            comm: "bash".into(),
            name: "bash".into(),
            exe: Some("/usr/bin/bash".into()),
            cmdline: Some("bash -l".into()),
            user: "user".into(),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: "bash bash -l user".into(),
        });
        let a = ProcessRecord {
            key: pkey(10, 1),
            ppid: 0,
            uid: 1000,
            meta,
            group: GroupKind::Background.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::NEW,
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
            validity: FieldValidity::default(),
        };
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.meta, &b.meta));
    }

    #[test]
    fn group_kind_key_stable() {
        let a = GroupKind::App {
            app_id: "firefox".into(),
        };
        let b = GroupKind::App {
            app_id: "firefox".into(),
        };
        assert_eq!(a.key(), b.key());
        assert_ne!(GroupKind::Background.key(), a.key());
        assert_ne!(GroupKind::Background.key(), b.key());
    }

    #[test]
    fn unit_key_unique_per_unit() {
        let unit = |manager, name: &str| UnitRef {
            manager,
            name: name.into(),
        };
        let foo = unit(Manager::System, "foo.service");
        assert_eq!(foo.key(), unit(Manager::System, "foo.service").key());
        assert_ne!(foo.key(), unit(Manager::System, "bar.service").key());
        assert_ne!(foo.key(), unit(Manager::User, "foo.service").key());
        for kind in [
            GroupKind::Background,
            GroupKind::System,
            GroupKind::Ungrouped,
            GroupKind::App {
                app_id: "foo.service".into(),
            },
        ] {
            assert_ne!(foo.key(), kind.key());
        }
    }

    #[test]
    fn extras_p2_default_all_none() {
        let p2 = ProcessExtrasP2::default();
        assert!(p2.key.is_none());
        assert!(p2.exe_mtime_s.is_none());
        assert!(p2.exe_size_bytes.is_none());
        assert!(p2.elf_class.is_none());
        assert!(p2.elf_machine.is_none());
        assert!(p2.session_id.is_none());
        assert!(p2.security_label.is_none());
        assert!(p2.elevation.is_none());
        assert!(p2.sandbox.is_none());
        assert!(p2.sched_policy_text.is_none());
        assert!(p2.oom_score.is_none());
        assert!(p2.oom_score_adj.is_none());
        assert!(p2.tty.is_none());
        assert!(p2.cwd.is_none());
        assert!(p2.io_priority.is_none());
        assert!(p2.ctx_switches_voluntary.is_none());
        assert!(p2.ctx_switches_involuntary.is_none());
        assert!(p2.power_w.is_none());
        assert!(p2.power_trend_w.is_none());
        assert!(p2.ctx_switches_delta.is_none());
        assert!(p2.package.is_none());
        assert!(p2.package_status.is_none());
        assert!(p2.gpu_mem_shared_bytes.is_none());
        assert!(p2.gpu_encode_pct.is_none());
        assert!(p2.gpu_decode_pct.is_none());
        assert!(p2.gpu_engine.is_none());
        assert!(p2.container.is_none());
    }

    #[test]
    fn low_memory_info_clone() {
        let info = LowMemoryInfo {
            groups: Arc::from([LowMemGroup {
                group: GroupKey(3),
                name: Arc::from("firefox"),
                process_count: 12,
                uid: 1000,
                cgroup: CgroupRef {
                    rel_path: Arc::from("user.slice/app-firefox.scope"),
                    cgroup_id: 4242,
                },
                memory_current: 1 << 30,
            }]),
            mem_total: 16 << 30,
            mem_available: 512 << 20,
            psi_full_pct: Some(12.5),
            critical: true,
        };
        let copy = info.clone();
        assert_eq!(copy, info);
        // The group slice is shared, not copied.
        assert!(Arc::ptr_eq(&copy.groups, &info.groups));
    }
}
