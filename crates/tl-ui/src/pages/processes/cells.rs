//! Process table columns: the registry of every column with its label, picker group, kind,
//! width template, default width, sort rule, heat reference and phase (PROC-022 to PROC-036,
//! PROC-040, PROC-047, R-006, R-007, R-059, R-065, R-191, UI-PRC-005, UI-PRC-007).
//!
//! Labels, aliases and header tooltips are Fluent keys (R-132). Callers translate them with
//! `tr` and cache the text per `strings_version`.

use std::sync::OnceLock;

use tl_model::ids::ColumnId;

use crate::table::columns::ColumnAlign;

/// What a process cell holds. The cell formatter dispatches on it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ProcColumnKind {
    Text,
    IntegerId,
    Percent,
    MemorySize,
    DiskRate,
    NetRate,
    Count,
    BytesCumulative,
    Duration,
    Time,
    PowerClass,
}

/// The heat scale a cell is tinted against (PROC-040, R-059). `None` is never tinted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HeatRef {
    CpuMachine,
    CpuOneCore,
    Memory,
    DiskIo,
    Network,
    GpuPercent,
    Vram,
    Power,
    None,
}

/// Column picker heading (PROC-024, R-191).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PickerGroup {
    Image,
    Identity,
    Security,
    Scheduling,
    Cpu,
    Memory,
    Io,
    Disk,
    Network,
    Gpu,
    Power,
    Objects,
    Linux,
}

impl PickerGroup {
    /// The headings in picker order.
    pub const ALL: [PickerGroup; 13] = [
        Self::Image,
        Self::Identity,
        Self::Security,
        Self::Scheduling,
        Self::Cpu,
        Self::Memory,
        Self::Io,
        Self::Disk,
        Self::Network,
        Self::Gpu,
        Self::Power,
        Self::Objects,
        Self::Linux,
    ];

    /// Fluent key of the heading text.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Image => "processes-group-image",
            Self::Identity => "processes-group-identity",
            Self::Security => "processes-group-security",
            Self::Scheduling => "processes-group-scheduling",
            Self::Cpu => "processes-group-cpu",
            Self::Memory => "processes-group-memory",
            Self::Io => "processes-group-io",
            Self::Disk => "processes-group-disk",
            Self::Network => "processes-group-network",
            Self::Gpu => "processes-group-gpu",
            Self::Power => "processes-group-power",
            Self::Objects => "processes-group-objects",
            Self::Linux => "processes-group-linux",
        }
    }
}

/// One process-table column.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ProcColumn {
    /// The R-007 string id.
    pub id: ColumnId,
    /// Fluent key of header line 1 and the picker label.
    pub label: &'static str,
    /// Fluent key of the picker full name, shown as `<alias> - "<label>"` (R-065).
    pub alias: Option<&'static str>,
    pub group: PickerGroup,
    pub kind: ProcColumnKind,
    /// PROC-023 width template. Network rates hold the bits form; read it through `template`.
    pub template: Option<&'static str>,
    /// Logical px at 100%. `None`: max(minimum, header label width + 16), measured at run time.
    pub default_width: Option<f32>,
    /// False for the columns measured only for visible rows (PROC-047).
    pub sortable: bool,
    pub heat: HeatRef,
    /// 1..=4. Columns of a later phase are not offered.
    pub phase: u8,
    /// Fluent key of the header tooltip (UI-PRC-005). GPU N tooltips are built at run time.
    pub header_tooltip: Option<&'static str>,
}

impl ProcColumn {
    /// `Text` and `PowerClass` are left aligned, every other kind right.
    pub fn align(&self) -> ColumnAlign {
        match self.kind {
            ProcColumnKind::Text | ProcColumnKind::PowerClass => ColumnAlign::Left,
            _ => ColumnAlign::Right,
        }
    }

    /// The width template for the current `units.network`: `net_bytes` is true for "bytes".
    pub fn template(&self, net_bytes: bool) -> Option<&'static str> {
        match self.kind {
            ProcColumnKind::NetRate if net_bytes => Some(NET_BYTES_TEMPLATE),
            _ => self.template,
        }
    }
}

/// `[columns.processes]` default `order` and `visible`, canonical table (a).
pub const DEFAULT_VISIBLE: [&str; 9] = [
    "name",
    "user",
    "pid",
    "cpu",
    "memory",
    "disk",
    "network",
    "gpu",
    "description",
];

