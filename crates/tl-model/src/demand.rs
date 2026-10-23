//! What the UI currently shows, which drives sampling (ADR-008).
//!
//! The UI's `DemandAggregator` builds one `Demand` and sends it with `EngineClient::set_demand`
//! only when it changed. Every engine worker reads it to pick its rate or switch off.
//! `Demand::default()` means nothing is visible and every source is off.

use std::sync::Arc;

use crate::details::{DetailTab, DetailTarget};
use crate::ids::{ColumnId, GraphTarget, ProcessKey};
use crate::table::UnitRef;

/// How much CPU data the visible surfaces need. `PerCore` implies `Total`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CpuDemand {
    #[default]
    Off,
    Total,
    PerCore,
}

impl CpuDemand {
    pub fn wants_total(self) -> bool {
        self != Self::Off
    }

    pub fn wants_per_core(self) -> bool {
        self == Self::PerCore
    }
}

/// Bitset of `ColumnId`, 256 bits. Bit index = `ColumnId::bit()` (number minus 1, R-155 item 1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct ColumnSet(pub [u64; 4]);

impl ColumnSet {
    pub const EMPTY: Self = Self([0; 4]);

    pub fn insert(&mut self, c: ColumnId) {
        let b = c.bit();
        self.0[(b / 64) as usize] |= 1 << (b % 64);
    }

    pub fn remove(&mut self, c: ColumnId) {
        let b = c.bit();
        self.0[(b / 64) as usize] &= !(1 << (b % 64));
    }

    pub fn contains(&self, c: ColumnId) -> bool {
        let b = c.bit();
        self.0[(b / 64) as usize] & (1 << (b % 64)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == [0; 4]
    }

    /// Columns in number order.
    pub fn iter(&self) -> impl Iterator<Item = ColumnId> + '_ {
        ColumnId::ALL.into_iter().filter(|c| self.contains(*c))
    }
}

/// One open Properties window and its visible tab (UI-PWN-002).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DetailDemand {
    pub target: DetailTarget,
    pub tab: DetailTab,
}

/// What the tray icons show (P3). There is no process list here: tray process graphs use
/// `Demand.graph_targets` (R-156 item 2, R-154).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TrayDemand {
    /// Any tray icon is visible. The aggregator counts this as a visible surface.
    pub icons_shown: bool,
    pub cpu: bool,
    pub memory: bool,
    pub disks: bool,
    pub nets: bool,
    pub gpus: bool,
}

pub const DETAIL_KEYS_MAX: usize = 512;
pub const DETAILS_MAX: usize = 16;
pub const AFFINITY_TARGETS_MAX: usize = 256;
pub const UNIT_DETAILS_MAX: usize = 10;
pub const GRAPH_TARGETS_MAX: usize = 16; // R-154

/// Everything the visible surfaces need. Lists are shared `Arc` slices, so a clone is cheap.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Demand {
    pub any_surface_visible: bool,
    pub cpu: CpuDemand,
    /// A visible CPU chart is animating: sample at 10 Hz.
    pub cpu_fast: bool,
    pub memory: bool,
    pub disks: bool,
    pub nets: bool,
    pub gpus: bool,
    pub sensors: bool,
    pub process_table: bool,
    /// The Overview Top processes box is placed: the process sweep runs at
    /// `processes.interval_ms`, with no table history window (crw.45).
    pub process_top: bool,
    pub process_columns: ColumnSet,
    /// Visible rows plus one viewport of overscan, max `DETAIL_KEYS_MAX`.
    pub detail_keys: Arc<[ProcessKey]>,
    pub gpu_process: bool,
    pub connections: bool,
    pub services: bool,
    pub startup: bool,
    pub inventory: bool,
    /// P2: open Properties windows, max `DETAILS_MAX`.
    pub details: Arc<[DetailDemand]>,
    /// P2: the open Set Affinity dialog, max `AFFINITY_TARGETS_MAX`.
    pub affinity_targets: Arc<[ProcessKey]>,
    /// P2: a unit resource column is shown.
    pub service_cgroup_stats: bool,
    /// P2: open Service Properties windows, max `UNIT_DETAILS_MAX`.
    pub unit_details: Arc<[UnitRef]>,
    /// P4: visible rows of the Remote host column.
    pub remote_host_cookies: Arc<[u64]>,
    /// P4: Namespaces "All".
    pub connections_all_netns: bool,
    /// P3.
    pub tray: TrayDemand,
    /// P2: per-process graphs (minigraph cards, tray), max `GRAPH_TARGETS_MAX` (R-154).
    /// Non-empty keeps the Processes worker at `processes.interval_ms` even when
    /// `process_table` is false (PROC-093). Columns other than cpu, memory, disk, network, gpu
    /// and gpu_mem_dedicated are ignored by the engine.
    pub graph_targets: Arc<[(GraphTarget, ColumnId)]>,
    /// P3: history recording is enabled.
    pub recording: bool,
    /// P2: `history.device_metrics` is on (ADR-017). Independent of visibility.
    pub device_history: bool,
}

impl Demand {
    /// Truncates every list to its max (keeps the first entries). The aggregator calls it before
    /// sending.
    pub fn clamp_lists(&mut self) {
        truncate(&mut self.detail_keys, DETAIL_KEYS_MAX);
        truncate(&mut self.details, DETAILS_MAX);
        truncate(&mut self.affinity_targets, AFFINITY_TARGETS_MAX);
        truncate(&mut self.unit_details, UNIT_DETAILS_MAX);
        truncate(&mut self.graph_targets, GRAPH_TARGETS_MAX);
    }
}

