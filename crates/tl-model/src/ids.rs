//! Frozen identifiers: process keys, devices, metrics, series, groups, table nodes, columns (ADR-003).
//!
//! `DeviceIdx`, `MetricId` and `SeriesId` numbers are stored in history files. Never renumber
//! them (ADR-012 constraint 2); append new ones only.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::time::BootId;

/// Identity of one process for one boot. PID alone is never used as identity.
///
/// The field order makes the derived `Ord` compare `(start_ticks, boot_id, pid)`. The process
/// table uses it as the stable sort tie-break.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessKey {
    /// /proc/<pid>/stat field 22, clock ticks since boot.
    pub start_ticks: u64,
    pub boot_id: BootId,
    pub pid: u32,
}

/// Index into `DeviceCatalog` for this engine session.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceIdx(pub u16);

/// Frozen metric number. Numbers stay below `TIER_1HZ` (0x8000).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MetricId(pub u16);

impl MetricId {
    pub const CPU_TOTAL: Self = Self(1);
    pub const CPU_KERNEL: Self = Self(2);
    pub const CPU_FREQ: Self = Self(3);
    pub const CPU_TEMP: Self = Self(4);
    pub const MEM_USED: Self = Self(10);
    pub const MEM_COMMIT: Self = Self(11);
    pub const SWAP_USED: Self = Self(12);
    pub const MEM_CACHED: Self = Self(13);
    pub const DISK_ACTIVE: Self = Self(20);
    pub const DISK_READ: Self = Self(21);
    pub const DISK_WRITE: Self = Self(22);
    pub const DISK_LATENCY: Self = Self(23);
    pub const NET_RX: Self = Self(30);
    pub const NET_TX: Self = Self(31);
    pub const GPU_UTIL: Self = Self(40);
    pub const GPU_MEM: Self = Self(41);
    pub const GPU_ENC: Self = Self(42);
    pub const GPU_DEC: Self = Self(43);
    pub const GPU_TEMP: Self = Self(44);
    pub const GPU_POWER: Self = Self(45);
    pub const GPU_CLOCK: Self = Self(46);
    pub const FAN_RPM: Self = Self(50);
    pub const SENSOR_TEMP: Self = Self(51);
    pub const BATTERY_PCT: Self = Self(60);
    // Appended by R-170 item 1 (never renumber):
    pub const CPU_POWER_PKG: Self = Self(5);
    pub const CPU_POWER_CORE: Self = Self(6);
    pub const CPU_CLOCK_MAX: Self = Self(7);
    pub const MEM_DIRTY: Self = Self(14);
    pub const GPU_MEM_SHARED: Self = Self(47);
    pub const GPU_MEM_CLOCK: Self = Self(48);
    pub const GPU_PCIE_RX: Self = Self(49);
    pub const GPU_PCIE_TX: Self = Self(52);
    pub const GPU_FAN: Self = Self(53);
    pub const GPU_ENGINE_MEMCTL: Self = Self(54);
    pub const GPU_ENGINE_JPEG: Self = Self(55);
    pub const GPU_ENGINE_OFA: Self = Self(56);
    /// The busiest vendor-specific engine, named by `GpuDeviceInfo`.
    pub const GPU_ENGINE_OTHER: Self = Self(57);
    pub const FAN_PWM: Self = Self(58);
    pub const BATTERY_POWER: Self = Self(61);

