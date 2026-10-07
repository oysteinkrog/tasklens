//! The one message type workers send to the reducer (ADR-004).
//!
//! `SourceUpdate` carries every variant for every phase up front, so later epics add reducer
//! arms, never variants. Payloads are built from `tl-model` types so the reducer can move them
//! into `Arc<[T]>` without a second copy. Device-bound samples are keyed by `DeviceIdentity`,
//! because the reducer's catalog assigns `DeviceIdx` (ADR-003).
//!
//! Expected conditions (EACCES, missing device, unsupported) travel as `Validity` inside the
//! payload. `WorkerError` is only the restart signal (ADR-004, ADR-015).
//!
//! `ProcessSweep` holds process text. Its `Debug` output is never logged.

use std::sync::Arc;
use std::time::Duration;

use tl_model::actions::{ActionId, CgroupRef};
use tl_model::command::{Command, SetupState};
use tl_model::details::{AffinityInfo, DetailTab, DetailTarget, HandleSearchState, TabPayload};
use tl_model::devices::{DeviceIdentity, NetKind};
use tl_model::gpu_proto::{ApiVersions, GpuProcessSample};
use tl_model::health::{HelperStartOutcome, HelperState, ManagerState, SourceId, SourceState};
use tl_model::history::HistoryView;
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{
    AppCgroupSample, LowMemoryInfo, ProcessExtras, ProcessExtrasP2, ProcessRecord,
};
use tl_model::snapshot::{
    BatteryDetails, ConnRecord, CpuStaticInfo, DiskDetails, InventorySnapshot, MemoryDetails,
    NetDetails, ServiceBackend, StartupEntry, UnitDetails, UnitRecord, WindowsSnapshot,
};
use tl_model::system::{
    CpuTopology, CpuTotals, DiskTotals, GpuTotals, MemoryTotals, NetTotals, SensorReading,
};
use tl_model::table::{Manager, UnitRef};
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

#[derive(Debug, Clone)]
pub enum EngineMsg {
    Source(SourceUpdate),
    Command(Command),
    ActionFinished {
        id: ActionId,
    },
    /// Housekeeping, sent every 1 s by the session (stale marking, linger expiry).
    Tick,
}

/// The restart signal a worker returns as `Outcome::Err` (ADR-015).
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("collect: {0}")]
    Collect(#[from] tl_collect::CollectError),
    #[error("d-bus: {0}")]
    Dbus(Arc<str>),
    #[error("probe: {0}")]
    Probe(Arc<str>),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("channel closed")]
    Closed,
    #[error("{0}")]
    Other(Arc<str>),
}

#[derive(Debug, Clone)]
pub enum SourceUpdate {
    Health {
        id: SourceId,
        state: SourceState,
        last_error: Option<Arc<str>>,
        t_ns: TimeNs,
    },
    /// ADR-008 clock check: Δboot − Δmono > 250 ms.
    ResumeDetected { id: SourceId, t_ns: TimeNs },
    /// Logind `PrepareForSleep`.
    Sleep { entering: bool, t_ns: TimeNs },
    /// E12 `Cpu`.
    Cpu(CpuSample),
    /// E12 `Memory`.
    Memory(MemorySample),
    /// E12 `Disk`.
    Disks(DiskSample),
    /// E12 `Net`.
    Nets(NetSample),
    /// E12 `Sensors`.
    Sensors(SensorSample),
    /// E12 device enumeration, every 5 s.
    Devices(DeviceList),
    /// E13 `GpuNvidia`, E12 `GpuDrm`.
    Gpu(GpuSample),
    /// Static per-device GPU info.
    GpuInfo(GpuInfoUpdate),
    /// E13, once at start.
    GpuApiVersions(ApiVersions),
    /// E10 `Processes`, E32 low-memory sweep.
    Processes(ProcessSweep),
    /// E10 `ProcessDetail` and the handle search.
    Details(DetailsUpdate),
    /// E14 `Connections`.
    Connections(ConnectionsUpdate),
    /// E11 SockDestroy answer; sets `destroy_supported`.
    ConnectionsDestroySupport(bool),
    /// E14 `SystemdSystem` and `SystemdUser`.
    Services(ServicesUpdate),
    /// E14 `Startup`.
    Startup(StartupUpdate),
    /// E12 `Inventory`. The reducer rewrites the header.
    Inventory(Box<InventorySnapshot>),
    /// E14 `KWin`.
    Windows(Box<WindowsSnapshot>),
    /// E30 answer to `HistoryQuery`.
    History(Arc<HistoryView>),
    /// E12 static and slow device rows (R-139 item 7).
    DeviceDetails(DeviceDetailsUpdate),
    /// E29 helper session state (R-150 item 1).
    Helper(HelperUpdate),
    /// E29 setup runner (R-150 item 1, R-149 item 4).
    Setup(Arc<SetupState>),
    /// E14 portal color scheme 0, 1 or 2 (R-170 item 4).
    ColorScheme(u8),
}

