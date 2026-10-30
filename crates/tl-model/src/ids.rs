//! Frozen identifiers: process keys, devices, metrics, series, groups, table nodes, columns (ADR-003).
//!
//! `DeviceIdx`, `MetricId` and `SeriesId` numbers are stored in history files. Never renumber
//! them (ADR-012 constraint 2); append new ones only.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::table::UnitRef;
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
    /// MemAvailable in bytes (tasklens-hjn.35).
    pub const MEM_AVAILABLE: Self = Self(15);
    // Per-engine GPU series (tasklens-hjn.37). 70 to 79 are kept for GPU engines.
    // GPU_ENGINE_OTHER (57) stays unused: no engine maps to it.
    /// AMD and Intel compute engine.
    pub const GPU_ENGINE_COMPUTE: Self = Self(70);
    /// AMD SDMA engine.
    pub const GPU_ENGINE_DMA: Self = Self(71);
    /// Intel render engine (Render/3D).
    pub const GPU_ENGINE_RENDER: Self = Self(72);
    /// Intel copy (blitter) engine.
    pub const GPU_ENGINE_COPY: Self = Self(73);
    /// Intel video engine, which both encodes and decodes.
    pub const GPU_ENGINE_VIDEO: Self = Self(74);
    /// Intel video-enhance engine (Video Processing).
    pub const GPU_ENGINE_VIDEO_ENHANCE: Self = Self(75);
    // System pressure for the status bar and device history (tasklens-hjn.47, ADR-016 "System
    // pressure"). CPU_LOAD1 and PSI_CPU_SOME are on the CpuPackage device, the other three on
    // the Memory device.
    /// The 1-minute load average from /proc/loadavg.
    pub const CPU_LOAD1: Self = Self(8);
    /// /proc/pressure/cpu `some` avg10, percent.
    pub const PSI_CPU_SOME: Self = Self(9);
    /// /proc/pressure/memory `some` avg10, percent.
    pub const PSI_MEM_SOME: Self = Self(16);
    /// /proc/pressure/memory `full` avg10, percent.
    pub const PSI_MEM_FULL: Self = Self(17);
    /// Pages swapped in per second, from the /proc/vmstat `pswpin` delta.
    pub const SWAP_IN: Self = Self(18);
    /// RssAnon bytes of one memory group, on a `DeviceIdentity::MemGroup` device (D-036,
    /// UI-PRF-060). "Other apps" and "Shared and kernel" use it too.
    pub const MEM_GROUP: Self = Self(19);
    /// Clock of one physical core in MHz: the highest clock of its SMT siblings
    /// (tasklens-o55.56). On the `CpuLogical` device of the core's lowest online CPU. Not
    /// recorded while the clock comes from BogoMIPS. 70 to 79 stay kept for GPU engines.
    pub const CPU_CORE_CLOCK: Self = Self(80);

    /// Every constant with its name: the 24 base ones in order, then the 29 appended ones in
    /// order (53 entries). Used by the frozen-id test.
    pub const ALL: [(&'static str, MetricId); 53] = [
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
        ("MEM_AVAILABLE", Self::MEM_AVAILABLE),
        ("GPU_ENGINE_COMPUTE", Self::GPU_ENGINE_COMPUTE),
        ("GPU_ENGINE_DMA", Self::GPU_ENGINE_DMA),
        ("GPU_ENGINE_RENDER", Self::GPU_ENGINE_RENDER),
        ("GPU_ENGINE_COPY", Self::GPU_ENGINE_COPY),
        ("GPU_ENGINE_VIDEO", Self::GPU_ENGINE_VIDEO),
        ("GPU_ENGINE_VIDEO_ENHANCE", Self::GPU_ENGINE_VIDEO_ENHANCE),
        ("CPU_LOAD1", Self::CPU_LOAD1),
        ("PSI_CPU_SOME", Self::PSI_CPU_SOME),
        ("PSI_MEM_SOME", Self::PSI_MEM_SOME),
        ("PSI_MEM_FULL", Self::PSI_MEM_FULL),
        ("SWAP_IN", Self::SWAP_IN),
        ("MEM_GROUP", Self::MEM_GROUP),
        ("CPU_CORE_CLOCK", Self::CPU_CORE_CLOCK),
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
/// Not `Copy`: `Unit(UnitRef)` holds an `Arc<str>`. Callers clone it.
///
/// serde for the engine protocol only (ADR-018). `NodeId` is never written to disk or sent to the
/// helper.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum NodeId {
    Group(GroupKey),
    Process(ProcessKey),
    KernelThreads,
    Pseudo(PseudoKind),
    Unit(UnitRef),
}

/// Pseudo rows of the process table (PROC-012).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum PseudoKind {
    Interrupts,
    MemoryCompression,
}