    /// Every constant with its name: the 24 base ones in order, then the 15 appended ones in
    /// order (39 entries). Used by the frozen-id test.
    pub const ALL: [(&'static str, MetricId); 39] = [
        ("CPU_TOTAL", Self::CPU_TOTAL),
        ("CPU_KERNEL", Self::CPU_KERNEL),
        ("CPU_FREQ", Self::CPU_FREQ),
        ("CPU_TEMP", Self::CPU_TEMP),
        ("MEM_USED", Self::MEM_USED),
        ("MEM_COMMIT", Self::MEM_COMMIT),
        ("SWAP_USED", Self::SWAP_USED),
        ("MEM_CACHED", Self::MEM_CACHED),
        ("DISK_ACTIVE", Self::DISK_ACTIVE),
        ("DISK_READ", Self::DISK_READ),
        ("DISK_WRITE", Self::DISK_WRITE),
        ("DISK_LATENCY", Self::DISK_LATENCY),
        ("NET_RX", Self::NET_RX),
        ("NET_TX", Self::NET_TX),
        ("GPU_UTIL", Self::GPU_UTIL),
        ("GPU_MEM", Self::GPU_MEM),
        ("GPU_ENC", Self::GPU_ENC),
        ("GPU_DEC", Self::GPU_DEC),
        ("GPU_TEMP", Self::GPU_TEMP),
        ("GPU_POWER", Self::GPU_POWER),
        ("GPU_CLOCK", Self::GPU_CLOCK),
        ("FAN_RPM", Self::FAN_RPM),
        ("SENSOR_TEMP", Self::SENSOR_TEMP),
        ("BATTERY_PCT", Self::BATTERY_PCT),
        ("CPU_POWER_PKG", Self::CPU_POWER_PKG),
        ("CPU_POWER_CORE", Self::CPU_POWER_CORE),
        ("CPU_CLOCK_MAX", Self::CPU_CLOCK_MAX),
        ("MEM_DIRTY", Self::MEM_DIRTY),
        ("GPU_MEM_SHARED", Self::GPU_MEM_SHARED),
        ("GPU_MEM_CLOCK", Self::GPU_MEM_CLOCK),
        ("GPU_PCIE_RX", Self::GPU_PCIE_RX),
        ("GPU_PCIE_TX", Self::GPU_PCIE_TX),
        ("GPU_FAN", Self::GPU_FAN),
        ("GPU_ENGINE_MEMCTL", Self::GPU_ENGINE_MEMCTL),
        ("GPU_ENGINE_JPEG", Self::GPU_ENGINE_JPEG),
        ("GPU_ENGINE_OFA", Self::GPU_ENGINE_OFA),
        ("GPU_ENGINE_OTHER", Self::GPU_ENGINE_OTHER),
        ("FAN_PWM", Self::FAN_PWM),
        ("BATTERY_POWER", Self::BATTERY_POWER),
    ];

    /// Bit 15 marks the 1 Hz slow-tier view of a metric (R-170 item 2).
    pub const TIER_1HZ: u16 = 0x8000;

    pub const fn with_tier_1hz(self) -> MetricId {
        Self(self.0 | Self::TIER_1HZ)
    }

    pub const fn is_tier_1hz(self) -> bool {
        self.0 & Self::TIER_1HZ != 0
    }

    /// The metric without the tier bit.
    pub const fn base(self) -> MetricId {
        Self(self.0 & !Self::TIER_1HZ)
    }
}

/// Series ordering is (device, metric); `SeriesSet` is sorted by it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SeriesId {
    pub device: DeviceIdx,
    pub metric: MetricId,
}

/// FxHash of `GroupKind` (process.rs computes it).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroupKey(pub u64);

/// One row of the process table (ADR-009, R-141 item 3, R-155 item 2).
///
/// Not `Copy`: E03-072 appends `Unit(UnitRef)`, which holds an `Arc<str>`. Callers clone it.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum NodeId {
    Group(GroupKey),
    Process(ProcessKey),
    KernelThreads,
    Pseudo(PseudoKind),
}

/// Pseudo rows of the process table (PROC-012).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum PseudoKind {
    Interrupts,
    MemoryCompression,
}

