//! The published snapshot and its components (ADR-003).

use std::collections::HashMap;
use std::ops::{BitOr, BitOrAssign};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::actions::{Family, FourTuple, Proto};
use crate::details::DetailsSnapshot;
use crate::devices::{DeviceCatalog, NetKind};
use crate::health::{EngineCaps, HealthSnapshot, HelperState, ManagerState, SourceHealth};
use crate::history::HistoryView;
use crate::ids::{DeviceIdx, ProcessKey, ProcessSeriesId, SeriesId};
use crate::process::{
    AppCgroupSample, Coverage, LowMemoryInfo, ProcessExtras, ProcessExtrasP2, ProcessRecord,
};
use crate::series::SeriesView;
use crate::system::{
    CpuTopology, CpuTotals, DiskTotals, GpuTotals, LinkState, MemoryTotals, NetTotals,
    SensorReading,
};
use crate::table::{Manager, ProcessTableView, StartupRef, SysRow, Total, UnitRef, UnitType};
use crate::time::{BootId, TimeNs};
use crate::validity::Validity;

/// Common to every snapshot component. The default validity is `WarmingUp`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ComponentHeader {
    /// Bumps when the component content changes.
    pub version: u64,
    pub observed_start: TimeNs,
    pub observed_end: TimeNs,
    pub validity: Validity,
}

/// One bit per `Snapshot` component, computed by the UI hub (ADR-005). Never stored, so the bit
/// positions are not part of any file format. They follow the order ADR-003 lists them in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct ComponentMask(pub u16);

impl ComponentMask {
    pub const SYSTEM: Self = Self(1 << 0);
    pub const SERIES: Self = Self(1 << 1);
    pub const PROCESSES: Self = Self(1 << 2);
    pub const TABLE: Self = Self(1 << 3);
    pub const CONNECTIONS: Self = Self(1 << 4);
    pub const SERVICES: Self = Self(1 << 5);
    pub const STARTUP: Self = Self(1 << 6);
    pub const INVENTORY: Self = Self(1 << 7);
    pub const DEVICES: Self = Self(1 << 8);
    pub const HEALTH: Self = Self(1 << 9);
    pub const WINDOWS: Self = Self(1 << 10);
    pub const DETAILS: Self = Self(1 << 11);
    pub const HISTORY: Self = Self(1 << 12);
    /// `Snapshot.process_series` (R-154).
    pub const PROCESS_SERIES: Self = Self(1 << 13);
    /// `Snapshot.device_details` (R-139).
    pub const DEVICE_DETAILS: Self = Self(1 << 14);
    /// All fifteen bits.
    pub const ALL: Self = Self(0x7FFF);
    pub const NONE: Self = Self(0);

    /// True when every bit of `other` is set in `self`.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// True when `self` and `other` share at least one bit.
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOr for ComponentMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl BitOrAssign for ComponentMask {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = self.union(rhs);
    }
}

/// Every published system series. Unchanged series reuse their `Arc` across snapshots.
#[derive(Clone, Debug, Default)]
pub struct SeriesSet {
    pub header: ComponentHeader,
    /// Sorted by `SeriesId`.
    pub series: Arc<[Arc<SeriesView>]>,
    /// 1 Hz tier views. Their ids carry `MetricId::TIER_1HZ`. Sorted by `SeriesId` (R-141 item 4,
    /// R-170 item 2).
    pub slow: Arc<[Arc<SeriesView>]>,
}

impl SeriesSet {
    /// Binary search over `slow` when the id carries the tier bit, else over `series`.
    pub fn get(&self, id: SeriesId) -> Option<&Arc<SeriesView>> {
        let views = if id.metric.is_tier_1hz() {
            &self.slow
        } else {
            &self.series
        };
        views
            .binary_search_by(|v| v.id.cmp(&id))
            .ok()
            .map(|i| &views[i])
    }
}

/// Engine-sampled per-process series for `Demand.graph_targets` (R-154).
#[derive(Clone, Debug, Default)]
pub struct ProcessSeriesSet {
    pub header: ComponentHeader,
    /// Fast tier views (60 s at the sweep period). Sorted by `ProcessSeriesId`.
    pub series: Arc<[(ProcessSeriesId, Arc<SeriesView>)]>,
    /// 1 Hz tier views (per-second maximum, 602 samples), one per entry of `series`. Sorted by
    /// `ProcessSeriesId`. The view ids carry no tier bit, since process series are not keyed by
    /// `MetricId`.
    pub slow: Arc<[(ProcessSeriesId, Arc<SeriesView>)]>,
}

impl ProcessSeriesSet {
    /// The fast tier view of `id`. Binary search over `series`.
    pub fn get(&self, id: &ProcessSeriesId) -> Option<&Arc<SeriesView>> {
        find_process_view(&self.series, id)
    }

    /// The 1 Hz tier view of `id`. Binary search over `slow`.
    pub fn get_slow(&self, id: &ProcessSeriesId) -> Option<&Arc<SeriesView>> {
        find_process_view(&self.slow, id)
    }
}

fn find_process_view<'a>(
    views: &'a [(ProcessSeriesId, Arc<SeriesView>)],
    id: &ProcessSeriesId,
) -> Option<&'a Arc<SeriesView>> {
    views
        .binary_search_by(|(k, _)| k.cmp(id))
        .ok()
        .map(|i| &views[i].1)
}

#[derive(Clone, Debug, Default)]
pub struct ProcessSnapshot {
    pub header: ComponentHeader,
    /// Sorted by `ProcessKey`.
    pub records: Arc<[ProcessRecord]>,
    pub coverage: Coverage,
    /// Index aligned with `records` (R-141 item 1).
    pub extras: Arc<[ProcessExtras]>,
    /// Sorted by `GroupKey` (R-141 item 2).
    pub apps: Arc<[AppCgroupSample]>,
    /// Index aligned with `records`. Default entries outside `detail_keys` (R-163 item 2).
    pub extras_p2: Arc<[ProcessExtrasP2]>,
    /// Some only in `EngineMode::LowMemory` (R-150 item 2).
    pub low_memory: Option<Arc<LowMemoryInfo>>,
}

impl ProcessSnapshot {
    /// Binary search by key.
    pub fn get(&self, key: ProcessKey) -> Option<&ProcessRecord> {
        self.records
            .binary_search_by(|r| r.key.cmp(&key))
            .ok()
            .map(|i| &self.records[i])
    }
}

/// Machine totals. Per-device lists are sorted by `DeviceIdx`.
#[derive(Clone, Debug, Default)]
pub struct SystemSnapshot {
    pub header: ComponentHeader,
    pub cpu: CpuTotals,
    pub memory: MemoryTotals,
    pub disks: Arc<[(DeviceIdx, DiskTotals)]>,
    pub nets: Arc<[(DeviceIdx, NetTotals)]>,
    pub gpus: Arc<[(DeviceIdx, GpuTotals)]>,
    pub sensors: Arc<[(DeviceIdx, SensorReading)]>,
    /// Read at start and on CPU hotplug.
    pub cpu_topology: Arc<CpuTopology>,
    /// /proc/stat btime. The UI formats Start time from it (R-141 item 1).
    pub boot_time_s: i64,
    /// Portal org.freedesktop.appearance color-scheme: 0 none, 1 dark, 2 light (R-170 item 4).
    pub color_scheme: u8,
}

/// Window frame in logical pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FrameGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// One KWin window (R-046, PROC-052). Captions are untrusted text from other programs: the UI
/// shows them through its untrusted-text path and this crate never logs them.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowEntry {
    /// KWin internalId, a braced UUID, as given.
    pub internal_id: Arc<str>,
    pub pid: u32,
    pub caption: Arc<str>,
    pub resource_class: Arc<str>,
    pub desktop_file_name: Arc<str>,
    pub minimized: bool,
    pub maximized: bool,
    pub keep_above: bool,
    pub frame_geometry: FrameGeometry,
    /// KWin windowType value as reported.
    pub window_type: i32,
}

#[derive(Clone, Debug, Default)]
pub struct WindowsSnapshot {
    /// Validity is `Unsupported` when the KWin bridge is not available.
    pub header: ComponentHeader,
    /// Stable-sorted by `pid`; windows with the same pid keep KWin's order.
    pub windows: Arc<[WindowEntry]>,
    /// Latest window-pick answer (R-149 item 3).
    pub pick: Option<PickResult>,
}

impl WindowsSnapshot {
    /// The windows of one process: a binary-searched range of `windows`.
    pub fn for_pid(&self, pid: u32) -> &[WindowEntry] {
        let start = self.windows.partition_point(|w| w.pid < pid);
        let end = self.windows.partition_point(|w| w.pid <= pid);
        &self.windows[start..end]
    }
}

/// `seq` comes from `KWinCommand::Pick { seq }`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickResult {
    pub seq: u64,
    pub outcome: PickOutcome,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickOutcome {
    Picked { pid: u32 },
    NoWindow,
    Cancelled,
}

/// Services table status (SYS-003).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ServiceStatus {
    Running,
    Active,
    Starting,
    Stopping,
    Reloading,
    Stopped,
    Failed,
    Maintenance,
}

impl ServiceStatus {
    /// SYS-003 mapping: active+running is Running; any other active sub-state (exited,
    /// listening, mounted, waiting, elapsed, plugged) is Active; activating is Starting;
    /// deactivating is Stopping; reloading or refreshing is Reloading; inactive is Stopped;
    /// failed is Failed; maintenance is Maintenance. An unknown active state is Stopped.
    pub fn from_states(active_state: &str, sub_state: &str) -> ServiceStatus {
        match active_state {
            "active" if sub_state == "running" => ServiceStatus::Running,
            "active" => ServiceStatus::Active,
            "activating" => ServiceStatus::Starting,
            "deactivating" => ServiceStatus::Stopping,
            "reloading" | "refreshing" => ServiceStatus::Reloading,
            "failed" => ServiceStatus::Failed,
            "maintenance" => ServiceStatus::Maintenance,
            _ => ServiceStatus::Stopped,
        }
    }
}