/// What a per-process graph follows (R-154). `ProcessSeriesId` pairs it with a column.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GraphTarget {
    Process(ProcessKey),
    Name(Arc<str>),
    App(Arc<str>),
}

/// Process-table column (PROC-022, R-007). Numbers (the discriminants) and strings are frozen
/// (ADR-012 constraint 2): new columns append after 121 and are never renumbered.
///
/// `ws_private` is left out on purpose (R-005). Serde writes and reads the string id.
#[repr(u16)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum ColumnId {
    // PROC-027 identity, image and package
    Name = 1,
    NameDisplay = 2,
    ImageName = 3,
    ImagePath = 4,
    Cmdline = 5,
    Description = 6,
    Type = 7,
    StartTime = 8,
    Version = 9,
    Timestamp = 10,
    FileMtime = 11,
    FileSize = 12,
    Company = 13,
    Signer = 14,
    Signature = 15,
    Architecture = 16,
    // PROC-028 ownership, session and security
    User = 17,
    Pid = 18,
    Ppid = 19,
    Session = 20,
    SecurityLabel = 21,
    Elevation = 22,
    Sandbox = 23,
    // PROC-029 CPU and scheduling
    Cpu = 24,
    CpuKernel = 25,
    CpuUser = 26,
    CpuRelative = 27,
    CpuTime = 28,
    CpuTimeKernel = 29,
    CpuTimeUser = 30,
    Priority = 31,
    Affinity = 32,
    Cycles = 33,
    CyclesDelta = 34,
    CtxSwitches = 35,
    CtxSwitchesDelta = 36,
    // PROC-030 memory
    Memory = 37,
    WsTotal = 38,
    WsPeak = 39,
    WsShared = 40,
    Commit = 41,
    CommitPeak = 42,
    CommitDelta = 43,
    Vsize = 44,
    VsizePeak = 45,
    PageTables = 46,
    PageFaults = 47,
    PageFaultsDelta = 48,
    MajorFaults = 49,
    MajorFaultsDelta = 50,
    Swap = 51,
    Pss = 52,
    Uss = 53,
    // PROC-031 I/O and disk
    Disk = 54,
    DiskReadRate = 55,
    DiskWriteRate = 56,
    DiskTotalRate = 57,
    DiskReadBytes = 58,
    DiskWriteBytes = 59,
    DiskTotalBytes = 60,
    DiskReadDelta = 61,
    DiskWriteDelta = 62,
    DiskTotalDelta = 63,
    DiskReads = 64,
    DiskWrites = 65,
    DiskReadsDelta = 66,
    DiskWritesDelta = 67,
    IoReadBytes = 68,
    IoWriteBytes = 69,
    IoReadDelta = 70,
    IoWriteDelta = 71,
    IoReadRate = 72,
    IoWriteRate = 73,
    IoReadOps = 74,
    IoWriteOps = 75,
    // PROC-032 network
    Network = 76,
    NetSendRate = 77,
    NetRecvRate = 78,
    // PROC-033 GPU
    Gpu = 79,
    GpuMemDedicated = 80,
    GpuMemShared = 81,
    GpuEngine = 82,
    GpuEncode = 83,
    GpuDecode = 84,
    Gpu0 = 85,
    Gpu1 = 86,
    Gpu2 = 87,
    Gpu3 = 88,
    Gpu4 = 89,
    Gpu5 = 90,
    Gpu6 = 91,
    Gpu7 = 92,
    Gpu8 = 93,
    Gpu9 = 94,
    Gpu10 = 95,
    Gpu11 = 96,
    Gpu12 = 97,
    Gpu13 = 98,
    Gpu14 = 99,
    Gpu15 = 100,
    // PROC-034 power
    Power = 101,
    PowerTrend = 102,
    // PROC-035 object counts
    Handles = 103,
    Threads = 104,
    Windows = 105,
    X11Resources = 106,
    // PROC-036 Linux extras
    State = 107,
    Nice = 108,
    SchedPolicy = 109,
    IoPriority = 110,
    Unit = 111,
    Cgroup = 112,
    AppId = 113,
    OomScore = 114,
    OomScoreAdj = 115,
    Tty = 116,
    Cwd = 117,
    Pgrp = 118,
    Sid = 119,
    LastCpu = 120,
    Container = 121,
}

