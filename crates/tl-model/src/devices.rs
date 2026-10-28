//! Device identity and the device catalog (ADR-003).
//!
//! Every device has an identity and a session index. A device that disappears keeps its index
//! with `present = false`. One that comes back with the same identity gets the same index and
//! `generation + 1`. Counters are never subtracted across generations (ADR-003, PERF-005).
//! The engine reducer owns the catalog; this module only defines the types.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::DeviceIdx;

/// What makes a device the same device across disappear and return (ADR-003).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeviceIdentity {
    /// The whole machine's CPU.
    CpuPackage,
    CpuLogical {
        id: u16,
    },
    Memory,
    /// NVML UUID, or the DRM PCI slot for non-NVIDIA cards.
    Gpu {
        uuid: [u8; 16],
        pci_bus_id: Arc<str>,
    },
    /// `wwn_or_serial` is a hardware serial. Never log it (PERF-011).
    Disk {
        major: u32,
        minor: u32,
        name: Arc<str>,
        wwn_or_serial: Option<Arc<str>>,
    },
    Net {
        netns_ino: u64,
        ifindex: u32,
        name: Arc<str>,
    },
    /// hwmon name plus label.
    Sensor {
        chip: Arc<str>,
        label: Arc<str>,
    },
    Fan {
        chip: Arc<str>,
        index: u16,
    },
    Battery {
        native_path: Arc<str>,
    },
    /// Disk, Network and GPU card series computed by the reducer (R-154).
    Aggregate(AggregateKind),
    /// One band of the memory-by-app chart (D-036, UI-PRF-060): a memory group key such as
    /// `app:google-chrome` or `exe:rustc`, or `other` and `shared` for the two fixed bands.
    /// The persistent key is `memgroup:<key>`.
    MemGroup {
        key: Arc<str>,
    },
}

/// Aggregate devices, created at session start with `present = true`. Persistent keys
/// `aggregate:disk`, `aggregate:net`, `aggregate:gpu` (PERF-005, R-154).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum AggregateKind {
    Disks,
    Nets,
    Gpus,
}

/// Device class without the identity data, so pages can filter the catalog.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DeviceKind {
    Cpu,
    CpuLogical,
    Memory,
    Gpu,
    Disk,
    Net,
    Sensor,
    Fan,
    Battery,
    Aggregate,
    MemGroup,
}

impl DeviceIdentity {
    /// `CpuPackage` gives `Cpu`, `Aggregate(_)` gives `Aggregate`, the rest match by name.
    pub fn kind(&self) -> DeviceKind {
        match self {
            DeviceIdentity::CpuPackage => DeviceKind::Cpu,
            DeviceIdentity::CpuLogical { .. } => DeviceKind::CpuLogical,
            DeviceIdentity::Memory => DeviceKind::Memory,
            DeviceIdentity::Gpu { .. } => DeviceKind::Gpu,
            DeviceIdentity::Disk { .. } => DeviceKind::Disk,
            DeviceIdentity::Net { .. } => DeviceKind::Net,
            DeviceIdentity::Sensor { .. } => DeviceKind::Sensor,
            DeviceIdentity::Fan { .. } => DeviceKind::Fan,
            DeviceIdentity::Battery { .. } => DeviceKind::Battery,
            DeviceIdentity::Aggregate(_) => DeviceKind::Aggregate,
            DeviceIdentity::MemGroup { .. } => DeviceKind::MemGroup,
        }
    }
}

/// One catalog entry. A device with `present = false` shows `Missing` for its live values.
#[derive(Clone, Debug)]
pub struct DeviceEntry {
    pub idx: DeviceIdx,
    pub identity: DeviceIdentity,
    pub generation: u32,
    pub display_name: Arc<str>,
    pub present: bool,
    /// Partitions and device-mapper children point to their disk.
    pub parent: Option<DeviceIdx>,
    /// PERF-005 key computed by the worker, for example `disk:<ID_WWN or ID_SERIAL>`
    /// (R-155 item 7).
    pub persistent_key: Arc<str>,
    /// `Some` for `Net` devices only (R-155 item 7).
    pub net_kind: Option<NetKind>,
    /// The Hot threshold in °C (ADR-016, UI-PRF-054): hwmon `temp<n>_crit`, else `temp<n>_max`
    /// for a sensor; the NVML slowdown threshold for an NVIDIA GPU. `None` when the device
    /// has none: it never counts as Hot from its temperature.
    pub hot_c: Option<f32>,
}

/// All devices of the session. The reducer replaces the whole `Arc` only when something
/// changes, and bumps `version` when it does.
#[derive(Clone, Debug, Default)]
pub struct DeviceCatalog {
    pub version: u64,
    pub entries: Arc<[DeviceEntry]>,
}

impl DeviceCatalog {
    /// `entries[idx.0]`, or `None` when out of range. Entries are stored in idx order:
    /// `entries[i].idx == DeviceIdx(i)`.
    pub fn get(&self, idx: DeviceIdx) -> Option<&DeviceEntry> {
        self.entries.get(usize::from(idx.0))
    }

    /// Linear search by identity (the catalog is small: tens of entries).
    pub fn find(&self, identity: &DeviceIdentity) -> Option<&DeviceEntry> {
        self.entries.iter().find(|e| e.identity == *identity)
    }