/// Static and slow device rows. The reducer merges them by field (R-139 item 7).
#[derive(Debug, Clone)]
pub enum DeviceDetailsUpdate {
    Cpu(Arc<CpuStaticInfo>),
    Memory(Arc<MemoryDetails>),
    /// The full list from that source.
    Disks {
        from: SourceId,
        entries: Vec<(DeviceIdentity, DiskDetails)>,
    },
    Nets {
        from: SourceId,
        entries: Vec<(DeviceIdentity, NetDetails)>,
    },
    Batteries(Vec<(DeviceIdentity, BatteryDetails)>),
}

/// Helper session state (R-150 item 1).
#[derive(Debug, Clone)]
pub struct HelperUpdate {
    pub state: HelperState,
    pub start: Option<(u64, HelperStartOutcome)>,
}

#[derive(Debug, Clone)]
pub struct CpuSample {
    pub t_ns: TimeNs,
    pub totals: CpuTotals,
    /// Δ(irq + softirq) / Δtotal, for the Interrupts row.
    pub irq_pct: f32,
    pub per_cpu: Vec<CpuCoreSample>,
    pub freq_mhz: Option<u32>,
    pub temp_c: Option<f32>,
    pub temp_validity: Validity,
    /// `Some` on the first sample and on hotplug.
    pub topology: Option<Arc<CpuTopology>>,
    pub validity: Validity,
}

#[derive(Debug, Clone)]
pub struct CpuCoreSample {
    pub cpu: u16,
    pub pct: f32,
    pub kernel_pct: f32,
    pub freq_mhz: Option<u32>,
    pub validity: Validity,
}

#[derive(Debug, Clone)]
pub struct MemorySample {
    pub t_ns: TimeNs,
    pub totals: MemoryTotals,
    pub zram: Vec<ZramStat>,
    pub validity: Validity,
}

#[derive(Debug, Clone)]
pub struct ZramStat {
    pub name: Arc<str>,
    pub disksize: u64,
    pub orig_bytes: u64,
    pub compr_bytes: u64,
    pub mem_used_total: u64,
    pub read_bps: f32,
    pub write_bps: f32,
}

#[derive(Debug, Clone)]
pub struct DiskSample {
    pub t_ns: TimeNs,
    pub disks: Vec<(DeviceIdentity, DiskTotals)>,
}

#[derive(Debug, Clone)]
pub struct NetSample {
    pub t_ns: TimeNs,
    pub nets: Vec<(DeviceIdentity, NetTotals)>,
}

#[derive(Debug, Clone)]
pub struct SensorSample {
    pub t_ns: TimeNs,
    pub readings: Vec<(DeviceIdentity, SensorReading)>,
}

bitflags::bitflags! {
    /// Device kinds a `DeviceList` covers.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct DeviceKinds: u8 {
        const CPUS = 1;
        const DISKS = 2;
        const NETS = 4;
        const SENSORS = 8;
        const FANS = 16;
        const GPUS = 32;
        const BATTERIES = 64;
    }
}

/// The complete device list for `kinds`. The reducer diffs it against the catalog.
#[derive(Debug, Clone)]
pub struct DeviceList {
    pub t_ns: TimeNs,
    pub kinds: DeviceKinds,
    pub entries: Vec<DeviceDesc>,
}

#[derive(Debug, Clone)]
pub struct DeviceDesc {
    pub identity: DeviceIdentity,
    pub display_name: Arc<str>,
    pub parent: Option<DeviceIdentity>,
    /// PERF-005 key, computed by the worker (R-155 item 7).
    pub persistent_key: Arc<str>,
    /// Net devices only.
    pub net_kind: Option<NetKind>,
}