/// What a per-process graph follows (R-154). `ProcessSeriesId` pairs it with a column.
///
/// No serde derive: `Arc<str>` needs serde's `rc` feature.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GraphTarget {
    Process(ProcessKey),
    Name(Arc<str>),
    App(Arc<str>),
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    fn key(start_ticks: u64, pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BootId(42),
            pid,
        }
    }

    #[test]
    fn process_key_order_start_ticks_first() {
        assert!(key(100, 900) < key(200, 5));
    }

    #[test]
    fn process_key_pid_tie_break() {
        assert!(key(100, 5) < key(100, 7));
    }

    #[test]
    fn process_key_pid_reuse_not_equal() {
        assert_ne!(key(100, 5), key(200, 5));
    }

    #[test]
    fn metric_all_matches_constants() {
        let expected: [(&str, u16); 39] = [
            ("CPU_TOTAL", 1),
            ("CPU_KERNEL", 2),
            ("CPU_FREQ", 3),
            ("CPU_TEMP", 4),
            ("MEM_USED", 10),
            ("MEM_COMMIT", 11),
            ("SWAP_USED", 12),
            ("MEM_CACHED", 13),
            ("DISK_ACTIVE", 20),
            ("DISK_READ", 21),
            ("DISK_WRITE", 22),
            ("DISK_LATENCY", 23),
            ("NET_RX", 30),
            ("NET_TX", 31),
            ("GPU_UTIL", 40),
            ("GPU_MEM", 41),
            ("GPU_ENC", 42),
            ("GPU_DEC", 43),
            ("GPU_TEMP", 44),
            ("GPU_POWER", 45),
            ("GPU_CLOCK", 46),
            ("FAN_RPM", 50),
            ("SENSOR_TEMP", 51),
            ("BATTERY_PCT", 60),
            ("CPU_POWER_PKG", 5),
            ("CPU_POWER_CORE", 6),
            ("CPU_CLOCK_MAX", 7),
            ("MEM_DIRTY", 14),
            ("GPU_MEM_SHARED", 47),
            ("GPU_MEM_CLOCK", 48),
            ("GPU_PCIE_RX", 49),
            ("GPU_PCIE_TX", 52),
            ("GPU_FAN", 53),
            ("GPU_ENGINE_MEMCTL", 54),
            ("GPU_ENGINE_JPEG", 55),
            ("GPU_ENGINE_OFA", 56),
            ("GPU_ENGINE_OTHER", 57),
            ("FAN_PWM", 58),
            ("BATTERY_POWER", 61),
        ];
        assert_eq!(MetricId::ALL.len(), 39);
        for ((name, id), (want_name, want_num)) in MetricId::ALL.iter().zip(expected) {
            assert_eq!(*name, want_name);
            assert_eq!(id.0, want_num, "{name}");
            assert!(id.0 < MetricId::TIER_1HZ, "{name}");
        }
        let numbers: HashSet<u16> = MetricId::ALL.iter().map(|(_, id)| id.0).collect();
        assert_eq!(numbers.len(), 39, "a metric number is used twice");
        let names: HashSet<&str> = MetricId::ALL.iter().map(|(name, _)| *name).collect();
        assert_eq!(names.len(), 39, "a metric name is used twice");
    }

    #[test]
    fn metric_tier_1hz_bit() {
        let slow = MetricId::GPU_UTIL.with_tier_1hz();
        assert_eq!(slow.0, 0x8000 | 40);
        assert!(slow.is_tier_1hz());
        assert_eq!(slow.base(), MetricId::GPU_UTIL);
        assert!(!MetricId::GPU_UTIL.is_tier_1hz());
    }

    #[test]
    fn series_id_order() {
        let s = |device, metric| SeriesId {
            device: DeviceIdx(device),
            metric: MetricId(metric),
        };
        let mut ids = vec![s(1, 1), s(0, 2), s(0, 1), s(1, 0)];
        ids.sort();
        assert_eq!(ids, vec![s(0, 1), s(0, 2), s(1, 0), s(1, 1)]);
    }

    #[test]
    fn node_id_hash_map_keys() {
        let nodes = [
            NodeId::Process(key(100, 5)),
            NodeId::Group(GroupKey(7)),
            NodeId::KernelThreads,
            NodeId::Pseudo(PseudoKind::Interrupts),
        ];
        let map: HashMap<NodeId, usize> = nodes.iter().cloned().zip(0..).collect();
        assert_eq!(map.len(), 4);
        for (i, node) in nodes.iter().enumerate() {
            assert_eq!(map.get(node), Some(&i));
        }
    }
}