fn truncate<T: Clone>(list: &mut Arc<[T]>, max: usize) {
    if list.len() > max {
        *list = Arc::from(&list[..max]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::Manager;
    use crate::time::BootId;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1000 + pid as u64,
            boot_id: BootId(42),
            pid,
        }
    }

    fn col(s: &str) -> ColumnId {
        ColumnId::from_str(s).unwrap()
    }

    #[test]
    fn cpu_demand_per_core_implies_total() {
        assert!(CpuDemand::PerCore.wants_total());
        assert!(CpuDemand::PerCore.wants_per_core());
        assert!(CpuDemand::Total.wants_total());
        assert!(!CpuDemand::Total.wants_per_core());
        assert!(!CpuDemand::Off.wants_total());
        assert!(!CpuDemand::Off.wants_per_core());
    }

    #[test]
    fn column_set_insert_contains_iter() {
        let mut set = ColumnSet::EMPTY;
        set.insert(col("cpu"));
        set.insert(col("memory"));
        assert!(set.contains(col("cpu")));
        assert!(set.contains(col("memory")));
        assert!(!set.contains(col("pid")));
        let got: Vec<ColumnId> = set.iter().collect();
        assert_eq!(got, vec![col("cpu"), col("memory")]);

        // The first and last columns land on the right bits.
        let mut edges = ColumnSet::EMPTY;
        edges.insert(ColumnId::Name);
        edges.insert(ColumnId::Container);
        assert_eq!(
            edges.iter().collect::<Vec<_>>(),
            vec![ColumnId::Name, ColumnId::Container]
        );
    }

    #[test]
    fn column_set_remove_empty() {
        let mut set = ColumnSet::default();
        set.insert(col("cpu"));
        assert!(!set.is_empty());
        set.remove(col("cpu"));
        assert!(set.is_empty());
        assert_eq!(set, ColumnSet::EMPTY);
    }

    #[test]
    fn demand_clamp_lists() {
        let keys: Vec<ProcessKey> = (0..600).map(key).collect();
        let details: Vec<DetailDemand> = (0..20)
            .map(|i| DetailDemand {
                target: DetailTarget::Process(key(i)),
                tab: DetailTab::General,
            })
            .collect();
        let affinity: Vec<ProcessKey> = (0..300).map(key).collect();
        let units: Vec<UnitRef> = (0..12)
            .map(|i| UnitRef {
                manager: Manager::System,
                name: format!("u{i}.service").into(),
            })
            .collect();
        let graphs: Vec<(GraphTarget, ColumnId)> = (0..20)
            .map(|i| (GraphTarget::Process(key(i)), ColumnId::Cpu))
            .collect();
        let mut d = Demand {
            detail_keys: keys.clone().into(),
            details: details.clone().into(),
            affinity_targets: affinity.clone().into(),
            unit_details: units.clone().into(),
            graph_targets: graphs.clone().into(),
            ..Demand::default()
        };
        d.clamp_lists();
        assert_eq!(&d.detail_keys[..], &keys[..DETAIL_KEYS_MAX]);
        assert_eq!(&d.details[..], &details[..DETAILS_MAX]);
        assert_eq!(&d.affinity_targets[..], &affinity[..AFFINITY_TARGETS_MAX]);
        assert_eq!(&d.unit_details[..], &units[..UNIT_DETAILS_MAX]);
        assert_eq!(&d.graph_targets[..], &graphs[..GRAPH_TARGETS_MAX]);
        assert_eq!(
            [
                d.detail_keys.len(),
                d.details.len(),
                d.affinity_targets.len(),
                d.unit_details.len(),
                d.graph_targets.len()
            ],
            [512, 16, 256, 10, 16]
        );

        // Lists already within their max keep the same allocation.
        let before = d.detail_keys.clone();
        d.clamp_lists();
        assert!(Arc::ptr_eq(&before, &d.detail_keys));
    }

    #[test]
    fn demand_eq_detects_cpu_fast() {
        let a = Demand::default();
        let b = Demand {
            cpu_fast: true,
            ..Demand::default()
        };
        assert_ne!(a, b);
    }

    #[test]
    fn demand_default_all_off() {
        let d = Demand::default();
        let bools = [
            d.any_surface_visible,
            d.cpu_fast,
            d.memory,
            d.disks,
            d.nets,
            d.gpus,
            d.sensors,
            d.process_table,
            d.process_top,
            d.gpu_process,
            d.connections,
            d.services,
            d.startup,
            d.inventory,
            d.service_cgroup_stats,
            d.connections_all_netns,
            d.recording,
            d.tray.icons_shown,
            d.tray.cpu,
            d.tray.memory,
            d.tray.disks,
            d.tray.nets,
            d.tray.gpus,
        ];
        assert!(bools.iter().all(|b| !b));
        assert_eq!(d.cpu, CpuDemand::Off);
        assert!(d.process_columns.is_empty());
        assert!(d.detail_keys.is_empty());
        assert!(d.details.is_empty());
        assert!(d.affinity_targets.is_empty());
        assert!(d.unit_details.is_empty());
        assert!(d.remote_host_cookies.is_empty());
        assert!(d.graph_targets.is_empty());
    }
}