/// Startup type column, from the systemd UnitFileState word.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StartupType {
    Enabled,
    EnabledRuntime,
    Disabled,
    Static,
    Indirect,
    Masked,
    Generated,
    Transient,
    Alias,
    Linked,
    /// No unit file. Shown as "-".
    NoUnitFile,
}

impl StartupType {
    /// masked-runtime maps to Masked and linked-runtime to Linked. An empty or unknown word is
    /// NoUnitFile.
    pub fn from_unit_file_state(s: &str) -> StartupType {
        match s {
            "enabled" => StartupType::Enabled,
            "enabled-runtime" => StartupType::EnabledRuntime,
            "disabled" => StartupType::Disabled,
            "static" => StartupType::Static,
            "indirect" => StartupType::Indirect,
            "masked" | "masked-runtime" => StartupType::Masked,
            "generated" => StartupType::Generated,
            "transient" => StartupType::Transient,
            "alias" => StartupType::Alias,
            "linked" | "linked-runtime" => StartupType::Linked,
            _ => StartupType::NoUnitFile,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ServiceBackend {
    Systemd,
    OpenRc,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ServiceCounts {
    pub total: u32,
    pub running: u32,
    pub stopped: u32,
}

/// One unit as the Services table shows it (UI-SYS-015). Resource fields are `Missing` when the
/// unit has no cgroup, so the column shows "-" and never 0 (SYS-003).
#[derive(Clone, Debug)]
pub struct UnitRecord {
    pub unit: UnitRef,
    pub kind: UnitType,
    /// Description=, the "Display name" column.
    pub description: Arc<str>,
    pub status: ServiceStatus,
    pub sub_state: Arc<str>,
    pub load_state: Arc<str>,
    pub startup: StartupType,
    /// 0 means none, and the column is blank.
    pub main_pid: u32,
    /// SYS-003 rule: the real uid owner of MainPID, else User=, else root or the current user.
    pub user: Arc<str>,
    /// SYS-003 Path column rule.
    pub path: Arc<str>,
    /// FragmentPath.
    pub unit_file: Option<Arc<str>>,
    /// ActiveEnterTimestamp in CLOCK_REALTIME microseconds. `None` when systemd reports 0.
    pub active_since_us: Option<u64>,
    pub triggered_by: Arc<[Arc<str>]>,
    /// ControlGroup, the unit's cgroup path. `None` when systemd reports it empty or the unit
    /// type has none.
    pub control_group: Option<Arc<str>>,
    /// CanReload: the Reload action is offered only when true.
    pub can_reload: bool,
    pub has_cgroup_procs: bool,
    /// Child process rows, sorted by `ProcessKey`.
    pub cgroup_pids: Arc<[ProcessKey]>,
    /// cgroup cpu.stat usage_usec delta.
    pub cpu_pct: Total<f32>,
    /// memory.current.
    pub memory_bytes: Total<u64>,
    /// io.stat rbytes + wbytes delta per second.
    pub disk_bps: Total<f64>,
    /// pids.current.
    pub tasks: Total<u64>,
}

/// Recovery tab values (SYS-013), all from unit properties. `None` shows "Not reported".
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RecoverySettings {
    /// Restart= word.
    pub restart: Option<Arc<str>>,
    /// RestartUSec.
    pub restart_usec: Option<u64>,
    /// RestartSteps.
    pub restart_steps: Option<u32>,
    /// RestartMaxDelayUSec.
    pub restart_max_delay_usec: Option<u64>,
    pub start_limit_burst: Option<u32>,
    pub start_limit_interval_usec: Option<u64>,
    pub start_limit_action: Option<Arc<str>>,
    pub failure_action: Option<Arc<str>>,
    /// OnFailure= units. Empty shows "None".
    pub on_failure: Arc<[Arc<str>]>,
    /// NRestarts.
    pub n_restarts: Option<u32>,
}

/// A dependency relation on the Dependencies tab (SYS-014).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Relation {
    // "Depends on:"
    Requires,
    Requisite,
    Wants,
    BindsTo,
    PartOf,
    Upholds,
    // "Dependents:"
    RequiredBy,
    RequisiteOf,
    WantedBy,
    BoundBy,
    ConsistsOf,
    UpheldBy,
    // "Ordering"
    After,
    Before,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DepEntry {
    pub unit: UnitRef,
    pub relation: Relation,
}

/// Shown as "Runs as: uid 1000, gid 1000".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PidIds {
    pub uid: u32,
    pub gid: u32,
}

/// What the Service Properties window shows for one unit. `exec_start_argv` can hold secrets
/// passed on command lines: never log it above debug level (ADR-015).
#[derive(Clone, Debug)]
pub struct UnitDetails {
    // General (SYS-011, UI-SYS-031)
    pub id: Arc<str>,
    pub description: Arc<str>,
    pub documentation: Arc<[Arc<str>]>,
    pub exec_start_argv: Arc<[Arc<str>]>,
    pub fragment_path: Option<Arc<str>>,
    pub drop_in_paths: Arc<[Arc<str>]>,
    pub startup: StartupType,
    pub active_state: Arc<str>,
    pub sub_state: Arc<str>,
    /// CLOCK_REALTIME microseconds.
    pub active_enter_timestamp_us: Option<u64>,
    pub main_pid: u32,
    /// `None` when systemd reports u64::MAX.
    pub memory_current: Option<u64>,
    /// `None` when systemd reports u64::MAX.
    pub tasks_current: Option<u64>,
    pub cpu_usage_nsec: Option<u64>,
    pub manager: Manager,
    // Log On (SYS-012, UI-SYS-032)
    pub user: Option<Arc<str>>,
    pub group: Option<Arc<str>>,
    pub dynamic_user: bool,
    pub supplementary_groups: Arc<[Arc<str>]>,
    pub pam_name: Option<Arc<str>>,
    /// `None` when MainPID is 0 ("Not running").
    pub main_pid_ids: Option<PidIds>,
    pub graphical_session: bool,
    // Recovery (SYS-013, UI-SYS-033)
    pub recovery: RecoverySettings,
    pub recovery_dropin_exists: bool,
    // Dependencies (SYS-014, UI-SYS-034)
    /// Sorted by unit name, one row per relation.
    pub deps: Arc<[DepEntry]>,
}

/// The Services tab component (ADR-003). Filled by the systemd workers, read by the UI.
#[derive(Clone, Debug)]
pub struct ServicesSnapshot {
    pub header: ComponentHeader,
    pub rows: Arc<[SysRow]>,
    pub query_generation: u64,
    pub units: Arc<[UnitRecord]>,
    pub counts: ServiceCounts,
    /// Index 0 is System, 1 is User.
    pub managers: [ManagerState; 2],
    /// Units per manager after all filters, collapsed groups included. Index 0 is System, 1 is User.
    pub manager_counts: [u32; 2],
    /// Units in `Demand.unit_details`, at most 10.
    pub details: Arc<HashMap<UnitRef, Arc<UnitDetails>>>,
    pub backend: ServiceBackend,
}

impl Default for ServicesSnapshot {
    fn default() -> Self {
        Self {
            header: ComponentHeader::default(),
            rows: Arc::from([]),
            query_generation: 0,
            units: Arc::from([]),
            counts: ServiceCounts::default(),
            managers: [ManagerState::default(); 2],
            manager_counts: [0; 2],
            details: Arc::default(),
            backend: ServiceBackend::Systemd,
        }
    }
}

// ---- Startup Apps ----

/// Startup Apps Status column (SYS-020).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StartupStatus {
    Enabled,
    Disabled,
    Masked,
    NotInThisDesktop,
    MissingProgram,
}

impl StartupStatus {
    pub const fn label(self) -> &'static str {
        match self {
            StartupStatus::Enabled => "Enabled",
            StartupStatus::Disabled => "Disabled",
            StartupStatus::Masked => "Masked",
            StartupStatus::NotInThisDesktop => "Not in this desktop",
            StartupStatus::MissingProgram => "Missing program",
        }
    }
}

/// Startup Apps Type column (SYS-020).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StartupKind {
    AutostartUser,
    AutostartSystem,
    AutostartSystemOverridden,
    UserUnit,
    PlasmaPreStartup,
    PlasmaLogout,
}

impl StartupKind {
    pub const fn label(self) -> &'static str {
        match self {
            StartupKind::AutostartUser => "Autostart (user)",
            StartupKind::AutostartSystem => "Autostart (system)",
            StartupKind::AutostartSystemOverridden => "Autostart (system, overridden)",
            StartupKind::UserUnit => "systemd user unit",
            StartupKind::PlasmaPreStartup => "Plasma pre-startup script",
            StartupKind::PlasmaLogout => "Plasma logout script",
        }
    }

    /// The Source word of the Properties "Type / Source / Scope" line (UI-SYS-050).
    pub const fn source_word(self) -> &'static str {
        match self {
            StartupKind::AutostartUser
            | StartupKind::AutostartSystem
            | StartupKind::AutostartSystemOverridden => "XDG autostart",
            StartupKind::UserUnit => "systemd user unit",
            StartupKind::PlasmaPreStartup => "Plasma pre-startup script",
            StartupKind::PlasmaLogout => "Plasma logout script",
        }
    }
}

/// Whether a startup entry's file is the user's or the system's (SYS-026 Scope).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StartupScope {
    User,
    System,
}

impl StartupScope {
    pub const fn label(self) -> &'static str {
        match self {
            StartupScope::User => "user",
            StartupScope::System => "system",
        }
    }
}

/// Startup impact (SYS-021). Ascending sort follows the declared order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Impact {
    High,
    Medium,
    Low,
    /// The entry is disabled.
    None,
    /// The entry did not run through systemd this session.
    NotMeasured,
}

impl Impact {
    /// SYS-021: High at 1000 ms or more, Medium 300..=999 ms, Low under 300 ms.
    pub const fn from_activation_ms(ms: u64) -> Impact {
        if ms >= 1000 {
            Impact::High
        } else if ms >= 300 {
            Impact::Medium
        } else {
            Impact::Low
        }
    }
}