impl ColumnId {
    pub const COUNT: usize = 121;

    /// Every column in number order (1..=121).
    pub const ALL: [ColumnId; Self::COUNT] = [
        Self::Name,
        Self::NameDisplay,
        Self::ImageName,
        Self::ImagePath,
        Self::Cmdline,
        Self::Description,
        Self::Type,
        Self::StartTime,
        Self::Version,
        Self::Timestamp,
        Self::FileMtime,
        Self::FileSize,
        Self::Company,
        Self::Signer,
        Self::Signature,
        Self::Architecture,
        Self::User,
        Self::Pid,
        Self::Ppid,
        Self::Session,
        Self::SecurityLabel,
        Self::Elevation,
        Self::Sandbox,
        Self::Cpu,
        Self::CpuKernel,
        Self::CpuUser,
        Self::CpuRelative,
        Self::CpuTime,
        Self::CpuTimeKernel,
        Self::CpuTimeUser,
        Self::Priority,
        Self::Affinity,
        Self::Cycles,
        Self::CyclesDelta,
        Self::CtxSwitches,
        Self::CtxSwitchesDelta,
        Self::Memory,
        Self::WsTotal,
        Self::WsPeak,
        Self::WsShared,
        Self::Commit,
        Self::CommitPeak,
        Self::CommitDelta,
        Self::Vsize,
        Self::VsizePeak,
        Self::PageTables,
        Self::PageFaults,
        Self::PageFaultsDelta,
        Self::MajorFaults,
        Self::MajorFaultsDelta,
        Self::Swap,
        Self::Pss,
        Self::Uss,
        Self::Disk,
        Self::DiskReadRate,
        Self::DiskWriteRate,
        Self::DiskTotalRate,
        Self::DiskReadBytes,
        Self::DiskWriteBytes,
        Self::DiskTotalBytes,
        Self::DiskReadDelta,
        Self::DiskWriteDelta,
        Self::DiskTotalDelta,
        Self::DiskReads,
        Self::DiskWrites,
        Self::DiskReadsDelta,
        Self::DiskWritesDelta,
        Self::IoReadBytes,
        Self::IoWriteBytes,
        Self::IoReadDelta,
        Self::IoWriteDelta,
        Self::IoReadRate,
        Self::IoWriteRate,
        Self::IoReadOps,
        Self::IoWriteOps,
        Self::Network,
        Self::NetSendRate,
        Self::NetRecvRate,
        Self::Gpu,
        Self::GpuMemDedicated,
        Self::GpuMemShared,
        Self::GpuEngine,
        Self::GpuEncode,
        Self::GpuDecode,
        Self::Gpu0,
        Self::Gpu1,
        Self::Gpu2,
        Self::Gpu3,
        Self::Gpu4,
        Self::Gpu5,
        Self::Gpu6,
        Self::Gpu7,
        Self::Gpu8,
        Self::Gpu9,
        Self::Gpu10,
        Self::Gpu11,
        Self::Gpu12,
        Self::Gpu13,
        Self::Gpu14,
        Self::Gpu15,
        Self::Power,
        Self::PowerTrend,
        Self::Handles,
        Self::Threads,
        Self::Windows,
        Self::X11Resources,
        Self::State,
        Self::Nice,
        Self::SchedPolicy,
        Self::IoPriority,
        Self::Unit,
        Self::Cgroup,
        Self::AppId,
        Self::OomScore,
        Self::OomScoreAdj,
        Self::Tty,
        Self::Cwd,
        Self::Pgrp,
        Self::Sid,
        Self::LastCpu,
        Self::Container,
    ];

