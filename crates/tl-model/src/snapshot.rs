//! The published snapshot and its components (ADR-003).

use std::ops::{BitOr, BitOrAssign};
use std::sync::Arc;

use crate::ids::{DeviceIdx, ProcessKey, ProcessSeriesId, SeriesId};
use crate::process::{Coverage, ProcessRecord};
use crate::series::SeriesView;
use crate::system::{
    CpuTopology, CpuTotals, DiskTotals, GpuTotals, MemoryTotals, NetTotals, SensorReading,
};
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
}
