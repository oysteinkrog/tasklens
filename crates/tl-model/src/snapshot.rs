//! The published snapshot and its components (ADR-003).

use std::collections::HashMap;
use std::ops::{BitOr, BitOrAssign};
use std::sync::Arc;

use crate::health::ManagerState;
use crate::ids::{DeviceIdx, ProcessKey, ProcessSeriesId, SeriesId};
use crate::process::{Coverage, ProcessRecord};
use crate::series::SeriesView;
use crate::system::{
    CpuTopology, CpuTotals, DiskTotals, GpuTotals, MemoryTotals, NetTotals, SensorReading,
};
use crate::table::{Manager, SysRow, Total, UnitRef, UnitType};
use crate::time::TimeNs;
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
    /// Sorted by `ProcessSeriesId`.
    pub series: Arc<[(ProcessSeriesId, Arc<SeriesView>)]>,
}

impl ProcessSeriesSet {
    /// Binary search by id.
    pub fn get(&self, id: &ProcessSeriesId) -> Option<&Arc<SeriesView>> {
        self.series
            .binary_search_by(|(k, _)| k.cmp(id))
            .ok()
            .map(|i| &self.series[i].1)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProcessSnapshot {
    pub header: ComponentHeader,
    /// Sorted by `ProcessKey`.
    pub records: Arc<[ProcessRecord]>,
    pub coverage: Coverage,
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
    /// Sorted by `(pid, internal_id)`.
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
            details: Arc::default(),
            backend: ServiceBackend::Systemd,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ColumnId, GraphTarget, GroupKey, MetricId};
    use crate::process::{ProcFlags, ProcState, ProcessMeta};
    use crate::time::BootId;
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
        let set = ProcessSeriesSet {
            header: ComponentHeader::default(),
            series: Arc::from(series),
        };
        assert_eq!(set.get(&b).map(|v| v.id), Some(sid(1, MetricId::CPU_TOTAL)));
        assert!(set.get(&absent).is_none());
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
            assert_eq!(ServiceStatus::from_states(active, sub), want, "{active}+{sub}");
        }
        for sub in ["listening", "mounted", "waiting", "elapsed", "plugged"] {
            assert_eq!(ServiceStatus::from_states("active", sub), ServiceStatus::Active);
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
    }
}