    /// The frozen string id, for example "memory".
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::NameDisplay => "name_display",
            Self::ImageName => "image_name",
            Self::ImagePath => "image_path",
            Self::Cmdline => "cmdline",
            Self::Description => "description",
            Self::Type => "type",
            Self::StartTime => "start_time",
            Self::Version => "version",
            Self::Timestamp => "timestamp",
            Self::FileMtime => "file_mtime",
            Self::FileSize => "file_size",
            Self::Company => "company",
            Self::Signer => "signer",
            Self::Signature => "signature",
            Self::Architecture => "architecture",
            Self::User => "user",
            Self::Pid => "pid",
            Self::Ppid => "ppid",
            Self::Session => "session",
            Self::SecurityLabel => "security_label",
            Self::Elevation => "elevation",
            Self::Sandbox => "sandbox",
            Self::Cpu => "cpu",
            Self::CpuKernel => "cpu_kernel",
            Self::CpuUser => "cpu_user",
            Self::CpuRelative => "cpu_relative",
            Self::CpuTime => "cpu_time",
            Self::CpuTimeKernel => "cpu_time_kernel",
            Self::CpuTimeUser => "cpu_time_user",
            Self::Priority => "priority",
            Self::Affinity => "affinity",
            Self::Cycles => "cycles",
            Self::CyclesDelta => "cycles_delta",
            Self::CtxSwitches => "ctx_switches",
            Self::CtxSwitchesDelta => "ctx_switches_delta",
            Self::Memory => "memory",
            Self::WsTotal => "ws_total",
            Self::WsPeak => "ws_peak",
            Self::WsShared => "ws_shared",
            Self::Commit => "commit",
            Self::CommitPeak => "commit_peak",
            Self::CommitDelta => "commit_delta",
            Self::Vsize => "vsize",
            Self::VsizePeak => "vsize_peak",
            Self::PageTables => "page_tables",
            Self::PageFaults => "page_faults",
            Self::PageFaultsDelta => "page_faults_delta",
            Self::MajorFaults => "major_faults",
            Self::MajorFaultsDelta => "major_faults_delta",
            Self::Swap => "swap",
            Self::Pss => "pss",
            Self::Uss => "uss",
            Self::Disk => "disk",
            Self::DiskReadRate => "disk_read_rate",
            Self::DiskWriteRate => "disk_write_rate",
            Self::DiskTotalRate => "disk_total_rate",
            Self::DiskReadBytes => "disk_read_bytes",
            Self::DiskWriteBytes => "disk_write_bytes",
            Self::DiskTotalBytes => "disk_total_bytes",
            Self::DiskReadDelta => "disk_read_delta",
            Self::DiskWriteDelta => "disk_write_delta",
            Self::DiskTotalDelta => "disk_total_delta",
            Self::DiskReads => "disk_reads",
            Self::DiskWrites => "disk_writes",
            Self::DiskReadsDelta => "disk_reads_delta",
            Self::DiskWritesDelta => "disk_writes_delta",
            Self::IoReadBytes => "io_read_bytes",
            Self::IoWriteBytes => "io_write_bytes",
            Self::IoReadDelta => "io_read_delta",
            Self::IoWriteDelta => "io_write_delta",
            Self::IoReadRate => "io_read_rate",
            Self::IoWriteRate => "io_write_rate",
            Self::IoReadOps => "io_read_ops",
            Self::IoWriteOps => "io_write_ops",
            Self::Network => "network",
            Self::NetSendRate => "net_send_rate",
            Self::NetRecvRate => "net_recv_rate",
            Self::Gpu => "gpu",
            Self::GpuMemDedicated => "gpu_mem_dedicated",
            Self::GpuMemShared => "gpu_mem_shared",
            Self::GpuEngine => "gpu_engine",
            Self::GpuEncode => "gpu_encode",
            Self::GpuDecode => "gpu_decode",
            Self::Gpu0 => "gpu_0",
            Self::Gpu1 => "gpu_1",
            Self::Gpu2 => "gpu_2",
            Self::Gpu3 => "gpu_3",
            Self::Gpu4 => "gpu_4",
            Self::Gpu5 => "gpu_5",
            Self::Gpu6 => "gpu_6",
            Self::Gpu7 => "gpu_7",
            Self::Gpu8 => "gpu_8",
            Self::Gpu9 => "gpu_9",
            Self::Gpu10 => "gpu_10",
            Self::Gpu11 => "gpu_11",
            Self::Gpu12 => "gpu_12",
            Self::Gpu13 => "gpu_13",
            Self::Gpu14 => "gpu_14",
            Self::Gpu15 => "gpu_15",
            Self::Power => "power",
            Self::PowerTrend => "power_trend",
            Self::Handles => "handles",
            Self::Threads => "threads",
            Self::Windows => "windows",
            Self::X11Resources => "x11_resources",
            Self::State => "state",
            Self::Nice => "nice",
            Self::SchedPolicy => "sched_policy",
            Self::IoPriority => "io_priority",
            Self::Unit => "unit",
            Self::Cgroup => "cgroup",
            Self::AppId => "app_id",
            Self::OomScore => "oom_score",
            Self::OomScoreAdj => "oom_score_adj",
            Self::Tty => "tty",
            Self::Cwd => "cwd",
            Self::Pgrp => "pgrp",
            Self::Sid => "sid",
            Self::LastCpu => "last_cpu",
            Self::Container => "container",
        }
    }

    /// Exact, case-sensitive match on the string id. `None` for anything else, including
    /// "ws_private". The caller logs and drops unknown ids (ADR-011).
    #[allow(clippy::should_implement_trait)] // returns Option, not Result, on purpose
    pub fn from_str(s: &str) -> Option<ColumnId> {
        Some(match s {
            "name" => Self::Name,
            "name_display" => Self::NameDisplay,
            "image_name" => Self::ImageName,
            "image_path" => Self::ImagePath,
            "cmdline" => Self::Cmdline,
            "description" => Self::Description,
            "type" => Self::Type,
            "start_time" => Self::StartTime,
            "version" => Self::Version,
            "timestamp" => Self::Timestamp,
            "file_mtime" => Self::FileMtime,
            "file_size" => Self::FileSize,
            "company" => Self::Company,
            "signer" => Self::Signer,
            "signature" => Self::Signature,
            "architecture" => Self::Architecture,
            "user" => Self::User,
            "pid" => Self::Pid,
            "ppid" => Self::Ppid,
            "session" => Self::Session,
            "security_label" => Self::SecurityLabel,
            "elevation" => Self::Elevation,
            "sandbox" => Self::Sandbox,
            "cpu" => Self::Cpu,
            "cpu_kernel" => Self::CpuKernel,
            "cpu_user" => Self::CpuUser,
            "cpu_relative" => Self::CpuRelative,
            "cpu_time" => Self::CpuTime,
            "cpu_time_kernel" => Self::CpuTimeKernel,
            "cpu_time_user" => Self::CpuTimeUser,
            "priority" => Self::Priority,
            "affinity" => Self::Affinity,
            "cycles" => Self::Cycles,
            "cycles_delta" => Self::CyclesDelta,
            "ctx_switches" => Self::CtxSwitches,
            "ctx_switches_delta" => Self::CtxSwitchesDelta,
            "memory" => Self::Memory,
            "ws_total" => Self::WsTotal,
            "ws_peak" => Self::WsPeak,
            "ws_shared" => Self::WsShared,
            "commit" => Self::Commit,
            "commit_peak" => Self::CommitPeak,
            "commit_delta" => Self::CommitDelta,
            "vsize" => Self::Vsize,
            "vsize_peak" => Self::VsizePeak,
            "page_tables" => Self::PageTables,
            "page_faults" => Self::PageFaults,
            "page_faults_delta" => Self::PageFaultsDelta,
            "major_faults" => Self::MajorFaults,
            "major_faults_delta" => Self::MajorFaultsDelta,
            "swap" => Self::Swap,
            "pss" => Self::Pss,
            "uss" => Self::Uss,
            "disk" => Self::Disk,
            "disk_read_rate" => Self::DiskReadRate,
            "disk_write_rate" => Self::DiskWriteRate,
            "disk_total_rate" => Self::DiskTotalRate,
            "disk_read_bytes" => Self::DiskReadBytes,
            "disk_write_bytes" => Self::DiskWriteBytes,
            "disk_total_bytes" => Self::DiskTotalBytes,
            "disk_read_delta" => Self::DiskReadDelta,
            "disk_write_delta" => Self::DiskWriteDelta,
            "disk_total_delta" => Self::DiskTotalDelta,
            "disk_reads" => Self::DiskReads,
            "disk_writes" => Self::DiskWrites,
            "disk_reads_delta" => Self::DiskReadsDelta,
            "disk_writes_delta" => Self::DiskWritesDelta,
            "io_read_bytes" => Self::IoReadBytes,
            "io_write_bytes" => Self::IoWriteBytes,
            "io_read_delta" => Self::IoReadDelta,
            "io_write_delta" => Self::IoWriteDelta,
            "io_read_rate" => Self::IoReadRate,
            "io_write_rate" => Self::IoWriteRate,
            "io_read_ops" => Self::IoReadOps,
            "io_write_ops" => Self::IoWriteOps,
            "network" => Self::Network,
            "net_send_rate" => Self::NetSendRate,
            "net_recv_rate" => Self::NetRecvRate,
            "gpu" => Self::Gpu,
            "gpu_mem_dedicated" => Self::GpuMemDedicated,
            "gpu_mem_shared" => Self::GpuMemShared,
            "gpu_engine" => Self::GpuEngine,
            "gpu_encode" => Self::GpuEncode,
            "gpu_decode" => Self::GpuDecode,
            "gpu_0" => Self::Gpu0,
            "gpu_1" => Self::Gpu1,
            "gpu_2" => Self::Gpu2,
            "gpu_3" => Self::Gpu3,
            "gpu_4" => Self::Gpu4,
            "gpu_5" => Self::Gpu5,
            "gpu_6" => Self::Gpu6,
            "gpu_7" => Self::Gpu7,
            "gpu_8" => Self::Gpu8,
            "gpu_9" => Self::Gpu9,
            "gpu_10" => Self::Gpu10,
            "gpu_11" => Self::Gpu11,
            "gpu_12" => Self::Gpu12,
            "gpu_13" => Self::Gpu13,
            "gpu_14" => Self::Gpu14,
            "gpu_15" => Self::Gpu15,
            "power" => Self::Power,
            "power_trend" => Self::PowerTrend,
            "handles" => Self::Handles,
            "threads" => Self::Threads,
            "windows" => Self::Windows,
            "x11_resources" => Self::X11Resources,
            "state" => Self::State,
            "nice" => Self::Nice,
            "sched_policy" => Self::SchedPolicy,
            "io_priority" => Self::IoPriority,
            "unit" => Self::Unit,
            "cgroup" => Self::Cgroup,
            "app_id" => Self::AppId,
            "oom_score" => Self::OomScore,
            "oom_score_adj" => Self::OomScoreAdj,
            "tty" => Self::Tty,
            "cwd" => Self::Cwd,
            "pgrp" => Self::Pgrp,
            "sid" => Self::Sid,
            "last_cpu" => Self::LastCpu,
            "container" => Self::Container,
            _ => return None,
        })
    }

    /// The frozen number (the discriminant), 1..=121.
    pub const fn number(self) -> u16 {
        self as u16
    }

    /// Inverse of `number()`. `None` for 0 and 122..=65535.
    pub const fn from_number(n: u16) -> Option<ColumnId> {
        if n == 0 || n as usize > Self::COUNT {
            return None;
        }
        Some(Self::ALL[n as usize - 1])
    }

    /// Bit index for `ColumnSet`: `number() - 1`, so 0..=120 (fits a 128-bit set).
    pub const fn bit(self) -> u32 {
        self.number() as u32 - 1
    }
}