    /// Present entries of one kind, in idx order.
    pub fn present_of(&self, kind: DeviceKind) -> impl Iterator<Item = &DeviceEntry> + '_ {
        self.entries
            .iter()
            .filter(move |e| e.present && e.identity.kind() == kind)
    }
}

/// Network interface class for sidebar visibility (ADR-016, PERF-054).
///
/// tl-collect (E04) classifies interfaces and the UI relies on these rules:
/// - `lo` is never shown.
/// - `tailscale0` is `VpnTunnel` and shown.
/// - `docker0`, `veth*`, `br-*`, `virbr*` and `vnet*` are `Virtual` and hidden by default.
/// - Settings `performance.show_network_{wired,wireless,vpn,virtual,other}` control each
///   class. Only `virtual` defaults to false.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum NetKind {
    Wired,
    Wireless,
    VpnTunnel,
    Virtual,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(idx: u16, identity: DeviceIdentity, present: bool) -> DeviceEntry {
        let net_kind = matches!(identity, DeviceIdentity::Net { .. }).then_some(NetKind::Wired);
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("device"),
            present,
            parent: None,
            persistent_key: Arc::from("key"),
            net_kind,
            hot_c: None,
        }
    }

    fn catalog(entries: Vec<DeviceEntry>) -> DeviceCatalog {
        DeviceCatalog {
            version: 1,
            entries: entries.into(),
        }
    }

    fn disk(minor: u32, name: &str) -> DeviceIdentity {
        DeviceIdentity::Disk {
            major: 259,
            minor,
            name: Arc::from(name),
            wwn_or_serial: None,
        }
    }

    fn net(ifindex: u32) -> DeviceIdentity {
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex,
            name: Arc::from("eno1"),
        }
    }

    #[test]
    fn catalog_get_by_idx() {
        let c = catalog(vec![
            entry(0, DeviceIdentity::CpuPackage, true),
            entry(1, DeviceIdentity::Memory, true),
            entry(2, net(2), true),
        ]);
        let e = c.get(DeviceIdx(1)).expect("idx 1 exists");
        assert_eq!(e.idx, DeviceIdx(1));
        assert_eq!(e.identity, DeviceIdentity::Memory);
        assert!(c.get(DeviceIdx(3)).is_none());
    }

    #[test]
    fn catalog_find_by_identity() {
        let c = catalog(vec![
            entry(0, DeviceIdentity::CpuPackage, true),
            entry(1, net(2), true),
        ]);
        let e = c.find(&net(2)).expect("eno1 is in the catalog");
        assert_eq!(e.idx, DeviceIdx(1));
        assert_eq!(e.net_kind, Some(NetKind::Wired));
        assert!(c.find(&net(3)).is_none());
    }

    #[test]
    fn catalog_present_of_kind() {
        let c = catalog(vec![
            entry(0, DeviceIdentity::CpuPackage, true),
            entry(1, disk(0, "nvme0n1"), false),
            entry(2, disk(1, "nvme1n1"), true),
        ]);
        let disks: Vec<DeviceIdx> = c.present_of(DeviceKind::Disk).map(|e| e.idx).collect();
        assert_eq!(disks, vec![DeviceIdx(2)]);
    }

    #[test]
    fn identity_kind_mapping() {
        let s = |v: &str| Arc::<str>::from(v);
        let cases = [
            (DeviceIdentity::CpuPackage, DeviceKind::Cpu),
            (DeviceIdentity::CpuLogical { id: 3 }, DeviceKind::CpuLogical),
            (DeviceIdentity::Memory, DeviceKind::Memory),
            (
                DeviceIdentity::Gpu {
                    uuid: [0; 16],
                    pci_bus_id: s("0000:01:00.0"),
                },
                DeviceKind::Gpu,
            ),
            (disk(0, "sda"), DeviceKind::Disk),
            (net(2), DeviceKind::Net),
            (
                DeviceIdentity::Sensor {
                    chip: s("k10temp"),
                    label: s("Tctl"),
                },
                DeviceKind::Sensor,
            ),
            (
                DeviceIdentity::Fan {
                    chip: s("nct6775"),
                    index: 1,
                },
                DeviceKind::Fan,
            ),
            (
                DeviceIdentity::Battery {
                    native_path: s("BAT0"),
                },
                DeviceKind::Battery,
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Disks),
                DeviceKind::Aggregate,
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Nets),
                DeviceKind::Aggregate,
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Gpus),
                DeviceKind::Aggregate,
            ),
            (
                DeviceIdentity::MemGroup {
                    key: s("exe:rustc"),
                },
                DeviceKind::MemGroup,
            ),
        ];
        for (identity, kind) in cases {
            assert_eq!(identity.kind(), kind, "{identity:?}");
        }
    }

    #[test]
    fn catalog_find_aggregate() {
        let mut nets = entry(1, DeviceIdentity::Aggregate(AggregateKind::Nets), true);
        nets.persistent_key = Arc::from("aggregate:net");
        let c = catalog(vec![
            entry(0, DeviceIdentity::Aggregate(AggregateKind::Disks), true),
            nets,
            entry(2, DeviceIdentity::Aggregate(AggregateKind::Gpus), true),
        ]);
        let e = c
            .find(&DeviceIdentity::Aggregate(AggregateKind::Nets))
            .expect("net aggregate exists");
        assert_eq!(e.idx, DeviceIdx(1));
        assert_eq!(&*e.persistent_key, "aggregate:net");
    }
}