const NET_BITS_TEMPLATE: &str = "xxx Mbps";
const NET_BYTES_TEMPLATE: &str = "xxx MiB/s";
const VISIBLE_ROWS_TOOLTIP: &str = "processes-column-visible-rows-tooltip";

/// The entry for `id`. Every `ColumnId` has one.
pub fn process_column(id: ColumnId) -> &'static ProcColumn {
    static INDEX: OnceLock<[u8; ColumnId::COUNT + 1]> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        let mut index = [u8::MAX; ColumnId::COUNT + 1];
        for (i, col) in PROCESS_COLUMNS.iter().enumerate() {
            index[col.id as usize] = i as u8;
        }
        index
    });
    &PROCESS_COLUMNS[usize::from(index[id as usize])]
}

/// PROC-023 templates by kind. PID, Parent PID and SID are the only ids with one.
const fn template(id: ColumnId, kind: ProcColumnKind) -> Option<&'static str> {
    match kind {
        ProcColumnKind::IntegerId => match id {
            ColumnId::Pid | ColumnId::Ppid | ColumnId::Sid => Some("999999 +99"),
            _ => None,
        },
        ProcColumnKind::Percent => Some("100.0%"),
        ProcColumnKind::NetRate => Some(NET_BITS_TEMPLATE),
        ProcColumnKind::DiskRate => Some("xxx GiB/s"),
        ProcColumnKind::MemorySize => Some("999 MiB"),
        ProcColumnKind::Count => Some("999999"),
        ProcColumnKind::Duration => Some("999:59:59"),
        ProcColumnKind::Text
        | ProcColumnKind::BytesCumulative
        | ProcColumnKind::Time
        | ProcColumnKind::PowerClass => None,
    }
}

/// R-006 default widths.
const fn default_width(id: ColumnId) -> Option<f32> {
    match id {
        ColumnId::Name => Some(248.0),
        ColumnId::User => Some(92.0),
        ColumnId::Pid => Some(96.0),
        ColumnId::Cpu => Some(63.0),
        ColumnId::Memory => Some(92.0),
        ColumnId::Disk => Some(82.0),
        ColumnId::Network => Some(92.0),
        ColumnId::Gpu => Some(72.0),
        ColumnId::Description => Some(342.0),
        ColumnId::Power => Some(101.0),
        _ => None,
    }
}

/// R-065 picker aliases.
const fn alias(id: ColumnId) -> Option<&'static str> {
    match id {
        ColumnId::Memory => Some("processes-column-memory-alias"),
        ColumnId::GpuMemDedicated => Some("processes-column-gpu-mem-dedicated-alias"),
        _ => None,
    }
}

/// PROC-047: these are read only for visible rows, so they cannot be sorted.
const fn sortable(id: ColumnId) -> bool {
    !matches!(
        id,
        ColumnId::Pss
            | ColumnId::Uss
            | ColumnId::Cycles
            | ColumnId::CyclesDelta
            | ColumnId::CtxSwitches
            | ColumnId::CtxSwitchesDelta
    )
}

/// UI-PRC-005 header tooltips.
const fn header_tooltip(id: ColumnId) -> Option<&'static str> {
    if !sortable(id) {
        return Some(VISIBLE_ROWS_TOOLTIP);
    }
    match id {
        ColumnId::Memory => Some("processes-column-memory-tooltip"),
        ColumnId::WsTotal => Some("processes-column-ws-total-tooltip"),
        ColumnId::Network => Some("processes-column-network-tooltip"),
        ColumnId::Handles => Some("processes-column-handles-tooltip"),
        _ => None,
    }
}

const fn col(
    id: ColumnId,
    label: &'static str,
    group: PickerGroup,
    kind: ProcColumnKind,
    heat: HeatRef,
    phase: u8,
) -> ProcColumn {
    ProcColumn {
        id,
        label,
        alias: alias(id),
        group,
        kind,
        template: template(id, kind),
        default_width: default_width(id),
        sortable: sortable(id),
        heat,
        phase,
        header_tooltip: header_tooltip(id),
    }
}

use {ColumnId as Id, HeatRef as Heat, PickerGroup as Group, ProcColumnKind as Kind};