impl Serialize for ColumnId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ColumnId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ColumnIdVisitor;

        impl serde::de::Visitor<'_> for ColumnIdVisitor {
            type Value = ColumnId;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a process column id string")
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<ColumnId, E> {
                ColumnId::from_str(s)
                    .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Str(s), &self))
            }
        }

        deserializer.deserialize_str(ColumnIdVisitor)
    }
}

/// One engine-sampled per-process series (`Demand.graph_targets`, `Snapshot.process_series`).
///
/// Columns allowed as graph sources: `cpu`, `memory`, `disk`, `network`, `gpu`,
/// `gpu_mem_dedicated`. The engine ignores others (R-154).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessSeriesId {
    pub target: GraphTarget,
    pub column: ColumnId,
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
        let expected: [(&str, u16); 53] = [
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
            ("MEM_AVAILABLE", 15),
            ("GPU_ENGINE_COMPUTE", 70),
            ("GPU_ENGINE_DMA", 71),
            ("GPU_ENGINE_RENDER", 72),
            ("GPU_ENGINE_COPY", 73),
            ("GPU_ENGINE_VIDEO", 74),
            ("GPU_ENGINE_VIDEO_ENHANCE", 75),
            ("CPU_LOAD1", 8),
            ("PSI_CPU_SOME", 9),
            ("PSI_MEM_SOME", 16),
            ("PSI_MEM_FULL", 17),
            ("SWAP_IN", 18),
            ("MEM_GROUP", 19),
            ("CPU_CORE_CLOCK", 80),
        ];
        assert_eq!(MetricId::ALL.len(), 53);
        for ((name, id), (want_name, want_num)) in MetricId::ALL.iter().zip(expected) {
            assert_eq!(*name, want_name);
            assert_eq!(id.0, want_num, "{name}");
            assert!(id.0 < MetricId::TIER_1HZ, "{name}");
        }
        let numbers: HashSet<u16> = MetricId::ALL.iter().map(|(_, id)| id.0).collect();
        assert_eq!(numbers.len(), 53, "a metric number is used twice");
        let names: HashSet<&str> = MetricId::ALL.iter().map(|(name, _)| *name).collect();
        assert_eq!(names.len(), 53, "a metric name is used twice");
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

    #[test]
    fn node_id_unit_manager_distinguishes() {
        use crate::table::Manager;

        let system = NodeId::Unit(UnitRef {
            manager: Manager::System,
            name: "sshd.service".into(),
        });
        let user = NodeId::Unit(UnitRef {
            manager: Manager::User,
            name: "sshd.service".into(),
        });
        let mut map = HashMap::new();
        map.insert(system.clone(), 1);
        map.insert(user.clone(), 2);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&system), Some(&1));
        assert_eq!(map.get(&user), Some(&2));
    }
}