/// The raw numbers behind the impact tooltip (SYS-021).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ImpactDetail {
    /// ActiveEnterTimestampMonotonic minus InactiveExitTimestampMonotonic.
    pub activation_ms: u64,
    /// InactiveExitTimestampMonotonic minus the Manager's UserspaceTimestampMonotonic.
    pub started_after_login_ms: u64,
    /// CPUUsageNSec.
    pub cpu_nsec: Option<u64>,
    /// True when the unit Type= is simple or exec, or unset. The tooltip then adds the fork/exec
    /// note (UI-SYS-043).
    pub fork_exec_only: bool,
}

/// One Startup Apps row (UI-SYS-042). `command` can hold secrets: never log it above debug
/// level (ADR-015).
#[derive(Clone, Debug)]
pub struct StartupEntry {
    pub id: StartupRef,
    pub name: Arc<str>,
    pub icon_path: Option<Arc<str>>,
    /// "name version", "User", or `None` (blank).
    pub publisher: Option<Arc<str>>,
    pub status: StartupStatus,
    pub impact: Impact,
    pub impact_detail: Option<ImpactDetail>,
    /// State code (SYS-026): "<ActiveState> (<SubState>)" of the entry's generated or native
    /// unit, for example "active (running)". `None` when there is no loaded unit; the UI shows
    /// "-".
    pub state_code: Option<Arc<str>>,
    pub kind: StartupKind,
    /// Directory path.
    pub location: Arc<str>,
    /// Exec, ExecStart or the script path.
    pub command: Arc<str>,
    pub source_path: Arc<str>,
    pub can_toggle: bool,
    pub can_delete: bool,
    pub can_run: bool,
    pub overridden_by_user: bool,
    /// Absolute path of the command's program, found on `PATH` when not absolute. `None` when
    /// it was not found; the UI shows "Not found".
    pub resolved_path: Option<Arc<str>>,
    /// At scan time the source file could be read and the program at `resolved_path` was a
    /// file. `false` when either is missing.
    pub file_exists: bool,
    /// Source word (UI-SYS-050), `kind.source_word()`.
    pub source_word: &'static str,
    pub scope: StartupScope,
    /// Desktop `OnlyShowIn`, empty when not set.
    pub only_show_in: SmallVec<[Arc<str>; 2]>,
    /// Desktop `NotShowIn`, empty when not set.
    pub not_show_in: SmallVec<[Arc<str>; 2]>,
    /// Desktop `TryExec`.
    pub try_exec: Option<Arc<str>>,
    /// Desktop `X-KDE-autostart-phase`.
    pub autostart_phase: Option<Arc<str>>,
    /// The systemd unit generated for a desktop entry; `None` for other kinds.
    pub generated_unit: Option<Arc<str>>,
    /// User desktop entry: the system path it shadows.
    pub overrides: Option<Arc<str>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StartupCounts {
    pub total: u32,
    pub enabled: u32,
    pub disabled: u32,
}

/// The Startup Apps tab component (ADR-003). Filled by the Startup worker, read by the UI.
#[derive(Clone, Debug, Default)]
pub struct StartupSnapshot {
    pub header: ComponentHeader,
    pub rows: Arc<[SysRow]>,
    pub query_generation: u64,
    pub entries: Arc<[StartupEntry]>,
    pub counts: StartupCounts,
}

// ---- Connections ----

/// Connection State column (SYS-029). UNCONNECTED and CONNECTED are for UDP, the rest for TCP.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ConnState {
    Listening,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    Closed,
    Unknown,
    Unconnected,
    Connected,
}

impl ConnState {
    /// Labels (SYS-029): LISTENING, SYN-SENT, SYN-RECEIVED, ESTABLISHED, FIN-WAIT-1, FIN-WAIT-2,
    /// CLOSE-WAIT, CLOSING, LAST-ACK, TIME-WAIT, CLOSED, UNKNOWN, UNCONNECTED, CONNECTED.
    pub const fn label(self) -> &'static str {
        match self {
            ConnState::Listening => "LISTENING",
            ConnState::SynSent => "SYN-SENT",
            ConnState::SynReceived => "SYN-RECEIVED",
            ConnState::Established => "ESTABLISHED",
            ConnState::FinWait1 => "FIN-WAIT-1",
            ConnState::FinWait2 => "FIN-WAIT-2",
            ConnState::CloseWait => "CLOSE-WAIT",
            ConnState::Closing => "CLOSING",
            ConnState::LastAck => "LAST-ACK",
            ConnState::TimeWait => "TIME-WAIT",
            ConnState::Closed => "CLOSED",
            ConnState::Unknown => "UNKNOWN",
            ConnState::Unconnected => "UNCONNECTED",
            ConnState::Connected => "CONNECTED",
        }
    }

    /// Kernel TCP state number (include/net/tcp_states.h): 1 ESTABLISHED, 2 SYN_SENT, 3 SYN_RECV,
    /// 4 FIN_WAIT1, 5 FIN_WAIT2, 6 TIME_WAIT, 7 CLOSE, 8 CLOSE_WAIT, 9 LAST_ACK, 10 LISTEN,
    /// 11 CLOSING, 12 NEW_SYN_RECV (-> SynReceived); anything else -> Unknown.
    pub const fn from_tcp_state(n: u8) -> ConnState {
        match n {
            1 => ConnState::Established,
            2 => ConnState::SynSent,
            3 | 12 => ConnState::SynReceived,
            4 => ConnState::FinWait1,
            5 => ConnState::FinWait2,
            6 => ConnState::TimeWait,
            7 => ConnState::Closed,
            8 => ConnState::CloseWait,
            9 => ConnState::LastAck,
            10 => ConnState::Listening,
            11 => ConnState::Closing,
            _ => ConnState::Unknown,
        }
    }
}

/// TCP_INFO values for the connection Properties window (SYS-034).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TcpInfoView {
    /// Shown as "RTT <ms> (var <ms>)".
    pub rtt_us: u32,
    pub rttvar_us: u32,
    /// "Congestion window".
    pub snd_cwnd: u32,
    /// tcpi_bytes_acked.
    pub bytes_acked: u64,
    /// tcpi_bytes_received.
    pub bytes_received: u64,
    /// tcpi_total_retrans.
    pub retransmits: u32,
    /// "Last data received <ms> ago".
    pub last_data_recv_ms: u32,
}

/// One socket as the Connections table shows it (UI-SYS-062).
#[derive(Clone, Debug)]
pub struct ConnRecord {
    pub cookie: u64,
    pub proto: Proto,
    pub family: Family,
    /// For SockDestroy targets.
    pub four_tuple: FourTuple,
    /// Formatted once per cookie by the engine (table d forms).
    pub local: Arc<str>,
    /// "*:*" for listeners and unconnected UDP.
    pub foreign: Arc<str>,
    pub state: ConnState,
    pub owners: SmallVec<[ProcessKey; 1]>,
    /// `Restricted` when the owner is another user's process and needs the helper, `Missing`
    /// when the owner is gone.
    pub owner_validity: Validity,
    pub uid: u32,
    pub inode: u64,
    pub ifindex: u32,
    /// The host interface name, resolved once by the engine. `None` for index 0, an unknown
    /// index, or a socket from another network namespace.
    pub interface: Option<Arc<str>>,
    pub recv_q: u32,
    pub send_q: u32,
    pub unit: Option<UnitRef>,
    pub tcp: Option<TcpInfoView>,
    /// The reverse DNS name of the foreign address (SYS-035). `None` while the lookup is
    /// pending, after a failed lookup, or when the Remote host column is hidden.
    pub remote_host: Option<Arc<str>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ConnCounts {
    pub total: u32,
    pub tcp: u32,
    pub udp: u32,
    pub established: u32,
    pub listening: u32,
}

/// The Connections tab component (ADR-003). Filled by the Connections worker, read by the UI.
#[derive(Clone, Debug, Default)]
pub struct ConnectionsSnapshot {
    pub header: ComponentHeader,
    pub rows: Arc<[SysRow]>,
    pub query_generation: u64,
    pub records: Arc<[ConnRecord]>,
    pub counts: ConnCounts,
    /// Set after the first SockDestroy answer.
    pub destroy_supported: Option<bool>,
}

// ---- Inventory ----

/// One Information tab value with its validity. `Restricted` and `Unsupported` ("Not reported")
/// follow R-050.
pub type InvField<T> = Total<T>;

/// Operating System section (PERF-095).
#[derive(Clone, Debug, Default)]
pub struct OsInfo {
    /// os-release PRETTY_NAME.
    pub name: InvField<Arc<str>>,
    /// os-release LOGO.
    pub logo_icon: Option<Arc<str>>,
    /// VERSION_ID, or "Rolling" when missing.
    pub version: InvField<Arc<str>>,
    /// BUILD_ID. `Unsupported` when missing.
    pub build: InvField<Arc<str>>,
    /// uname release.
    pub kernel: InvField<Arc<str>>,
    /// uname version.
    pub kernel_version: InvField<Arc<str>>,
    /// uname machine, for example "x86_64".
    pub architecture: InvField<Arc<str>>,
    /// Unix seconds.
    pub install_date_s: InvField<i64>,
    /// For example "pacman 7.0.0".
    pub package_manager: InvField<Arc<str>>,
    /// Shown on screen only. Never log it.
    pub machine_id: InvField<Arc<str>>,
}

/// Processor section (PERF-096).
#[derive(Clone, Debug, Default)]
pub struct ProcessorInfo {
    pub name: InvField<Arc<str>>,
    pub vendor: InvField<Arc<str>>,
    pub architecture: InvField<Arc<str>>,
    /// Shown as "Family 25 Model 97 Stepping 2".
    pub family: InvField<u32>,
    pub model: InvField<u32>,
    pub stepping: InvField<u32>,
    pub cores: InvField<u32>,
    pub logical: InvField<u32>,
    pub base_mhz: InvField<u32>,
    pub max_mhz: InvField<u32>,
    pub microcode: InvField<Arc<str>>,
    pub virtualization: InvField<Arc<str>>,
    pub avx512: InvField<bool>,
    /// 64 for x86_64, aarch64, riscv64, ppc64, ppc64le, s390x, loongarch64 and mips64, else 32
    /// (R-170 item 7).
    pub bits: InvField<u8>,
}

/// System section, static rows (PERF-097).
#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    pub pretty_hostname: InvField<Arc<str>>,
    pub static_hostname: InvField<Arc<str>>,
    pub user_name: InvField<Arc<str>>,
    /// `None` hides the row (empty or "(none)").
    pub domain: Option<Arc<str>>,
    /// Unix seconds, now minus CLOCK_BOOTTIME.
    pub boot_time_s: InvField<i64>,
    pub desktop: InvField<Arc<str>>,
    pub windowing_system: InvField<Arc<str>>,
    pub session_type: InvField<Arc<str>>,
    /// logind session Class, raw, for example user (R-205).
    pub session_class: InvField<Arc<str>>,
    pub virtual_terminal: InvField<Arc<str>>,
}