/// Every process column, Name first, then by picker group in picker order.
#[rustfmt::skip]
pub static PROCESS_COLUMNS: &[ProcColumn] = &[
    col(Id::Name, "processes-column-name", Group::Image, Kind::Text, Heat::None, 1),
    col(Id::NameDisplay, "processes-column-name-display", Group::Image, Kind::Text, Heat::None, 1),
    col(Id::ImageName, "processes-column-image-name", Group::Image, Kind::Text, Heat::None, 1),
    col(Id::ImagePath, "processes-column-image-path", Group::Image, Kind::Text, Heat::None, 1),
    col(Id::Cmdline, "processes-column-cmdline", Group::Image, Kind::Text, Heat::None, 1),
    col(Id::Description, "processes-column-description", Group::Image, Kind::Text, Heat::None, 1),
    col(Id::Type, "processes-column-type", Group::Image, Kind::Text, Heat::None, 1),
    col(Id::StartTime, "processes-column-start-time", Group::Image, Kind::Time, Heat::None, 1),
    col(Id::Version, "processes-column-version", Group::Image, Kind::Text, Heat::None, 2),
    col(Id::Timestamp, "processes-column-timestamp", Group::Image, Kind::Time, Heat::None, 2),
    col(Id::FileMtime, "processes-column-file-mtime", Group::Image, Kind::Time, Heat::None, 2),
    col(Id::FileSize, "processes-column-file-size", Group::Image, Kind::MemorySize, Heat::Memory, 2),
    col(Id::Company, "processes-column-company", Group::Image, Kind::Text, Heat::None, 2),
    col(Id::Signer, "processes-column-signer", Group::Image, Kind::Text, Heat::None, 2),
    col(Id::Signature, "processes-column-signature", Group::Image, Kind::Text, Heat::None, 2),
    col(Id::Architecture, "processes-column-architecture", Group::Image, Kind::Text, Heat::None, 2),
    col(Id::User, "processes-column-user", Group::Identity, Kind::Text, Heat::None, 1),
    col(Id::Pid, "processes-column-pid", Group::Identity, Kind::IntegerId, Heat::None, 1),
    col(Id::Ppid, "processes-column-ppid", Group::Identity, Kind::IntegerId, Heat::None, 1),
    col(Id::Session, "processes-column-session", Group::Identity, Kind::Text, Heat::None, 2),
    col(Id::SecurityLabel, "processes-column-security-label", Group::Security, Kind::Text, Heat::None, 2),
    col(Id::Elevation, "processes-column-elevation", Group::Security, Kind::Text, Heat::None, 2),
    col(Id::Sandbox, "processes-column-sandbox", Group::Security, Kind::Text, Heat::None, 2),
    col(Id::Priority, "processes-column-priority", Group::Scheduling, Kind::Text, Heat::None, 1),
    col(Id::Affinity, "processes-column-affinity", Group::Scheduling, Kind::Text, Heat::None, 1),
    col(Id::Cpu, "processes-column-cpu", Group::Cpu, Kind::Percent, Heat::CpuMachine, 1),
    col(Id::CpuKernel, "processes-column-cpu-kernel", Group::Cpu, Kind::Percent, Heat::CpuMachine, 1),
    col(Id::CpuUser, "processes-column-cpu-user", Group::Cpu, Kind::Percent, Heat::CpuMachine, 1),
    col(Id::CpuRelative, "processes-column-cpu-relative", Group::Cpu, Kind::Percent, Heat::CpuOneCore, 1),
    col(Id::CpuTime, "processes-column-cpu-time", Group::Cpu, Kind::Duration, Heat::None, 1),
    col(Id::CpuTimeKernel, "processes-column-cpu-time-kernel", Group::Cpu, Kind::Duration, Heat::None, 1),
    col(Id::CpuTimeUser, "processes-column-cpu-time-user", Group::Cpu, Kind::Duration, Heat::None, 1),
    col(Id::Cycles, "processes-column-cycles", Group::Cpu, Kind::Count, Heat::None, 4),
    col(Id::CyclesDelta, "processes-column-cycles-delta", Group::Cpu, Kind::Count, Heat::None, 4),
    col(Id::CtxSwitches, "processes-column-ctx-switches", Group::Cpu, Kind::Count, Heat::None, 2),
    col(Id::CtxSwitchesDelta, "processes-column-ctx-switches-delta", Group::Cpu, Kind::Count, Heat::None, 2),
    col(Id::Memory, "processes-column-memory", Group::Memory, Kind::MemorySize, Heat::Memory, 1),
    col(Id::WsTotal, "processes-column-ws-total", Group::Memory, Kind::MemorySize, Heat::Memory, 1),
    col(Id::WsPeak, "processes-column-ws-peak", Group::Memory, Kind::MemorySize, Heat::Memory, 1),
    col(Id::WsShared, "processes-column-ws-shared", Group::Memory, Kind::MemorySize, Heat::Memory, 1),
    col(Id::Commit, "processes-column-commit", Group::Memory, Kind::MemorySize, Heat::Memory, 1),
    col(Id::CommitPeak, "processes-column-commit-peak", Group::Memory, Kind::MemorySize, Heat::Memory, 2),
    col(Id::CommitDelta, "processes-column-commit-delta", Group::Memory, Kind::MemorySize, Heat::Memory, 2),
    col(Id::Vsize, "processes-column-vsize", Group::Memory, Kind::MemorySize, Heat::Memory, 1),
    col(Id::VsizePeak, "processes-column-vsize-peak", Group::Memory, Kind::MemorySize, Heat::Memory, 1),
    col(Id::PageTables, "processes-column-page-tables", Group::Memory, Kind::MemorySize, Heat::Memory, 2),
    col(Id::PageFaults, "processes-column-page-faults", Group::Memory, Kind::Count, Heat::None, 1),
    col(Id::PageFaultsDelta, "processes-column-page-faults-delta", Group::Memory, Kind::Count, Heat::None, 1),
    col(Id::MajorFaults, "processes-column-major-faults", Group::Memory, Kind::Count, Heat::None, 2),
    col(Id::MajorFaultsDelta, "processes-column-major-faults-delta", Group::Memory, Kind::Count, Heat::None, 2),
    col(Id::Swap, "processes-column-swap", Group::Memory, Kind::MemorySize, Heat::Memory, 1),
    col(Id::Pss, "processes-column-pss", Group::Memory, Kind::MemorySize, Heat::Memory, 2),
    col(Id::Uss, "processes-column-uss", Group::Memory, Kind::MemorySize, Heat::Memory, 2),
    col(Id::IoReadBytes, "processes-column-io-read-bytes", Group::Io, Kind::BytesCumulative, Heat::None, 2),
    col(Id::IoWriteBytes, "processes-column-io-write-bytes", Group::Io, Kind::BytesCumulative, Heat::None, 2),
    col(Id::IoReadDelta, "processes-column-io-read-delta", Group::Io, Kind::BytesCumulative, Heat::None, 2),
    col(Id::IoWriteDelta, "processes-column-io-write-delta", Group::Io, Kind::BytesCumulative, Heat::None, 2),
    col(Id::IoReadRate, "processes-column-io-read-rate", Group::Io, Kind::DiskRate, Heat::DiskIo, 2),
    col(Id::IoWriteRate, "processes-column-io-write-rate", Group::Io, Kind::DiskRate, Heat::DiskIo, 2),
    col(Id::IoReadOps, "processes-column-io-read-ops", Group::Io, Kind::Count, Heat::None, 2),
    col(Id::IoWriteOps, "processes-column-io-write-ops", Group::Io, Kind::Count, Heat::None, 2),
    col(Id::Disk, "processes-column-disk", Group::Disk, Kind::DiskRate, Heat::DiskIo, 1),
    col(Id::DiskReadRate, "processes-column-disk-read-rate", Group::Disk, Kind::DiskRate, Heat::DiskIo, 1),
    col(Id::DiskWriteRate, "processes-column-disk-write-rate", Group::Disk, Kind::DiskRate, Heat::DiskIo, 1),
    col(Id::DiskTotalRate, "processes-column-disk-total-rate", Group::Disk, Kind::DiskRate, Heat::DiskIo, 1),
    col(Id::DiskReadBytes, "processes-column-disk-read-bytes", Group::Disk, Kind::BytesCumulative, Heat::None, 2),
    col(Id::DiskWriteBytes, "processes-column-disk-write-bytes", Group::Disk, Kind::BytesCumulative, Heat::None, 2),
    col(Id::DiskTotalBytes, "processes-column-disk-total-bytes", Group::Disk, Kind::BytesCumulative, Heat::None, 2),
    col(Id::DiskReadDelta, "processes-column-disk-read-delta", Group::Disk, Kind::BytesCumulative, Heat::None, 2),
    col(Id::DiskWriteDelta, "processes-column-disk-write-delta", Group::Disk, Kind::BytesCumulative, Heat::None, 2),
    col(Id::DiskTotalDelta, "processes-column-disk-total-delta", Group::Disk, Kind::BytesCumulative, Heat::None, 2),
    col(Id::DiskReads, "processes-column-disk-reads", Group::Disk, Kind::Count, Heat::None, 2),
    col(Id::DiskWrites, "processes-column-disk-writes", Group::Disk, Kind::Count, Heat::None, 2),
    col(Id::DiskReadsDelta, "processes-column-disk-reads-delta", Group::Disk, Kind::Count, Heat::None, 2),
    col(Id::DiskWritesDelta, "processes-column-disk-writes-delta", Group::Disk, Kind::Count, Heat::None, 2),
    col(Id::Network, "processes-column-network", Group::Network, Kind::NetRate, Heat::Network, 1),
    col(Id::NetSendRate, "processes-column-net-send-rate", Group::Network, Kind::NetRate, Heat::Network, 1),
    col(Id::NetRecvRate, "processes-column-net-recv-rate", Group::Network, Kind::NetRate, Heat::Network, 1),
    col(Id::Gpu, "processes-column-gpu", Group::Gpu, Kind::Percent, Heat::GpuPercent, 1),
    col(Id::GpuMemDedicated, "processes-column-gpu-mem-dedicated", Group::Gpu, Kind::MemorySize, Heat::Vram, 1),
    col(Id::GpuMemShared, "processes-column-gpu-mem-shared", Group::Gpu, Kind::MemorySize, Heat::Memory, 2),
    col(Id::GpuEngine, "processes-column-gpu-engine", Group::Gpu, Kind::Text, Heat::None, 2),
    col(Id::GpuEncode, "processes-column-gpu-encode", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::GpuDecode, "processes-column-gpu-decode", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu0, "processes-column-gpu-0", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu1, "processes-column-gpu-1", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu2, "processes-column-gpu-2", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu3, "processes-column-gpu-3", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu4, "processes-column-gpu-4", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu5, "processes-column-gpu-5", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu6, "processes-column-gpu-6", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu7, "processes-column-gpu-7", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu8, "processes-column-gpu-8", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu9, "processes-column-gpu-9", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu10, "processes-column-gpu-10", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu11, "processes-column-gpu-11", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu12, "processes-column-gpu-12", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu13, "processes-column-gpu-13", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu14, "processes-column-gpu-14", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Gpu15, "processes-column-gpu-15", Group::Gpu, Kind::Percent, Heat::GpuPercent, 2),
    col(Id::Power, "processes-column-power", Group::Power, Kind::PowerClass, Heat::Power, 2),
    col(Id::PowerTrend, "processes-column-power-trend", Group::Power, Kind::PowerClass, Heat::Power, 2),
    col(Id::Handles, "processes-column-handles", Group::Objects, Kind::Count, Heat::None, 1),
    col(Id::Threads, "processes-column-threads", Group::Objects, Kind::Count, Heat::None, 1),
    col(Id::Windows, "processes-column-windows", Group::Objects, Kind::Count, Heat::None, 3),
    col(Id::X11Resources, "processes-column-x11-resources", Group::Objects, Kind::Count, Heat::None, 4),
    col(Id::State, "processes-column-state", Group::Linux, Kind::Text, Heat::None, 1),
    col(Id::Nice, "processes-column-nice", Group::Linux, Kind::IntegerId, Heat::None, 1),
    col(Id::SchedPolicy, "processes-column-sched-policy", Group::Linux, Kind::Text, Heat::None, 2),
    col(Id::IoPriority, "processes-column-io-priority", Group::Linux, Kind::Text, Heat::None, 2),
    col(Id::Unit, "processes-column-unit", Group::Linux, Kind::Text, Heat::None, 1),
    col(Id::Cgroup, "processes-column-cgroup", Group::Linux, Kind::Text, Heat::None, 2),
    col(Id::AppId, "processes-column-app-id", Group::Linux, Kind::Text, Heat::None, 2),
    col(Id::OomScore, "processes-column-oom-score", Group::Linux, Kind::IntegerId, Heat::None, 2),
    col(Id::OomScoreAdj, "processes-column-oom-score-adj", Group::Linux, Kind::IntegerId, Heat::None, 2),
    col(Id::Tty, "processes-column-tty", Group::Linux, Kind::Text, Heat::None, 2),
    col(Id::Cwd, "processes-column-cwd", Group::Linux, Kind::Text, Heat::None, 2),
    col(Id::Pgrp, "processes-column-pgrp", Group::Linux, Kind::IntegerId, Heat::None, 4),
    col(Id::Sid, "processes-column-sid", Group::Linux, Kind::IntegerId, Heat::None, 4),
    col(Id::LastCpu, "processes-column-last-cpu", Group::Linux, Kind::IntegerId, Heat::None, 4),
    col(Id::Container, "processes-column-container", Group::Linux, Kind::Text, Heat::None, 4),
];