#[cfg(test)]
mod column_tests {
    use std::collections::HashSet;

    use serde::de::value::{Error as ValueError, StrDeserializer};

    use super::*;

    #[test]
    fn column_str_roundtrip() {
        for id in ColumnId::ALL {
            assert_eq!(ColumnId::from_str(id.as_str()), Some(id), "{id:?}");
        }
    }

    #[test]
    fn column_number_roundtrip_and_order() {
        assert_eq!(ColumnId::ALL.len(), ColumnId::COUNT);
        for (i, id) in ColumnId::ALL.into_iter().enumerate() {
            assert_eq!(usize::from(id.number()), i + 1, "{id:?}");
            assert_eq!(ColumnId::from_number(id.number()), Some(id), "{id:?}");
        }
        assert_eq!(ColumnId::Name.number(), 1);
        assert_eq!(ColumnId::Memory.number(), 37);
        assert_eq!(ColumnId::Gpu15.number(), 100);
        assert_eq!(ColumnId::Container.number(), 121);
    }

    #[test]
    fn column_rejects_unknown() {
        for s in ["ws_private", "Memory", "", "gpu_16"] {
            assert_eq!(ColumnId::from_str(s), None, "{s:?}");
        }
        assert_eq!(ColumnId::from_number(0), None);
        assert_eq!(ColumnId::from_number(122), None);
        assert_eq!(ColumnId::from_number(u16::MAX), None);
    }