/// Memory section, static rows (PERF-098).
#[derive(Clone, Debug, Default)]
pub struct MemoryInfo {
    pub total_bytes: InvField<u64>,
    pub page_size: InvField<u64>,
    pub huge_page_size: InvField<u64>,
    pub swap_total_bytes: InvField<u64>,
    pub zram_bytes: InvField<u64>,
    /// Maximum Committed_AS seen since tasklens started. The reducer fills it; the inventory
    /// worker leaves it `WarmingUp` (R-166).
    pub peak_committed_bytes: InvField<u64>,
}

/// One memory module row (PERF-099). There is no serial field: DIMM serials are never read or
/// stored (D-007, R-030).
#[derive(Clone, Debug, Default)]
pub struct MemoryModule {
    pub slot: InvField<Arc<str>>,
    pub capacity_bytes: InvField<u64>,
    pub speed_mts: InvField<u32>,
    pub kind: InvField<Arc<str>>,
    pub form_factor: InvField<Arc<str>>,
    pub manufacturer: InvField<Arc<str>>,
    pub part_number: InvField<Arc<str>>,
    /// Configured clock speed, in MT/s.
    pub configured_speed_mts: InvField<u32>,
    pub rank: InvField<u32>,
    pub configured_voltage_mv: InvField<u32>,
}

/// One disk drive row (PERF-100).
#[derive(Clone, Debug, Default)]
pub struct DiskDrive {
    /// The worker sends `None`; the reducer sets it from `name` (R-166).
    pub device: Option<DeviceIdx>,
    /// The kernel name, for example "nvme0n1". The reducer matches it against the catalog's disk
    /// entries.
    pub name: Option<Arc<str>>,
    pub model: InvField<Arc<str>>,
    pub kind: InvField<Arc<str>>,
    /// NVMe, SATA, USB, SCSI or virtio.
    pub interface: InvField<Arc<str>>,
    pub capacity_bytes: InvField<u64>,
    pub free_bytes: InvField<u64>,
    /// Mount paths.
    pub volumes: Arc<[Arc<str>]>,
    /// Shown on screen only. Never log it.
    pub serial: InvField<Arc<str>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FirmwareType {
    Uefi,
    LegacyBios,
}

/// Firmware section (PERF-101).
#[derive(Clone, Debug, Default)]
pub struct FirmwareInfo {
    pub firmware_type: InvField<Option<FirmwareType>>,
    pub bios_vendor: InvField<Arc<str>>,
    pub bios_version: InvField<Arc<str>>,
    pub bios_date: InvField<Arc<str>>,
    pub bios_release: InvField<Arc<str>>,
    pub secure_boot: InvField<bool>,
}

/// Motherboard section (PERF-102).
#[derive(Clone, Debug, Default)]
pub struct BoardInfo {
    pub manufacturer: InvField<Arc<str>>,
    pub product: InvField<Arc<str>>,
    pub version: InvField<Arc<str>>,
    pub system_vendor: InvField<Arc<str>>,
    pub system_product: InvField<Arc<str>>,
    pub chassis_type: InvField<Arc<str>>,
    /// board_serial is mode 0400, so `Restricted` when unprivileged. Never log it.
    pub serial: InvField<Arc<str>>,
}

/// One display mode, shown as "3840x2160 @ 144 Hz (primary)".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub primary: bool,
}

/// One graphics adapter, static rows (PERF-103).
#[derive(Clone, Debug, Default)]
pub struct GraphicsAdapter {
    /// The worker sends `None`; the reducer sets it from `pci_slot` (R-166).
    pub device: Option<DeviceIdx>,
    /// The PCI bus id, lowercase, for example "0000:01:00.0". The reducer matches it against the
    /// catalog's GPU entries.
    pub pci_slot: Option<Arc<str>>,
    pub name: InvField<Arc<str>>,
    pub driver_version: InvField<Arc<str>>,
    pub kernel_module: InvField<Arc<str>>,
    pub vulkan_version: InvField<Arc<str>>,
    pub opengl_version: InvField<Arc<str>>,
    pub temperature_source: InvField<Arc<str>>,
    pub dedicated_bytes: InvField<u64>,
    pub shared_bytes: InvField<u64>,
    pub displays: Arc<[DisplayMode]>,
}

/// One network adapter (PERF-104).
#[derive(Clone, Debug, Default)]
pub struct NetworkAdapter {
    pub device: Option<DeviceIdx>,
    /// Connection name and type.
    pub connection: InvField<Arc<str>>,
    pub adapter_name: InvField<Arc<str>>,
    pub driver: InvField<Arc<str>>,
    pub link_speed_mbps: InvField<u32>,
    pub ip_addresses: Arc<[Arc<str>]>,
    /// Shown on screen only. Never log it.
    pub mac: InvField<Arc<str>>,
}

/// Public IP fetch state (PERF-060). Never saved to disk. The UI sends
/// `DeviceCommand::FetchPublicIp` and the Inventory worker runs the fetch (R-139 items 2 to 4).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PublicIpState {
    #[default]
    NotFetched,
    Fetching,
    /// Shown as "203.0.113.4 (fetched 14:02)".
    Fetched {
        v4: Option<Arc<str>>,
        v6: Option<Arc<str>>,
        fetched_at_s: i64,
    },
    Failed(Arc<str>),
}

/// The Information tab component (ADR-003): the static values of its ten sections. Live rows come
/// from `SystemSnapshot`, `SeriesSet` and `DeviceCatalog`. The Inventory worker fills it on page
/// open and on `RefreshNow`. A failed scan sets `header.validity` to `Failed`.
#[derive(Clone, Debug, Default)]
pub struct InventorySnapshot {
    pub header: ComponentHeader,
    pub os: OsInfo,
    pub processor: ProcessorInfo,
    pub system: SystemInfo,
    pub memory: MemoryInfo,
    pub modules: Arc<[MemoryModule]>,
    /// Memory slots on the board, occupied or not. `Unsupported` when the DMI data lacks it.
    pub slots_total: InvField<u32>,
    pub disks: Arc<[DiskDrive]>,
    pub firmware: FirmwareInfo,
    pub board: BoardInfo,
    pub graphics: Arc<[GraphicsAdapter]>,
    pub network: Arc<[NetworkAdapter]>,
    pub public_ip: PublicIpState,
}

// ---- Device details ----
//
// R-139 item 6. Worker-side records are the same structs without the `DeviceIdx`; the reducer sets
// the index.

/// One cache level (E12-010).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CacheLevelInfo {
    pub instances: u16,
    pub instance_kib: u64,
    pub total_kib: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CacheInfo {
    pub l1d: Option<CacheLevelInfo>,
    pub l1i: Option<CacheLevelInfo>,
    pub l2: Option<CacheLevelInfo>,
    pub l3: Option<CacheLevelInfo>,
    pub l4: Option<CacheLevelInfo>,
}