#[derive(Debug, Clone)]
pub struct GpuSample {
    pub t_ns: TimeNs,
    pub devices: Vec<(DeviceIdentity, GpuTotals)>,
    /// NVIDIA rows; `None` when processes were not sampled.
    pub processes: Option<Vec<GpuProcessSample>>,
    /// AMD and Intel rows from DRM fdinfo; `None` when processes were not sampled.
    pub drm_processes: Option<Vec<DrmProcessVram>>,
}

/// Dedicated GPU memory of one DRM client, from `/proc/<pid>/fdinfo`. A process with several
/// clients has several rows; a client shared by several processes has one row.
#[derive(Debug, Clone, PartialEq)]
pub struct DrmProcessVram {
    /// The card's PCI slot in sysfs form, as in its `DeviceIdentity`.
    pub pdev: Arc<str>,
    pub pid: u32,
    pub vram_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct GpuInfoUpdate {
    pub devices: Vec<GpuStaticInfo>,
}

#[derive(Debug, Clone)]
pub struct GpuStaticInfo {
    pub identity: DeviceIdentity,
    pub name: Arc<str>,
    pub mem_total: u64,
    pub power_limit_mw: Option<u32>,
    pub driver: Arc<str>,
    pub nvml: Arc<str>,
    pub supports_process_util: bool,
    /// `"gpu:<uuid or PCI slot>"` (R-155 items 7 and 8).
    pub persistent_key: Arc<str>,
}

#[derive(Debug, Clone)]
pub struct ProcessSweep {
    pub observed_start: TimeNs,
    pub observed_end: TimeNs,
    /// Sorted by key. The reducer sets NEW and EXITED.
    pub records: Vec<ProcessRecord>,
    /// Index aligned with `records` (R-141 item 1).
    pub extras: Vec<ProcessExtras>,
    /// Index aligned with `records`; filled for detail keys only (R-163 item 2).
    pub extras_p2: Vec<ProcessExtrasP2>,
    /// R-141 item 2.
    pub app_stats: Vec<AppCgroupSample>,
    pub unit_stats: Vec<CgroupStats>,
    pub system_threads: u32,
    /// `/proc/sys/fs/file-nr`. The reducer copies it into `TableTotals.handles` (R-166 B3-7).
    pub file_nr: Option<u64>,
    pub online_cpus: u16,
    /// `Some` only from the E32 stripped sweep (R-150 item 2).
    pub low_memory: Option<Arc<LowMemoryInfo>>,
}

#[derive(Debug, Clone)]
pub struct CgroupStats {
    pub group: GroupKey,
    pub cgroup: CgroupRef,
    pub anon_bytes: u64,
    pub swap_bytes: u64,
    pub cpu_usage_usec: u64,
    pub io_rbytes: u64,
    pub io_wbytes: u64,
    pub pids_current: u32,
    pub validity: Validity,
}

#[derive(Debug, Clone)]
pub enum DetailsUpdate {
    Pss {
        t_ns: TimeNs,
        entries: Vec<PssEntry>,
    },
    Tab {
        target: DetailTarget,
        tab: DetailTab,
        payload: Arc<TabPayload>,
        validity: Validity,
    },
    Affinity {
        t_ns: TimeNs,
        entries: Vec<(ProcessKey, AffinityInfo)>,
    },
    HandleSearch(Arc<HandleSearchState>),
}

#[derive(Debug, Clone)]
pub struct PssEntry {
    pub key: ProcessKey,
    pub pss_bytes: u64,
    pub uss_bytes: u64,
    pub swap_bytes: u64,
    pub validity: Validity,
}

#[derive(Debug, Clone)]
pub struct ConnectionsUpdate {
    pub t_ns: TimeNs,
    pub records: Vec<ConnRecord>,
    pub validity: Validity,
    /// Per-process TCP send and receive rates (PROC-032). A socket held by several processes
    /// counts in full for each, with `shared` set.
    pub attribution: rustc_hash::FxHashMap<ProcessKey, tl_collect::net::NetRate>,
}

#[derive(Debug, Clone)]
pub struct ServicesUpdate {
    pub manager: Manager,
    pub state: ManagerState,
    pub units: Vec<UnitRecord>,
    pub details: Vec<(UnitRef, Arc<UnitDetails>)>,
    pub backend: ServiceBackend,
    pub validity: Validity,
}

#[derive(Debug, Clone)]
pub struct StartupUpdate {
    pub entries: Vec<StartupEntry>,
    pub validity: Validity,
}

/// The worker that produced `update`, for health tracking. `None` for updates that no
/// supervised source sends (R-150 item 1). Variants that several workers send follow R-272.
pub fn source_of(update: &SourceUpdate) -> Option<SourceId> {
    match update {
        SourceUpdate::Health { id, .. } | SourceUpdate::ResumeDetected { id, .. } => Some(*id),
        SourceUpdate::Sleep { .. } => None,
        SourceUpdate::Cpu(_) => Some(SourceId::Cpu),
        SourceUpdate::Memory(_) => Some(SourceId::Memory),
        SourceUpdate::Disks(_) => Some(SourceId::Disk),
        SourceUpdate::Nets(_) => Some(SourceId::Net),
        SourceUpdate::Sensors(_) => Some(SourceId::Sensors),
        SourceUpdate::Devices(list) => devices_source(list.kinds),
        SourceUpdate::Gpu(sample) => gpu_source(sample.devices.iter().map(|(id, _)| id)),
        SourceUpdate::GpuInfo(info) => gpu_source(info.devices.iter().map(|d| &d.identity)),
        // The one-shot API versions probe is not a supervised source.
        SourceUpdate::GpuApiVersions(_) => None,
        // The E32 low-memory sweep has no `SourceId` (R-155).
        SourceUpdate::Processes(sweep) => match sweep.low_memory {
            Some(_) => None,
            None => Some(SourceId::Processes),
        },
        SourceUpdate::Details(_) => Some(SourceId::ProcessDetail),
        SourceUpdate::Connections(_) => Some(SourceId::Connections),
        SourceUpdate::ConnectionsDestroySupport(_) => None,
        SourceUpdate::Services(services) => Some(match services.manager {
            Manager::System => SourceId::SystemdSystem,
            Manager::User => SourceId::SystemdUser,
        }),
        SourceUpdate::Startup(_) => Some(SourceId::Startup),
        SourceUpdate::Inventory(_) => Some(SourceId::Inventory),
        SourceUpdate::Windows(_) => Some(SourceId::KWin),
        SourceUpdate::History(_) => None,
        SourceUpdate::DeviceDetails(details) => Some(match details {
            DeviceDetailsUpdate::Cpu(_) => SourceId::Cpu,
            DeviceDetailsUpdate::Memory(_) => SourceId::Memory,
            DeviceDetailsUpdate::Disks { from, .. } | DeviceDetailsUpdate::Nets { from, .. } => {
                *from
            }
            DeviceDetailsUpdate::Batteries(_) => SourceId::UPower,
        }),
        SourceUpdate::Helper(_) => None,
        SourceUpdate::Setup(_) => None,
        // The portal reader (E14-020) is not a supervised source.
        SourceUpdate::ColorScheme(_) => None,
    }
}

/// The worker that owns `kinds`. `None` when no single source owns them all, and for GPUS
/// (no GPU list in P1).
fn devices_source(kinds: DeviceKinds) -> Option<SourceId> {
    if kinds == DeviceKinds::CPUS {
        Some(SourceId::Cpu)
    } else if kinds == DeviceKinds::DISKS {
        Some(SourceId::Disk)
    } else if kinds == DeviceKinds::NETS {
        Some(SourceId::Net)
    } else if !kinds.is_empty() && (DeviceKinds::SENSORS | DeviceKinds::FANS).contains(kinds) {
        Some(SourceId::Sensors)
    } else if kinds == DeviceKinds::BATTERIES {
        Some(SourceId::UPower)
    } else {
        None
    }
}

/// NVML devices carry their UUID; AMD and Intel GPUs use `uuid: [0; 16]` (R-166).
/// An empty or non-GPU list gives `None`.
fn gpu_source<'a>(mut ids: impl Iterator<Item = &'a DeviceIdentity>) -> Option<SourceId> {
    match ids.find(|id| matches!(id, DeviceIdentity::Gpu { .. }))? {
        DeviceIdentity::Gpu { uuid, .. } if *uuid != [0; 16] => Some(SourceId::GpuNvidia),
        _ => Some(SourceId::GpuDrm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_msg_size() {
        assert!(
            std::mem::size_of::<EngineMsg>() <= 256,
            "EngineMsg is {} bytes",
            std::mem::size_of::<EngineMsg>()
        );
    }

    fn gpu_id(uuid: [u8; 16]) -> DeviceIdentity {
        DeviceIdentity::Gpu {
            uuid,
            pci_bus_id: Arc::from("0000:01:00.0"),
        }
    }

    fn gpu_info(uuid: [u8; 16]) -> SourceUpdate {
        SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![GpuStaticInfo {
                identity: gpu_id(uuid),
                name: Arc::from(""),
                mem_total: 0,
                power_limit_mw: None,
                driver: Arc::from(""),
                nvml: Arc::from(""),
                supports_process_util: false,
                persistent_key: Arc::from("gpu:0000:01:00.0"),
            }],
        })
    }

    fn devices(kinds: DeviceKinds) -> SourceUpdate {
        SourceUpdate::Devices(DeviceList {
            t_ns: 0,
            kinds,
            entries: Vec::new(),
        })
    }

    fn sweep(low_memory: Option<Arc<LowMemoryInfo>>) -> SourceUpdate {
        SourceUpdate::Processes(ProcessSweep {
            observed_start: 0,
            observed_end: 0,
            records: Vec::new(),
            extras: Vec::new(),
            extras_p2: Vec::new(),
            app_stats: Vec::new(),
            unit_stats: Vec::new(),
            system_threads: 0,
            file_nr: None,
            online_cpus: 1,
            low_memory,
        })
    }

    fn services(manager: Manager) -> SourceUpdate {
        SourceUpdate::Services(ServicesUpdate {
            manager,
            state: ManagerState::default(),
            units: Vec::new(),
            details: Vec::new(),
            backend: ServiceBackend::Systemd,
            validity: Validity::default(),
        })
    }

    /// One update per variant. The match below has no wildcard arm, so a new variant fails to
    /// compile until it is added here.
    fn every_variant() -> Vec<(SourceUpdate, Option<SourceId>)> {
        let samples = vec![
            (
                SourceUpdate::Health {
                    id: SourceId::Journal,
                    state: SourceState::default(),
                    last_error: None,
                    t_ns: 0,
                },
                Some(SourceId::Journal),
            ),
            (
                SourceUpdate::ResumeDetected {
                    id: SourceId::Logind,
                    t_ns: 0,
                },
                Some(SourceId::Logind),
            ),
            (
                SourceUpdate::Sleep {
                    entering: true,
                    t_ns: 0,
                },
                None,
            ),
            (
                SourceUpdate::Cpu(CpuSample {
                    t_ns: 0,
                    totals: CpuTotals::default(),
                    irq_pct: 0.0,
                    per_cpu: Vec::new(),
                    freq_mhz: None,
                    temp_c: None,
                    temp_validity: Validity::default(),
                    topology: None,
                    validity: Validity::default(),
                }),
                Some(SourceId::Cpu),
            ),
            (
                SourceUpdate::Memory(MemorySample {
                    t_ns: 0,
                    totals: MemoryTotals::default(),
                    zram: Vec::new(),
                    validity: Validity::default(),
                }),
                Some(SourceId::Memory),
            ),
            (
                SourceUpdate::Disks(DiskSample {
                    t_ns: 0,
                    disks: Vec::new(),
                }),
                Some(SourceId::Disk),
            ),
            (
                SourceUpdate::Nets(NetSample {
                    t_ns: 0,
                    nets: Vec::new(),
                }),
                Some(SourceId::Net),
            ),
            (
                SourceUpdate::Sensors(SensorSample {
                    t_ns: 0,
                    readings: Vec::new(),
                }),
                Some(SourceId::Sensors),
            ),
            (devices(DeviceKinds::CPUS), Some(SourceId::Cpu)),
            (devices(DeviceKinds::DISKS), Some(SourceId::Disk)),
            (devices(DeviceKinds::NETS), Some(SourceId::Net)),
            (
                devices(DeviceKinds::SENSORS | DeviceKinds::FANS),
                Some(SourceId::Sensors),
            ),
            (devices(DeviceKinds::FANS), Some(SourceId::Sensors)),
            (devices(DeviceKinds::BATTERIES), Some(SourceId::UPower)),
            (devices(DeviceKinds::GPUS), None),
            (devices(DeviceKinds::CPUS | DeviceKinds::DISKS), None),
            (devices(DeviceKinds::empty()), None),
            (gpu_info([7; 16]), Some(SourceId::GpuNvidia)),
            (gpu_info([0; 16]), Some(SourceId::GpuDrm)),
            (
                SourceUpdate::Gpu(GpuSample {
                    t_ns: 0,
                    devices: Vec::new(),
                    processes: None,
                    drm_processes: None,
                }),
                None,
            ),
            (
                SourceUpdate::GpuApiVersions(ApiVersions {
                    devices: Vec::new(),
                }),
                None,
            ),
            (sweep(None), Some(SourceId::Processes)),
            (
                sweep(Some(Arc::new(LowMemoryInfo {
                    groups: Arc::from([]),
                    mem_total: 0,
                    mem_available: 0,
                    psi_full_pct: None,
                    critical: true,
                }))),
                None,
            ),
            (
                SourceUpdate::Details(DetailsUpdate::Pss {
                    t_ns: 0,
                    entries: Vec::new(),
                }),
                Some(SourceId::ProcessDetail),
            ),
            (
                SourceUpdate::Details(DetailsUpdate::Affinity {
                    t_ns: 0,
                    entries: Vec::new(),
                }),
                Some(SourceId::ProcessDetail),
            ),
            (
                SourceUpdate::Connections(ConnectionsUpdate {
                    t_ns: 0,
                    records: Vec::new(),
                    validity: Validity::default(),
                    attribution: Default::default(),
                }),
                Some(SourceId::Connections),
            ),
            (SourceUpdate::ConnectionsDestroySupport(true), None),
            (services(Manager::System), Some(SourceId::SystemdSystem)),
            (services(Manager::User), Some(SourceId::SystemdUser)),
            (
                SourceUpdate::Startup(StartupUpdate {
                    entries: Vec::new(),
                    validity: Validity::default(),
                }),
                Some(SourceId::Startup),
            ),
            (
                SourceUpdate::Inventory(Box::default()),
                Some(SourceId::Inventory),
            ),
            (SourceUpdate::Windows(Box::default()), Some(SourceId::KWin)),
            (SourceUpdate::History(Arc::default()), None),
            (
                SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Cpu(Arc::default())),
                Some(SourceId::Cpu),
            ),
            (
                SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Memory(Arc::default())),
                Some(SourceId::Memory),
            ),
            (
                SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
                    from: SourceId::Udisks,
                    entries: Vec::new(),
                }),
                Some(SourceId::Udisks),
            ),
            (
                SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Nets {
                    from: SourceId::NetworkManager,
                    entries: Vec::new(),
                }),
                Some(SourceId::NetworkManager),
            ),
            (
                SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Batteries(Vec::new())),
                Some(SourceId::UPower),
            ),
            (
                SourceUpdate::Helper(HelperUpdate {
                    state: HelperState::default(),
                    start: None,
                }),
                None,
            ),
            (SourceUpdate::Setup(Arc::default()), None),
            (SourceUpdate::ColorScheme(1), None),
        ];
        // Exhaustiveness check: no wildcard arm.
        for (update, _) in &samples {
            match update {
                SourceUpdate::Health { .. }
                | SourceUpdate::ResumeDetected { .. }
                | SourceUpdate::Sleep { .. }
                | SourceUpdate::Cpu(_)
                | SourceUpdate::Memory(_)
                | SourceUpdate::Disks(_)
                | SourceUpdate::Nets(_)
                | SourceUpdate::Sensors(_)
                | SourceUpdate::Devices(_)
                | SourceUpdate::Gpu(_)
                | SourceUpdate::GpuInfo(_)
                | SourceUpdate::GpuApiVersions(_)
                | SourceUpdate::Processes(_)
                | SourceUpdate::Details(_)
                | SourceUpdate::Connections(_)
                | SourceUpdate::ConnectionsDestroySupport(_)
                | SourceUpdate::Services(_)
                | SourceUpdate::Startup(_)
                | SourceUpdate::Inventory(_)
                | SourceUpdate::Windows(_)
                | SourceUpdate::History(_)
                | SourceUpdate::DeviceDetails(_)
                | SourceUpdate::Helper(_)
                | SourceUpdate::Setup(_)
                | SourceUpdate::ColorScheme(_) => {}
            }
        }
        samples
    }

    #[test]
    fn source_of_covers_every_variant() {
        for (update, expected) in every_variant() {
            assert_eq!(source_of(&update), expected, "{update:?}");
        }
    }
}