    #[test]
    fn column_strings_unique() {
        let strings: HashSet<&str> = ColumnId::ALL.iter().map(|id| id.as_str()).collect();
        assert_eq!(strings.len(), 121);
    }

    #[test]
    fn column_bits_fit_128() {
        for id in ColumnId::ALL {
            assert!(id.bit() < 128, "{id:?}");
            assert_eq!(id.bit(), u32::from(id.number()) - 1);
        }
    }

    #[test]
    fn process_series_id_order() {
        let target = GraphTarget::Name(Arc::from("firefox"));
        let cpu = ProcessSeriesId {
            target: target.clone(),
            column: ColumnId::Cpu,
        };
        let memory = ProcessSeriesId {
            target,
            column: ColumnId::Memory,
        };
        let mut ids = vec![memory.clone(), cpu.clone()];
        ids.sort();
        assert_eq!(ids, vec![cpu, memory]);
    }

    #[test]
    fn column_serde_as_string() {
        let bytes = postcard::to_stdvec(&ColumnId::Memory).expect("serialize");
        // postcard writes a string as a length byte and the UTF-8 bytes.
        assert_eq!(bytes, b"\x06memory");
        let back: ColumnId = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(back, ColumnId::Memory);

        let from_str = ColumnId::deserialize(StrDeserializer::<ValueError>::new("memory"));
        assert_eq!(from_str.ok(), Some(ColumnId::Memory));
        assert_eq!(ColumnId::Memory.as_str(), "memory");

        let unknown = ColumnId::deserialize(StrDeserializer::<ValueError>::new("ws_private"));
        assert!(unknown.is_err());
    }
}