/// `Default` exists only so `InvField<VirtState>` has one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VirtState {
    EnabledKvm,
    SupportedNoKvm,
    #[default]
    NotSupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityFeature {
    SecureBoot,
    LockdownIntegrity,
    LockdownConfidentiality,
    MemoryEncryption,
    UserShadowStack,
    Iommu,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VulnSummary {
    pub mitigated: u16,
    pub vulnerable: u16,
    pub not_affected: u16,
    pub unknown: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaplState {
    NoZone,
    Unreadable,
    Readable,
}

/// CPU static rows (E12-010).
#[derive(Clone, Debug, Default)]
pub struct CpuStaticInfo {
    pub cores: u16,
    pub logical: u16,
    pub caches: CacheInfo,
    pub driver: InvField<Arc<str>>,
    pub governor: InvField<Arc<str>>,
    pub epp: InvField<Arc<str>>,
    pub platform_profile: InvField<Arc<str>>,
    pub prefcore_rank: Arc<[Option<u32>]>,
    pub prefcore_differs: bool,
    pub virtualization: InvField<VirtState>,
    pub running_in_vm: InvField<Arc<str>>,
    pub hypervisor: Option<Arc<str>>,
    pub security: SmallVec<[SecurityFeature; 6]>,
    pub security_unreadable: SmallVec<[Arc<str>; 4]>,
    pub vulns: Option<VulnSummary>,
    pub microcode: InvField<Arc<str>>,
    pub rapl: Option<RaplState>,
}

/// One zram device (E12-011).
#[derive(Clone, Debug, PartialEq)]
pub struct ZramDetail {
    pub name: Arc<str>,
    pub disksize: u64,
    pub algorithm: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ZswapInfo {
    pub enabled: bool,
    pub compressor: Option<Arc<str>>,
    pub max_pool_percent: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapKindModel {
    Partition,
    File,
    Zram,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwapDevice {
    pub name: Arc<str>,
    pub kind: SwapKindModel,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub priority: i32,
}

/// Memory details (E12-011).
#[derive(Clone, Debug, Default)]
pub struct MemoryDetails {
    pub zram: Arc<[ZramDetail]>,
    pub zswap: InvField<Option<ZswapInfo>>,
    pub swaps: InvField<Arc<[SwapDevice]>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SmartKind {
    Ata,
    Nvme,
    NeedsFallback,
    #[default]
    None,
}

/// One ATA SMART attribute. Texts as E12-021.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartAttr {
    pub id: u8,
    pub name: Arc<str>,
    pub value: i64,
    pub normalized: i32,
    pub threshold: i32,
    pub worst: i32,
    pub prefail: bool,
    pub online: bool,
    pub assessment: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct SmartData {
    pub updated_s: i64,
    pub health: InvField<Arc<str>>,
    pub power_on_s: InvField<u64>,
    pub selftest: InvField<Arc<str>>,
    pub ata: Arc<[SmartAttr]>,
    pub nvme: Arc<[(Arc<str>, Arc<str>)]>,
}

/// A process that blocks an eject. The reducer fills `app` from the process grouping (E12-022).
#[derive(Clone, Debug, PartialEq)]
pub struct Blocker {
    pub key: ProcessKey,
    pub name: Arc<str>,
    pub app: Option<Arc<str>>,
    pub paths: Arc<[Arc<str>]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EjectState {
    Running,
    Blocked {
        blockers: Arc<[Blocker]>,
        others_may_hold: bool,
    },
    Done,
    Failed(Arc<str>),
}

/// One PERF-050 Partitions table row. tl-collect `parse/partitions.rs` (E04-031) builds it
/// (R-140 item 4).
#[derive(Clone, Debug, PartialEq)]
pub struct PartitionRow {
    pub name: Arc<str>,
    pub start_bytes: u64,
    pub size_bytes: u64,
    pub holder: Option<Arc<str>>,
    pub mount: Option<Arc<str>>,
    pub extra_mounts: Arc<[Arc<str>]>,
    pub label: Option<Arc<str>>,
    pub fs_type: Option<Arc<str>>,
    pub usage_pct: InvField<f32>,
    pub flags: Arc<[Arc<str>]>,
    pub type_name: Arc<str>,
    pub read_only: bool,
    pub unallocated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartitionTable {
    /// "GPT", "MBR" or "none".
    pub kind: Arc<str>,
    /// Ordered by `start_bytes`.
    pub rows: Arc<[PartitionRow]>,
}

/// Disk details. `Default` lets a source fill only its own fields; the reducer merges by field.
#[derive(Clone, Debug, Default)]
pub struct DiskDetails {
    /// PERF-049 "Type" row: the E04-021 DiskKind name (sysfs source).
    pub kind: Arc<str>,
    /// sysfs removable flag (sysfs source).
    pub removable: bool,
    pub model: InvField<Arc<str>>,
    /// Shown on screen only. Never log it.
    pub wwn: InvField<Arc<str>>,
    /// Shown on screen only. Never log it.
    pub serial: InvField<Arc<str>>,
    pub interface: InvField<Arc<str>>,
    pub rotation_rpm: Option<u32>,
    /// Block scheduler name.
    pub scheduler: InvField<Arc<str>>,
    pub smart: SmartKind,
    /// `None` when never requested, `WarmingUp` while reading.
    pub smart_data: Option<InvField<Arc<SmartData>>>,
    pub smart_temp_c: Option<f32>,
    pub ejectable: bool,
    pub can_power_off: bool,
    /// The UI's eject sequence and state. `None` is idle.
    pub eject: Option<(u64, EjectState)>,
    /// R-140 item 4. `None` until the table is read.
    pub partitions: Option<Arc<PartitionTable>>,
    pub formatted_bytes: InvField<u64>,
    pub used_bytes: InvField<u64>,
    pub free_bytes: InvField<u64>,
    pub system_disk: InvField<bool>,
    pub swap: InvField<bool>,
}

/// Wi-Fi link (R-139 field names). Values NetworkManager or nl80211 may not give are `Option`:
/// missing is never zero.
#[derive(Clone, Debug, PartialEq)]
pub struct WifiInfo {
    pub ssid: Arc<str>,
    /// nl80211 only.
    pub signal_dbm: Option<i32>,
    pub signal_pct: u8,
    /// nl80211 only.
    pub rx_mbps: Option<f32>,
    pub tx_mbps: Option<f32>,
    pub freq_mhz: u32,
    pub channel: Option<u16>,
    pub security: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct NetDetails {
    /// PERF-054 "Type": Ethernet, Wi-Fi, Virtual, USB network, Bridge, Cellular, Tunnel, VPN or
    /// Other.
    pub type_label: Arc<str>,
    /// NetworkManager active connection Id, shown under the adapter name (PERF-054).
    pub connection: Option<Arc<str>>,
    pub adapter_name: InvField<Arc<str>>,
    pub driver: InvField<Arc<str>>,
    /// Shown on screen only. Never log it.
    pub mac: InvField<Arc<str>>,
    pub ifindex: u32,
    pub link_speed_mbps: InvField<u32>,
    pub state: LinkState,
    pub addresses: Arc<[Arc<str>]>,
    pub kind: NetKind,
    pub wifi: Option<WifiInfo>,
}

/// Battery details (E12-023, history from E12-029).
#[derive(Clone, Debug)]
pub struct BatteryDetails {
    pub kind: Arc<str>,
    pub vendor: InvField<Arc<str>>,
    pub model: InvField<Arc<str>>,
    /// Shown on screen only. Never log it.
    pub serial: InvField<Arc<str>>,
    pub native_path: Arc<str>,
    pub percentage: InvField<f32>,
    pub energy_wh: InvField<f32>,
    pub energy_rate_w: InvField<f32>,
    pub voltage_v: InvField<f32>,
    /// "Charging", "Discharging", "Empty", "Fully charged", "Pending charge",
    /// "Pending discharge" or "Unknown".
    pub state: Arc<str>,
    pub time_to_full_s: InvField<i64>,
    pub time_to_empty_s: InvField<i64>,
    pub charge_cycles: InvField<u32>,
    pub technology: Arc<str>,
    pub capacity_pct: InvField<f32>,
    pub energy_empty_wh: InvField<f32>,
    pub energy_full_wh: InvField<f32>,
    pub energy_full_design_wh: InvField<f32>,
    pub voltage_min_design_v: InvField<f32>,
    pub voltage_max_design_v: InvField<f32>,
    pub power_supply: bool,
    /// (enabled, start percent, end percent).
    pub charge_threshold: Option<(bool, u8, u8)>,
    pub temperature_c: InvField<f32>,
    /// (unix seconds, percent) pairs (E12-029).
    pub history: Option<Arc<[(i64, f32)]>>,
}

/// The device details component (R-139 item 6; PERF-036 to PERF-046, PERF-050 to PERF-052,
/// PERF-078).
#[derive(Clone, Debug, Default)]
pub struct DeviceDetailsSnapshot {
    pub header: ComponentHeader,
    pub cpu: Arc<CpuStaticInfo>,
    pub memory: Arc<MemoryDetails>,
    /// Sorted by `DeviceIdx`.
    pub disks: Arc<[(DeviceIdx, DiskDetails)]>,
    pub nets: Arc<[(DeviceIdx, NetDetails)]>,
    pub batteries: Arc<[(DeviceIdx, BatteryDetails)]>,
    /// The system runs on battery (UPower `OnBattery`, or sysfs: a battery and no mains supply
    /// online). `None` without a battery or before the first reading (wlf.32).
    pub on_battery: Option<bool>,
}

// ---- The snapshot ----

/// One published state of the engine (ADR-003). Unchanged components reuse their `Arc` across
/// snapshots, and the UI hub compares versions to build the `ComponentMask` (ADR-005).
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Strictly increasing per engine process.
    pub sequence: u64,
    /// Engine session number (ADR-004).
    pub epoch: u32,
    pub boot_id: BootId,
    pub published_ns: TimeNs,
    pub system: Arc<SystemSnapshot>,
    pub series: Arc<SeriesSet>,
    pub processes: Arc<ProcessSnapshot>,
    pub table: Arc<ProcessTableView>,
    pub connections: Arc<ConnectionsSnapshot>,
    pub services: Arc<ServicesSnapshot>,
    pub startup: Arc<StartupSnapshot>,
    pub inventory: Arc<InventorySnapshot>,
    pub devices: Arc<DeviceCatalog>,
    pub health: Arc<HealthSnapshot>,
    /// P2.
    pub windows: Arc<WindowsSnapshot>,
    /// P2.
    pub details: Arc<DetailsSnapshot>,
    /// P3.
    pub history: Arc<HistoryView>,
    /// P2, mask bit `PROCESS_SERIES` (R-154).
    pub process_series: Arc<ProcessSeriesSet>,
    /// P1 and P2, mask bit `DEVICE_DETAILS` (R-139).
    pub device_details: Arc<DeviceDetailsSnapshot>,
}

impl Snapshot {
    /// Sequence 0, the given epoch and boot id, `published_ns` 0, and every component at its
    /// default: headers `WarmingUp`, empty lists, versions 0. The windows header is `WarmingUp`
    /// too; the KWin worker sets `Unsupported` when the bridge is not available. The engine
    /// publishes this before its first data, and the fake engine uses it in UI tests.
    pub fn empty(boot_id: BootId, epoch: u32) -> Snapshot {
        Snapshot {
            sequence: 0,
            epoch,
            boot_id,
            published_ns: 0,
            system: Arc::default(),
            series: Arc::default(),
            processes: Arc::default(),
            table: Arc::default(),
            connections: Arc::default(),
            services: Arc::default(),
            startup: Arc::default(),
            inventory: Arc::default(),
            devices: Arc::default(),
            // HealthSnapshot has no Default: version 0, every source Off.
            health: Arc::new(HealthSnapshot {
                version: 0,
                sources: std::array::from_fn(|_| SourceHealth::default()),
                helper: HelperState::default(),
                helper_start: None,
                caps: EngineCaps::default(),
                setup: Arc::default(),
            }),
            windows: Arc::default(),
            details: Arc::default(),
            history: Arc::default(),
            process_series: Arc::default(),
            device_details: Arc::default(),
        }
    }

    /// The components whose version differs from `prev`: `header.version` for each component,
    /// `DeviceCatalog::version` for `DEVICES` and `HealthSnapshot::version` for `HEALTH`.
    ///
    /// Clears `changed_series`, then fills it with the ids whose `SeriesView::version` differs or
    /// which are new, in `SeriesId` order (`series` first, then `slow`). `SERIES` is set when the
    /// set header changed or any series changed, appeared or went away.
    ///
    /// Compares versions only, never content. Walks each sorted series list in one merge pass and
    /// does not allocate while at most 64 series changed.
    pub fn changed_since(
        &self,
        prev: &Snapshot,
        changed_series: &mut SmallVec<[SeriesId; 64]>,
    ) -> ComponentMask {
        changed_series.clear();
        let versions = [
            (
                ComponentMask::SYSTEM,
                self.system.header.version,
                prev.system.header.version,
            ),
            (
                ComponentMask::SERIES,
                self.series.header.version,
                prev.series.header.version,
            ),
            (
                ComponentMask::PROCESSES,
                self.processes.header.version,
                prev.processes.header.version,
            ),
            (
                ComponentMask::TABLE,
                self.table.header.version,
                prev.table.header.version,
            ),
            (
                ComponentMask::CONNECTIONS,
                self.connections.header.version,
                prev.connections.header.version,
            ),
            (
                ComponentMask::SERVICES,
                self.services.header.version,
                prev.services.header.version,
            ),
            (
                ComponentMask::STARTUP,
                self.startup.header.version,
                prev.startup.header.version,
            ),
            (
                ComponentMask::INVENTORY,
                self.inventory.header.version,
                prev.inventory.header.version,
            ),
            (
                ComponentMask::DEVICES,
                self.devices.version,
                prev.devices.version,
            ),
            (
                ComponentMask::HEALTH,
                self.health.version,
                prev.health.version,
            ),
            (
                ComponentMask::WINDOWS,
                self.windows.header.version,
                prev.windows.header.version,
            ),
            (
                ComponentMask::DETAILS,
                self.details.header.version,
                prev.details.header.version,
            ),
            (
                ComponentMask::HISTORY,
                self.history.header.version,
                prev.history.header.version,
            ),
            (
                ComponentMask::PROCESS_SERIES,
                self.process_series.header.version,
                prev.process_series.header.version,
            ),
            (
                ComponentMask::DEVICE_DETAILS,
                self.device_details.header.version,
                prev.device_details.header.version,
            ),
        ];
        let mut mask = ComponentMask::NONE;
        for (bit, now, before) in versions {
            if now != before {
                mask |= bit;
            }
        }
        if !Arc::ptr_eq(&self.series, &prev.series) {
            let fast = diff_series(&self.series.series, &prev.series.series, changed_series);
            let slow = diff_series(&self.series.slow, &prev.series.slow, changed_series);
            if fast || slow {
                mask |= ComponentMask::SERIES;
            }
        }
        mask
    }
}

/// One merge pass over two lists sorted by `SeriesId`. Pushes the ids in `now` that are new or
/// have a different version. Returns true when anything changed, removed series included.
fn diff_series(
    now: &[Arc<SeriesView>],
    before: &[Arc<SeriesView>],
    changed: &mut SmallVec<[SeriesId; 64]>,
) -> bool {
    let mut any = false;
    let mut j = 0;
    for view in now {
        // Ids only in `before` were removed.
        while j < before.len() && before[j].id < view.id {
            any = true;
            j += 1;
        }
        match before.get(j) {
            Some(old) if old.id == view.id => {
                if old.version != view.version {
                    changed.push(view.id);
                    any = true;
                }
                j += 1;
            }
            _ => {
                changed.push(view.id);
                any = true;
            }
        }
    }
    any || j < before.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ColumnId, GraphTarget, GroupKey, MetricId};
    use crate::process::{ProcFlags, ProcState, ProcessMeta};
    use crate::validity::FieldValidity;

    const BITS: [ComponentMask; 15] = [
        ComponentMask::SYSTEM,
        ComponentMask::SERIES,
        ComponentMask::PROCESSES,
        ComponentMask::TABLE,
        ComponentMask::CONNECTIONS,
        ComponentMask::SERVICES,
        ComponentMask::STARTUP,
        ComponentMask::INVENTORY,
        ComponentMask::DEVICES,
        ComponentMask::HEALTH,
        ComponentMask::WINDOWS,
        ComponentMask::DETAILS,
        ComponentMask::HISTORY,
        ComponentMask::PROCESS_SERIES,
        ComponentMask::DEVICE_DETAILS,
    ];

    fn view(id: SeriesId) -> Arc<SeriesView> {
        Arc::new(SeriesView {
            id,
            version: 1,
            period_ns: 1_000_000_000,
            samples: Arc::from([]),
        })
    }

    fn sid(device: u16, metric: MetricId) -> SeriesId {
        SeriesId {
            device: DeviceIdx(device),
            metric,
        }
    }

    fn key(start_ticks: u64, pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BootId(1),
            pid,
        }
    }

    fn record(key: ProcessKey) -> ProcessRecord {
        let text: Arc<str> = Arc::from("x");
        ProcessRecord {
            key,
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: text.clone(),
                name: text.clone(),
                exe: None,
                cmdline: None,
                user: text.clone(),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: text,
            }),
            group: GroupKey(0),
            state: ProcState::Running,
            flags: ProcFlags::default(),
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
        }
    }

    fn window(pid: u32, internal_id: &str) -> WindowEntry {
        WindowEntry {
            internal_id: Arc::from(internal_id),
            pid,
            caption: Arc::from("caption"),
            resource_class: Arc::from("class"),
            desktop_file_name: Arc::from("app"),
            minimized: false,
            maximized: false,
            keep_above: false,
            frame_geometry: FrameGeometry::default(),
            window_type: 0,
        }
    }

    #[test]
    fn mask_bits_distinct_and_all() {
        let mut all = ComponentMask::NONE;
        for (i, a) in BITS.iter().enumerate() {
            assert_eq!(a.0.count_ones(), 1);
            for b in &BITS[i + 1..] {
                assert!(!a.intersects(*b));
            }
            all |= *a;
        }
        assert_eq!(all, ComponentMask::ALL);
        assert_eq!(ComponentMask::ALL.0, 0x7FFF);
    }

    #[test]
    fn mask_contains_intersects() {
        let m = ComponentMask::SYSTEM | ComponentMask::SERIES;
        assert!(m.contains(ComponentMask::SERIES));
        assert!(!m.contains(ComponentMask::TABLE));
        assert!(m.intersects(ComponentMask::SERIES | ComponentMask::TABLE));
        assert!(!m.intersects(ComponentMask::TABLE));
        assert!(m.contains(ComponentMask::NONE));
        assert!(ComponentMask::ALL.contains(m));
    }

    #[test]
    fn series_set_get_binary_search() {
        let ids = [
            sid(0, MetricId::CPU_TOTAL),
            sid(1, MetricId::CPU_TOTAL),
            sid(2, MetricId::CPU_TOTAL),
        ];
        let set = SeriesSet {
            series: ids.iter().map(|&id| view(id)).collect(),
            ..SeriesSet::default()
        };
        assert_eq!(set.get(ids[1]).map(|v| v.id), Some(ids[1]));
        assert!(set.get(sid(3, MetricId::CPU_TOTAL)).is_none());
    }

    #[test]
    fn series_set_slow_tier_get() {
        let fast = sid(0, MetricId::CPU_TOTAL);
        let slow = sid(0, MetricId::CPU_TOTAL.with_tier_1hz());
        let set = SeriesSet {
            series: Arc::from([view(fast)]),
            slow: Arc::from([view(slow)]),
            ..SeriesSet::default()
        };
        assert!(Arc::ptr_eq(set.get(slow).unwrap(), &set.slow[0]));
        assert!(Arc::ptr_eq(set.get(fast).unwrap(), &set.series[0]));
    }

    #[test]
    fn process_series_set_get() {
        let a = ProcessSeriesId {
            target: GraphTarget::Process(key(1, 10)),
            column: ColumnId::Cpu,
        };
        let b = ProcessSeriesId {
            target: GraphTarget::Process(key(1, 10)),
            column: ColumnId::Memory,
        };
        let absent = ProcessSeriesId {
            target: GraphTarget::Name(Arc::from("firefox")),
            column: ColumnId::Cpu,
        };
        let mut series = vec![(b.clone(), view(sid(1, MetricId::CPU_TOTAL)))];
        series.push((a.clone(), view(sid(0, MetricId::CPU_TOTAL))));
        series.sort_by(|x, y| x.0.cmp(&y.0));
        let slow = vec![
            (a.clone(), view(sid(2, MetricId::CPU_TOTAL))),
            (b.clone(), view(sid(3, MetricId::CPU_TOTAL))),
        ];
        let set = ProcessSeriesSet {
            header: ComponentHeader::default(),
            series: Arc::from(series),
            slow: Arc::from(slow),
        };
        assert_eq!(set.get(&b).map(|v| v.id), Some(sid(1, MetricId::CPU_TOTAL)));
        assert_eq!(
            set.get_slow(&b).map(|v| v.id),
            Some(sid(3, MetricId::CPU_TOTAL))
        );
        assert_eq!(
            set.get_slow(&a).map(|v| v.id),
            Some(sid(2, MetricId::CPU_TOTAL))
        );
        assert!(set.get(&absent).is_none());
        assert!(set.get_slow(&absent).is_none());
    }

    #[test]
    fn process_snapshot_get() {
        let snap = ProcessSnapshot {
            records: Arc::from([record(key(10, 5)), record(key(20, 3)), record(key(30, 9))]),
            ..ProcessSnapshot::default()
        };
        assert_eq!(snap.get(key(20, 3)).map(|r| r.key), Some(key(20, 3)));
        assert!(snap.get(key(25, 4)).is_none());
    }

    #[test]
    fn process_snapshot_default_extras_empty() {
        let snap = ProcessSnapshot::default();
        assert!(snap.extras.is_empty());
        assert!(snap.apps.is_empty());
        assert!(snap.extras_p2.is_empty());
        assert!(snap.low_memory.is_none());
    }

    fn extras(start_ticks: u64) -> ProcessExtras {
        ProcessExtras {
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            // 100 ticks per second, so one tick is 10_000_000 ns.
            start_boot_ns: start_ticks * 10_000_000,
            rss_peak_bytes: 0,
            rss_shared_bytes: 0,
            commit_bytes: 0,
            vsize_bytes: 0,
            vsize_peak_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            policy: crate::actions::SchedPolicy::Other,
            rt_prio: 0,
            affinity: crate::system::CpuSet::empty(),
            handles: None,
            exe_name: Arc::from("x"),
            exe_path: Arc::from("/usr/bin/x"),
            exe_deleted: false,
            unit: None,
            state_letter: b'R',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
        }
    }

    #[test]
    fn extras_index_aligned() {
        let keys = [key(10, 5), key(20, 3), key(30, 9)];
        let snap = ProcessSnapshot {
            records: keys.iter().map(|&k| record(k)).collect(),
            extras: keys.iter().map(|k| extras(k.start_ticks)).collect(),
            ..ProcessSnapshot::default()
        };
        assert_eq!(snap.records.len(), 3);
        assert_eq!(snap.extras.len(), snap.records.len());
        for (r, e) in snap.records.iter().zip(snap.extras.iter()) {
            assert_eq!(e.start_boot_ns, r.key.start_ticks * 10_000_000);
        }
    }

    #[test]
    fn windows_for_pid_range() {
        let snap = WindowsSnapshot {
            windows: Arc::from([window(10, "{a}"), window(10, "{b}"), window(12, "{c}")]),
            ..WindowsSnapshot::default()
        };
        let ten = snap.for_pid(10);
        assert_eq!(ten.len(), 2);
        assert!(ten.iter().all(|w| w.pid == 10));
        assert!(snap.for_pid(11).is_empty());
        assert_eq!(snap.for_pid(12).len(), 1);
    }

    #[test]
    fn windows_pick_default_none() {
        let snap = WindowsSnapshot::default();
        assert!(snap.pick.is_none());
        assert!(snap.windows.is_empty());
    }

    #[test]
    fn header_default_warming_up() {
        let h = ComponentHeader::default();
        assert_eq!(h.version, 0);
        assert_eq!(h.validity, Validity::WarmingUp);
    }

    #[test]
    fn service_status_mapping_sys003() {
        let cases = [
            (("active", "running"), ServiceStatus::Running),
            (("active", "exited"), ServiceStatus::Active),
            (("activating", "start"), ServiceStatus::Starting),
            (("deactivating", "stop"), ServiceStatus::Stopping),
            (("reloading", ""), ServiceStatus::Reloading),
            (("refreshing", ""), ServiceStatus::Reloading),
            (("inactive", "dead"), ServiceStatus::Stopped),
            (("failed", "failed"), ServiceStatus::Failed),
            (("maintenance", ""), ServiceStatus::Maintenance),
            (("bogus", "running"), ServiceStatus::Stopped),
        ];
        for ((active, sub), want) in cases {
            assert_eq!(
                ServiceStatus::from_states(active, sub),
                want,
                "{active}+{sub}"
            );
        }
        for sub in ["listening", "mounted", "waiting", "elapsed", "plugged"] {
            assert_eq!(
                ServiceStatus::from_states("active", sub),
                ServiceStatus::Active
            );
        }
    }

    #[test]
    fn startup_type_from_unit_file_state_all() {
        let cases = [
            ("enabled-runtime", StartupType::EnabledRuntime),
            ("masked-runtime", StartupType::Masked),
            ("linked-runtime", StartupType::Linked),
            ("", StartupType::NoUnitFile),
            ("enabled", StartupType::Enabled),
            ("disabled", StartupType::Disabled),
            ("static", StartupType::Static),
            ("indirect", StartupType::Indirect),
            ("masked", StartupType::Masked),
            ("generated", StartupType::Generated),
            ("transient", StartupType::Transient),
            ("alias", StartupType::Alias),
            ("linked", StartupType::Linked),
        ];
        for (word, want) in cases {
            assert_eq!(StartupType::from_unit_file_state(word), want, "{word:?}");
        }
    }

    #[test]
    fn services_snapshot_default_empty() {
        let snap = ServicesSnapshot::default();
        assert!(snap.units.is_empty());
        assert!(snap.rows.is_empty());
        assert!(snap.details.is_empty());
        assert_eq!(snap.backend, ServiceBackend::Systemd);
        assert_eq!(snap.query_generation, 0);
        assert_eq!(snap.counts, ServiceCounts::default());
        assert_eq!(snap.managers, [ManagerState::Unavailable; 2]);
        assert_eq!(snap.manager_counts, [0; 2]);
    }

    #[test]
    fn impact_thresholds_sys021() {
        let cases = [
            (1000, Impact::High),
            (999, Impact::Medium),
            (300, Impact::Medium),
            (299, Impact::Low),
            (0, Impact::Low),
        ];
        for (ms, want) in cases {
            assert_eq!(Impact::from_activation_ms(ms), want, "{ms} ms");
        }
    }

    #[test]
    fn conn_state_from_kernel_numbers() {
        let want = [
            ConnState::Established,
            ConnState::SynSent,
            ConnState::SynReceived,
            ConnState::FinWait1,
            ConnState::FinWait2,
            ConnState::TimeWait,
            ConnState::Closed,
            ConnState::CloseWait,
            ConnState::LastAck,
            ConnState::Listening,
            ConnState::Closing,
            ConnState::SynReceived,
        ];
        for (n, state) in (1u8..).zip(want) {
            assert_eq!(ConnState::from_tcp_state(n), state, "state {n}");
        }
        assert_eq!(ConnState::from_tcp_state(0), ConnState::Unknown);
        assert_eq!(ConnState::from_tcp_state(13), ConnState::Unknown);
    }

    #[test]
    fn conn_state_labels_sys029() {
        let cases = [
            (ConnState::Listening, "LISTENING"),
            (ConnState::SynSent, "SYN-SENT"),
            (ConnState::SynReceived, "SYN-RECEIVED"),
            (ConnState::Established, "ESTABLISHED"),
            (ConnState::FinWait1, "FIN-WAIT-1"),
            (ConnState::FinWait2, "FIN-WAIT-2"),
            (ConnState::CloseWait, "CLOSE-WAIT"),
            (ConnState::Closing, "CLOSING"),
            (ConnState::LastAck, "LAST-ACK"),
            (ConnState::TimeWait, "TIME-WAIT"),
            (ConnState::Closed, "CLOSED"),
            (ConnState::Unknown, "UNKNOWN"),
            (ConnState::Unconnected, "UNCONNECTED"),
            (ConnState::Connected, "CONNECTED"),
        ];
        for (state, label) in cases {
            assert_eq!(state.label(), label);
        }
    }

    #[test]
    fn startup_labels_sys020() {
        let kinds = [
            (StartupKind::AutostartUser, "Autostart (user)"),
            (StartupKind::AutostartSystem, "Autostart (system)"),
            (
                StartupKind::AutostartSystemOverridden,
                "Autostart (system, overridden)",
            ),
            (StartupKind::UserUnit, "systemd user unit"),
            (StartupKind::PlasmaPreStartup, "Plasma pre-startup script"),
            (StartupKind::PlasmaLogout, "Plasma logout script"),
        ];
        for (kind, label) in kinds {
            assert_eq!(kind.label(), label);
        }
        let sources = [
            (StartupKind::AutostartUser, "XDG autostart"),
            (StartupKind::AutostartSystem, "XDG autostart"),
            (StartupKind::AutostartSystemOverridden, "XDG autostart"),
            (StartupKind::UserUnit, "systemd user unit"),
            (StartupKind::PlasmaPreStartup, "Plasma pre-startup script"),
            (StartupKind::PlasmaLogout, "Plasma logout script"),
        ];
        for (kind, word) in sources {
            assert_eq!(kind.source_word(), word);
        }
        assert_eq!(StartupScope::User.label(), "user");
        assert_eq!(StartupScope::System.label(), "system");
        let statuses = [
            (StartupStatus::Enabled, "Enabled"),
            (StartupStatus::Disabled, "Disabled"),
            (StartupStatus::Masked, "Masked"),
            (StartupStatus::NotInThisDesktop, "Not in this desktop"),
            (StartupStatus::MissingProgram, "Missing program"),
        ];
        for (status, label) in statuses {
            assert_eq!(status.label(), label);
        }
    }

    #[test]
    fn impact_sort_order() {
        let mut v = vec![
            Impact::NotMeasured,
            Impact::Low,
            Impact::None,
            Impact::High,
            Impact::Medium,
        ];
        v.sort();
        assert_eq!(
            v,
            [
                Impact::High,
                Impact::Medium,
                Impact::Low,
                Impact::None,
                Impact::NotMeasured,
            ]
        );
    }

    #[test]
    fn inventory_default_warming_up() {
        let inv = InventorySnapshot::default();
        let w = Validity::WarmingUp;
        assert_eq!(inv.header.validity, w);
        let os = &inv.os;
        for v in [
            os.name.validity,
            os.version.validity,
            os.build.validity,
            os.kernel.validity,
            os.kernel_version.validity,
            os.architecture.validity,
            os.install_date_s.validity,
            os.package_manager.validity,
            os.machine_id.validity,
        ] {
            assert_eq!(v, w);
        }
        assert!(os.logo_icon.is_none());
        let p = &inv.processor;
        for v in [
            p.name.validity,
            p.vendor.validity,
            p.architecture.validity,
            p.family.validity,
            p.model.validity,
            p.stepping.validity,
            p.cores.validity,
            p.logical.validity,
            p.base_mhz.validity,
            p.max_mhz.validity,
            p.microcode.validity,
            p.virtualization.validity,
            p.avx512.validity,
            p.bits.validity,
        ] {
            assert_eq!(v, w);
        }
        let s = &inv.system;
        for v in [
            s.pretty_hostname.validity,
            s.static_hostname.validity,
            s.user_name.validity,
            s.boot_time_s.validity,
            s.desktop.validity,
            s.windowing_system.validity,
            s.session_type.validity,
            s.session_class.validity,
            s.virtual_terminal.validity,
        ] {
            assert_eq!(v, w);
        }
        assert!(s.domain.is_none());
        let m = &inv.memory;
        for v in [
            m.total_bytes.validity,
            m.page_size.validity,
            m.huge_page_size.validity,
            m.swap_total_bytes.validity,
            m.zram_bytes.validity,
            m.peak_committed_bytes.validity,
        ] {
            assert_eq!(v, w);
        }
        let f = &inv.firmware;
        for v in [
            f.firmware_type.validity,
            f.bios_vendor.validity,
            f.bios_version.validity,
            f.bios_date.validity,
            f.bios_release.validity,
            f.secure_boot.validity,
        ] {
            assert_eq!(v, w);
        }
        let b = &inv.board;
        for v in [
            b.manufacturer.validity,
            b.product.validity,
            b.version.validity,
            b.system_vendor.validity,
            b.system_product.validity,
            b.chassis_type.validity,
            b.serial.validity,
        ] {
            assert_eq!(v, w);
        }
        assert!(inv.modules.is_empty());
        assert!(inv.disks.is_empty());
        assert!(inv.graphics.is_empty());
        assert!(inv.network.is_empty());
        assert_eq!(inv.public_ip, PublicIpState::NotFetched);

        // List element defaults also start as WarmingUp.
        let d = DiskDrive::default();
        assert_eq!(d.serial.validity, w);
        assert!(d.device.is_none() && d.volumes.is_empty());
        let g = GraphicsAdapter::default();
        assert_eq!(g.dedicated_bytes.validity, w);
        assert!(g.displays.is_empty());
        let n = NetworkAdapter::default();
        assert_eq!(n.mac.validity, w);
        assert!(n.ip_addresses.is_empty());
    }

    #[test]
    fn device_details_default_warming_up() {
        let dd = DeviceDetailsSnapshot::default();
        assert_eq!(dd.header.validity, Validity::WarmingUp);
        assert!(dd.disks.is_empty());
        assert!(dd.nets.is_empty());
        assert!(dd.batteries.is_empty());
        assert_eq!(dd.cpu.virtualization.validity, Validity::WarmingUp);
        assert_eq!(dd.cpu.virtualization.value, VirtState::NotSupported);
        assert_eq!(dd.memory.swaps.validity, Validity::WarmingUp);
        assert!(dd.memory.zram.is_empty());
    }

    #[test]
    fn disk_details_eject_sequence() {
        let idle = DiskDetails::default();
        assert!(idle.eject.is_none());
        assert_eq!(idle.smart, SmartKind::None);
        assert!(idle.smart_data.is_none() && idle.partitions.is_none());
        let done = DiskDetails {
            eject: Some((7, EjectState::Done)),
            ..DiskDetails::default()
        };
        assert_eq!(done.eject, Some((7, EjectState::Done)));
    }

    #[test]
    fn memory_module_has_no_serial() {
        // Exhaustive pattern: adding a field breaks the build and forces a D-007 review.
        let m = MemoryModule::default();
        let MemoryModule {
            slot,
            capacity_bytes,
            speed_mts,
            kind,
            form_factor,
            manufacturer,
            part_number,
            configured_speed_mts,
            rank,
            configured_voltage_mv,
        } = m;
        for v in [
            slot.validity,
            capacity_bytes.validity,
            speed_mts.validity,
            kind.validity,
            form_factor.validity,
            manufacturer.validity,
            part_number.validity,
            configured_speed_mts.validity,
            rank.validity,
            configured_voltage_mv.validity,
        ] {
            assert_eq!(v, Validity::WarmingUp);
        }
    }

    fn view_at(id: SeriesId, version: u64) -> Arc<SeriesView> {
        Arc::new(SeriesView {
            id,
            version,
            period_ns: 1_000_000_000,
            samples: Arc::from([]),
        })
    }

    fn with_series(base: &Snapshot, views: Vec<Arc<SeriesView>>) -> Snapshot {
        let mut snap = base.clone();
        snap.series = Arc::new(SeriesSet {
            header: base.series.header,
            series: Arc::from(views),
            slow: Arc::from([]),
        });
        snap
    }

    fn three_ids() -> [SeriesId; 3] {
        [
            sid(0, MetricId::CPU_TOTAL),
            sid(0, MetricId::MEM_USED),
            sid(1, MetricId::NET_RX),
        ]
    }

    #[test]
    fn snapshot_empty_defaults() {
        let s = Snapshot::empty(BootId(7), 3);
        assert_eq!(s.sequence, 0);
        assert_eq!(s.epoch, 3);
        assert_eq!(s.boot_id, BootId(7));
        assert_eq!(s.published_ns, 0);
        let headers = [
            s.system.header,
            s.series.header,
            s.processes.header,
            s.table.header,
            s.connections.header,
            s.services.header,
            s.startup.header,
            s.inventory.header,
            s.windows.header,
            s.details.header,
            s.history.header,
            s.process_series.header,
            s.device_details.header,
        ];
        for h in headers {
            assert_eq!(h.validity, Validity::WarmingUp);
            assert_eq!(h.version, 0);
        }
        assert_eq!(s.devices.version, 0);
        assert!(s.devices.entries.is_empty());
        assert_eq!(s.health.version, 0);
        assert!(s.series.series.is_empty());
        assert!(s.processes.records.is_empty());
        assert!(s.table.rows.is_empty());
    }

    #[test]
    fn changed_since_none_when_shared() {
        let prev = with_series(
            &Snapshot::empty(BootId(1), 1),
            three_ids().map(|id| view_at(id, 4)).to_vec(),
        );
        let next = prev.clone();
        let mut changed = SmallVec::new();
        changed.push(sid(9, MetricId::GPU_UTIL));
        assert_eq!(next.changed_since(&prev, &mut changed), ComponentMask::NONE);
        assert!(changed.is_empty());
    }

    #[test]
    fn changed_since_table_only() {
        let prev = Snapshot::empty(BootId(1), 1);
        let mut next = prev.clone();
        Arc::make_mut(&mut next.table).header.version += 1;
        let mut changed = SmallVec::new();
        assert_eq!(
            next.changed_since(&prev, &mut changed),
            ComponentMask::TABLE
        );
        assert!(changed.is_empty());
    }

    #[test]
    fn changed_since_one_series() {
        let ids = three_ids();
        let base = Snapshot::empty(BootId(1), 1);
        let prev = with_series(&base, ids.map(|id| view_at(id, 4)).to_vec());
        // The other two views keep their Arc, like the engine does.
        let mut views = prev.series.series.to_vec();
        views[1] = view_at(ids[1], 5);
        let next = with_series(&prev, views);
        let mut changed = SmallVec::new();
        assert_eq!(
            next.changed_since(&prev, &mut changed),
            ComponentMask::SERIES
        );
        assert_eq!(changed.as_slice(), &[ids[1]]);
        assert!(!changed.spilled());
    }

    #[test]
    fn changed_since_new_series() {
        let ids = three_ids();
        let base = Snapshot::empty(BootId(1), 1);
        let prev = with_series(&base, vec![view_at(ids[0], 4), view_at(ids[2], 4)]);
        let next = with_series(&prev, ids.map(|id| view_at(id, 4)).to_vec());
        let mut changed = SmallVec::new();
        let mask = next.changed_since(&prev, &mut changed);
        assert!(mask.contains(ComponentMask::SERIES));
        assert_eq!(changed.as_slice(), &[ids[1]]);
    }

    #[test]
    fn changed_since_removed_series() {
        let ids = three_ids();
        let base = Snapshot::empty(BootId(1), 1);
        let prev = with_series(&base, ids.map(|id| view_at(id, 4)).to_vec());
        let next = with_series(&prev, vec![view_at(ids[0], 4)]);
        let mut changed = SmallVec::new();
        assert_eq!(
            next.changed_since(&prev, &mut changed),
            ComponentMask::SERIES
        );
        assert!(changed.is_empty());
    }

    #[test]
    fn changed_since_devices() {
        let prev = Snapshot::empty(BootId(1), 1);
        let mut next = prev.clone();
        Arc::make_mut(&mut next.devices).version += 1;
        let mut changed = SmallVec::new();
        assert!(
            next.changed_since(&prev, &mut changed)
                .contains(ComponentMask::DEVICES)
        );
    }

    #[test]
    fn changed_since_health() {
        let prev = Snapshot::empty(BootId(1), 1);
        let mut next = prev.clone();
        Arc::make_mut(&mut next.health).version += 1;
        let mut changed = SmallVec::new();
        assert_eq!(
            next.changed_since(&prev, &mut changed),
            ComponentMask::HEALTH
        );
    }

    #[test]
    fn changed_since_process_series() {
        let prev = Snapshot::empty(BootId(1), 1);
        let mut next = prev.clone();
        Arc::make_mut(&mut next.process_series).header.version += 1;
        let mut changed = SmallVec::new();
        assert_eq!(
            next.changed_since(&prev, &mut changed),
            ComponentMask::PROCESS_SERIES
        );
    }

    #[test]
    fn changed_since_device_details() {
        let prev = Snapshot::empty(BootId(1), 1);
        let mut next = prev.clone();
        Arc::make_mut(&mut next.device_details).header.version += 1;
        let mut changed = SmallVec::new();
        assert_eq!(
            next.changed_since(&prev, &mut changed),
            ComponentMask::DEVICE_DETAILS
        );
    }
}
