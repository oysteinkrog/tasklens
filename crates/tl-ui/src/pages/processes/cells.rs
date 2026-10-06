//! Process table columns: the registry of every column with its label, picker group, kind,
//! width template, default width, sort rule, heat reference and phase (PROC-022 to PROC-036,
//! PROC-040, PROC-047, R-006, R-007, R-059, R-065, R-191, UI-PRC-005, UI-PRC-007).
//!
//! Labels, aliases and header tooltips are Fluent keys (R-132). Callers translate them with
//! `tr` and cache the text per `strings_version`.

use std::fmt::Write;
use std::hash::BuildHasher;
use std::sync::{Arc, OnceLock};

use gpui_kit::App;
use rustc_hash::FxBuildHasher;
use tl_model::actions::SchedPolicy;
use tl_model::ids::{ColumnId, NodeId};
use tl_model::process::{
    GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessExtras, ProcessRecord,
};
use tl_model::settings::{NetworkUnit, Settings};
use tl_model::system::CpuSet;
use tl_model::table::{ProcessTableView, TableRow};
use tl_model::units::{self, LocalDateTime, NetUnit};
use tl_model::validity::{ProcessField, Validity};

use crate::i18n::tr;
use crate::table::cells::CellOut;
use crate::table::columns::ColumnAlign;
use crate::table::tree_table::RowKind;

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

// ---- Cell values (E22-004) ----

/// Characters of the command line shown in the cell (R-008). Properties shows the full text.
pub const CMDLINE_CELL_CHARS: usize = 512;

/// Words the cells show, translated once per `strings_version`. Each word is an `Arc<str>`, so a
/// new language gives new pointers and every word cell re-formats.
#[derive(Clone, Debug)]
pub struct CellWords {
    pub type_app: Arc<str>,
    pub type_unit: Arc<str>,
    pub type_background: Arc<str>,
    pub type_system: Arc<str>,
    pub type_kernel_thread: Arc<str>,
    pub type_pseudo: Arc<str>,
    /// Indexed by `ProcState as usize`, then "Frozen" last.
    pub state: [Arc<str>; 8],
    pub priority_realtime: Arc<str>,
    pub priority_idle: Arc<str>,
    pub priority_high: Arc<str>,
    pub priority_above_normal: Arc<str>,
    pub priority_normal: Arc<str>,
    pub priority_below_normal: Arc<str>,
    pub priority_low: Arc<str>,
    pub priority_batch: Arc<str>,
    pub affinity_all: Arc<str>,
}

impl CellWords {
    /// Every word through `tr`.
    pub fn new(cx: &App) -> Self {
        Self::from_fn(|key| Arc::from(tr(cx, key).as_ref()))
    }

    /// Every word from `text(fluent_key)`.
    pub fn from_fn(mut text: impl FnMut(&'static str) -> Arc<str>) -> Self {
        Self {
            type_app: text("processes-type-app"),
            type_unit: text("processes-type-unit"),
            type_background: text("processes-type-background"),
            type_system: text("processes-type-system"),
            type_kernel_thread: text("processes-type-kernel-thread"),
            type_pseudo: text("processes-type-pseudo"),
            state: [
                text("processes-state-running"),
                text("processes-state-sleeping"),
                text("processes-state-disk-sleep"),
                text("processes-state-stopped"),
                text("processes-state-zombie"),
                text("processes-state-idle"),
                text("processes-state-dead"),
                text("processes-state-frozen"),
            ],
            priority_realtime: text("processes-priority-realtime"),
            priority_idle: text("processes-priority-idle"),
            priority_high: text("processes-priority-high"),
            priority_above_normal: text("processes-priority-above-normal"),
            priority_normal: text("processes-priority-normal"),
            priority_below_normal: text("processes-priority-below-normal"),
            priority_low: text("processes-priority-low"),
            priority_batch: text("processes-priority-batch"),
            affinity_all: text("processes-affinity-all"),
        }
    }
}

/// What `fill_cell` reads besides the row.
pub struct CellCtx<'a> {
    pub view: &'a ProcessTableView,
    /// `ProcessSnapshot.records`.
    pub records: &'a [ProcessRecord],
    /// `ProcessSnapshot.extras`, index aligned with `records`. Empty gives WarmingUp cells.
    pub extras: &'a [ProcessExtras],
    /// `units.network`, `units.network_auto_scale`.
    pub settings: &'a Settings,
    pub words: &'a CellWords,
    /// Online CPUs, for heat (E22-004).
    pub ncpu: u16,
    /// The online CPU set, for the Affinity "All" form. Empty never reads as All.
    pub online: &'a CpuSet,
    /// MemTotal bytes, for heat.
    pub mem_total: u64,
    /// Sum of GPU VRAM bytes, for heat; 0 when no GPU.
    pub vram_total: u64,
    /// `SystemSnapshot.boot_time_s`, for Start time.
    pub boot_time_s: i64,
    /// `sysconf(_SC_CLK_TCK)`, for the CPU time columns. 0 shows them as Unsupported.
    pub clk_tck: u32,
}

/// The row a cell belongs to. A process row carries its `TableRow` for "PID +N" (PROC-018) and
/// the index of its `ProcessExtras`.
#[derive(Clone, Copy)]
pub enum RowRef<'a> {
    Group(&'a GroupRow, RowKind),
    Record(&'a ProcessRecord, &'a TableRow, RowKind),
}

/// The value of one cell before formatting.
enum Value<'a> {
    /// Empty text, Valid.
    Blank,
    /// A state glyph, no value.
    Glyph(Validity),
    /// Shown as is through `CellOut::arc`.
    Arc(&'a Arc<str>),
    /// The text after the last '/'.
    Basename(&'a Arc<str>),
    /// The first `CMDLINE_CELL_CHARS` characters.
    Cmdline(&'a Arc<str>),
    Percent(f32, Validity),
    Bytes(u64, Validity),
    DiskRate(f32, Validity),
    NetRate(f32, Validity),
    Count(u64, Validity),
    Id(i64),
    Pid {
        pid: u32,
        plus: u32,
    },
    /// Whole seconds.
    Duration(u64),
    /// Unix seconds.
    Time(i64),
    Priority(SchedPolicy, u8, i8),
    Affinity(&'a CpuSet),
}

/// Fills `out` for one cell. Sets `bits` and `validity` always; writes `text` only when
/// `out.want_text` is set and the cell has a value to show, so a cache hit formats nothing
/// (ADR-009). Arc texts go to `out.arc` and never to `text`.
pub fn fill_cell(row: RowRef, col: &ProcColumn, ctx: &CellCtx, out: &mut CellOut) {
    let value = match row {
        RowRef::Group(_, RowKind::GroupHeader) => Value::Blank,
        RowRef::Group(g, _) => group_value(g, col.id, ctx),
        RowRef::Record(r, t, _) => {
            let x = t.record.and_then(|i| ctx.extras.get(i as usize));
            record_value(r, t, x, col.id, ctx)
        }
    };
    write_value(value, ctx, out);
}

/// Splits PID cell text into the PID and the " +N" part drawn in `text_secondary`.
pub fn split_pid_text(text: &str) -> (&str, &str) {
    text.find(' ').map_or((text, ""), |at| text.split_at(at))
}

fn arc_bits(text: &Arc<str>) -> u64 {
    Arc::as_ptr(text).cast::<u8>() as usize as u64
}

const fn severity(v: Validity) -> u8 {
    match v {
        Validity::Valid => 0,
        Validity::Stale => 1,
        Validity::WarmingUp => 2,
        Validity::Restricted => 3,
        Validity::Unsupported => 4,
        Validity::Missing => 5,
        Validity::Failed => 6,
    }
}

/// The worse of two validities, as the engine combines them.
fn worse(a: Validity, b: Validity) -> Validity {
    if severity(b) > severity(a) { b } else { a }
}

fn shows_value(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

fn write_value(value: Value, ctx: &CellCtx, out: &mut CellOut) {
    let (bits, validity) = match &value {
        Value::Blank => (0, Validity::Valid),
        Value::Glyph(v) => (0, *v),
        Value::Arc(a) | Value::Basename(a) | Value::Cmdline(a) => (arc_bits(a), Validity::Valid),
        Value::Percent(p, v) | Value::DiskRate(p, v) | Value::NetRate(p, v) => {
            (u64::from(p.to_bits()), *v)
        }
        Value::Bytes(n, v) | Value::Count(n, v) => (*n, *v),
        Value::Duration(n) => (*n, Validity::Valid),
        Value::Id(n) | Value::Time(n) => (*n as u64, Validity::Valid),
        Value::Pid { pid, plus } => (u64::from(*pid) | u64::from(*plus) << 32, Validity::Valid),
        Value::Priority(policy, rt, nice) => (
            (*policy as u64) << 16 | u64::from(*rt) << 8 | u64::from(*nice as u8),
            Validity::Valid,
        ),
        Value::Affinity(set) => (
            FxBuildHasher.hash_one((set.0, ctx.online.0)),
            Validity::Valid,
        ),
    };
    out.bits = bits;
    out.validity = validity;
    if !shows_value(validity) {
        return;
    }
    if let Value::Arc(a) = value {
        out.arc = Some(Arc::clone(a));
        return;
    }
    if !out.want_text {
        return;
    }
    let s = &mut out.text;
    match value {
        Value::Blank | Value::Glyph(_) | Value::Arc(_) => {}
        Value::Basename(a) => s.push_str(a.rsplit('/').next().unwrap_or(a)),
        Value::Cmdline(a) => {
            let end = a
                .char_indices()
                .nth(CMDLINE_CELL_CHARS)
                .map_or(a.len(), |(i, _)| i);
            s.push_str(&a[..end]);
        }
        Value::Percent(p, _) => units::percent(s, p),
        Value::Bytes(n, _) => units::bytes(s, n),
        Value::DiskRate(bps, _) => units::rate_mib_fixed(s, f64::from(bps)),
        Value::NetRate(bps, _) => {
            let unit = match ctx.settings.units.network {
                NetworkUnit::Bits => NetUnit::Bits,
                NetworkUnit::Bytes => NetUnit::Bytes,
            };
            units::net_rate(
                s,
                f64::from(bps),
                unit,
                ctx.settings.units.network_auto_scale,
            );
        }
        Value::Count(n, _) => units::count(s, n),
        Value::Id(n) => {
            let _ = write!(s, "{n}");
        }
        Value::Pid { pid, plus } => {
            let _ = write!(s, "{pid}");
            if plus > 0 {
                let _ = write!(s, " +{plus}");
            }
        }
        Value::Duration(secs) => units::cpu_time(s, secs),
        Value::Time(unix_s) => write_start_time(s, unix_s),
        Value::Priority(policy, rt, nice) => write_priority(s, ctx.words, policy, rt, nice),
        Value::Affinity(set) => write_affinity(s, ctx.words, set, ctx.online),
    }
}

/// Today "HH:MM:SS", else "YYYY-MM-DD HH:MM", in the system time zone (PROC-027).
fn write_start_time(out: &mut String, unix_s: i64) {
    let Ok(ts) = jiff::Timestamp::from_second(unix_s) else {
        return;
    };
    let tz = jiff::tz::TimeZone::system();
    let z = ts.to_zoned(tz.clone());
    let today = jiff::Timestamp::now().to_zoned(tz).date();
    units::start_time(
        out,
        LocalDateTime {
            year: i32::from(z.year()),
            month: z.month() as u8,
            day: z.day() as u8,
            hour: z.hour() as u8,
            minute: z.minute() as u8,
            second: z.second() as u8,
        },
        z.date() == today,
    );
}

/// PROC-029 Priority: "Realtime (rt N)", "Idle", or the nice class, plus " (batch)".
fn write_priority(out: &mut String, w: &CellWords, policy: SchedPolicy, rt: u8, nice: i8) {
    let word = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            let _ = write!(out, "{} (rt {rt})", w.priority_realtime);
            return;
        }
        SchedPolicy::Idle => &w.priority_idle,
        SchedPolicy::Other | SchedPolicy::Batch => match nice {
            ..=-11 => &w.priority_high,
            -10..=-1 => &w.priority_above_normal,
            0 => &w.priority_normal,
            1..=9 => &w.priority_below_normal,
            _ => &w.priority_low,
        },
    };
    out.push_str(word);
    if policy == SchedPolicy::Batch {
        let _ = write!(out, " ({})", w.priority_batch);
    }
}

/// PROC-029 Affinity: "All" when `set` covers every online CPU, else ranges such as "0-7,16-23".
fn write_affinity(out: &mut String, w: &CellWords, set: &CpuSet, online: &CpuSet) {
    if !online.is_empty() && set.intersection(online) == *online {
        out.push_str(&w.affinity_all);
        return;
    }
    let mut run: Option<(u16, u16)> = None;
    let mut first = true;
    let mut flush = |out: &mut String, (a, b): (u16, u16)| {
        if !first {
            out.push(',');
        }
        first = false;
        let _ = if a == b {
            write!(out, "{a}")
        } else {
            write!(out, "{a}-{b}")
        };
    };
    for cpu in set.iter() {
        run = match run {
            Some((a, b)) if cpu == b + 1 => Some((a, cpu)),
            Some(r) => {
                flush(out, r);
                Some((cpu, cpu))
            }
            None => Some((cpu, cpu)),
        };
    }
    if let Some(r) = run {
        flush(out, r);
    }
}

/// PROC-027 Type of a record (PROC-004 class).
fn record_type<'a>(r: &ProcessRecord, w: &'a CellWords) -> &'a Arc<str> {
    if r.flags.contains(ProcFlags::KERNEL_THREAD) {
        &w.type_kernel_thread
    } else if r.group == GroupKind::Background.key() {
        &w.type_background
    } else if r.group == GroupKind::System.key() || r.group == GroupKind::Ungrouped.key() {
        &w.type_system
    } else {
        &w.type_app
    }
}

fn state_word<'a>(r: &ProcessRecord, x: Option<&ProcessExtras>, w: &'a CellWords) -> &'a Arc<str> {
    if x.is_some_and(|x| x.frozen) {
        return &w.state[7];
    }
    &w.state[match r.state {
        ProcState::Running => 0,
        ProcState::Sleeping => 1,
        ProcState::DiskSleep => 2,
        ProcState::Stopped => 3,
        ProcState::Zombie => 4,
        ProcState::Idle => 5,
        ProcState::Other => 6,
    }]
}

/// Unix seconds of a start time `start_boot_ns` after boot.
fn start_unix_s(ctx: &CellCtx, start_boot_ns: u64) -> i64 {
    ctx.boot_time_s
        .saturating_add(i64::try_from(start_boot_ns / 1_000_000_000).unwrap_or(i64::MAX))
}

fn record_value<'a>(
    r: &'a ProcessRecord,
    t: &TableRow,
    x: Option<&'a ProcessExtras>,
    col: ColumnId,
    ctx: &'a CellCtx,
) -> Value<'a> {
    use ColumnId as C;
    use ProcessField as F;

    let v = |f: F| r.validity.get(f);
    let pair = |a: F, b: F| worse(v(a), v(b));
    let text = |t: Option<&'a Arc<str>>| t.map_or(Value::Blank, Value::Arc);
    // A column read from `ProcessExtras`; WarmingUp until the extras arrive.
    let extra =
        |f: &dyn Fn(&'a ProcessExtras) -> Value<'a>| x.map_or(Value::Glyph(Validity::WarmingUp), f);
    let cpu_time = |ticks: fn(&ProcessExtras) -> u64| {
        if ctx.clk_tck == 0 {
            return Value::Glyph(Validity::Unsupported);
        }
        extra(&|x| Value::Duration(ticks(x) / u64::from(ctx.clk_tck)))
    };

    match col {
        // PROC-027 identity and image
        C::Name => Value::Arc(&r.meta.name),
        C::NameDisplay if r.meta.app_id.is_some() => Value::Arc(&r.meta.name),
        C::NameDisplay => Value::Arc(&r.meta.comm),
        C::ImageName => r
            .meta
            .exe
            .as_ref()
            .map_or(Value::Glyph(Validity::Restricted), Value::Basename),
        C::ImagePath => r
            .meta
            .exe
            .as_ref()
            .map_or(Value::Glyph(Validity::Restricted), Value::Arc),
        C::Cmdline => r
            .meta
            .cmdline
            .as_ref()
            .map_or(Value::Glyph(Validity::Restricted), Value::Cmdline),
        C::Description => text(r.meta.description.as_ref()),
        C::Type => Value::Arc(record_type(r, ctx.words)),
        C::StartTime => extra(&|x| Value::Time(start_unix_s(ctx, x.start_boot_ns))),
        // PROC-028 ownership
        C::User => Value::Arc(&r.meta.user),
        C::Pid => Value::Pid {
            pid: r.key.pid,
            plus: if t.has_children && !t.expanded {
                t.descendants
            } else {
                0
            },
        },
        C::Ppid => Value::Id(r.ppid.into()),
        // PROC-029 CPU and scheduling
        C::Cpu => Value::Percent(r.cpu_pct, v(F::Cpu)),
        C::CpuKernel => extra(&|x| Value::Percent(x.cpu_kernel_pct, v(F::Cpu))),
        C::CpuUser => extra(&|x| Value::Percent(x.cpu_user_pct, v(F::Cpu))),
        C::CpuRelative => extra(&|x| Value::Percent(x.cpu_relative_pct, v(F::Cpu))),
        C::CpuTime => cpu_time(|x| x.utime_ticks + x.stime_ticks),
        C::CpuTimeKernel => cpu_time(|x| x.stime_ticks),
        C::CpuTimeUser => cpu_time(|x| x.utime_ticks),
        C::Priority => extra(&|x| Value::Priority(x.policy, x.rt_prio, r.nice)),
        C::Affinity => extra(&|x| Value::Affinity(&x.affinity)),
        // PROC-030 memory
        C::Memory => Value::Bytes(r.rss_anon_bytes, v(F::RssAnon)),
        C::WsTotal => Value::Bytes(r.rss_bytes, v(F::Rss)),
        C::WsPeak => extra(&|x| Value::Bytes(x.rss_peak_bytes, Validity::Valid)),
        C::WsShared => extra(&|x| Value::Bytes(x.rss_shared_bytes, Validity::Valid)),
        C::Commit => extra(&|x| Value::Bytes(x.commit_bytes, Validity::Valid)),
        C::Vsize => extra(&|x| Value::Bytes(x.vsize_bytes, Validity::Valid)),
        C::VsizePeak => extra(&|x| Value::Bytes(x.vsize_peak_bytes, Validity::Valid)),
        C::PageFaults => extra(&|x| Value::Count(x.page_faults, Validity::Valid)),
        C::PageFaultsDelta => extra(&|x| Value::Count(x.page_faults_delta, Validity::Valid)),
        C::MajorFaults => extra(&|x| Value::Count(x.major_faults, Validity::Valid)),
        C::Swap => Value::Bytes(r.swap_bytes, v(F::Swap)),
        C::Pss => Value::Bytes(r.pss_bytes, v(F::Pss)),
        C::Uss => Value::Bytes(r.uss_bytes, v(F::Uss)),
        // PROC-031 disk rates
        C::Disk => Value::DiskRate(r.io_read_bps + r.io_write_bps, pair(F::IoRead, F::IoWrite)),
        C::DiskReadRate => Value::DiskRate(r.io_read_bps, v(F::IoRead)),
        C::DiskWriteRate => extra(&|x| Value::DiskRate(x.disk_write_raw_bps, v(F::IoWrite))),
        C::DiskTotalRate => extra(&|x| {
            Value::DiskRate(
                r.io_read_bps + x.disk_write_raw_bps,
                pair(F::IoRead, F::IoWrite),
            )
        }),
        // PROC-032 network
        C::Network => Value::NetRate(r.net_rx_bps + r.net_tx_bps, pair(F::NetRx, F::NetTx)),
        C::NetSendRate => Value::NetRate(r.net_tx_bps, v(F::NetTx)),
        C::NetRecvRate => Value::NetRate(r.net_rx_bps, v(F::NetRx)),
        // PROC-033 GPU
        C::Gpu => Value::Percent(r.gpu_util_pct, v(F::GpuUtil)),
        C::GpuMemDedicated => Value::Bytes(r.gpu_mem_bytes, v(F::GpuMem)),
        // PROC-035 object counts
        C::Handles => extra(&|x| {
            x.handles.map_or(Value::Glyph(Validity::Restricted), |h| {
                Value::Count(h.into(), Validity::Valid)
            })
        }),
        C::Threads => Value::Count(r.threads.into(), v(F::Threads)),
        // PROC-036 Linux extras
        C::State => Value::Arc(state_word(r, x, ctx.words)),
        C::Nice => Value::Id(r.nice.into()),
        C::Unit => extra(&|x| {
            x.unit
                .as_ref()
                .map_or(Value::Blank, |u| Value::Arc(&u.name))
        }),
        C::Cgroup => text(r.meta.cgroup.as_ref()),
        C::AppId => text(r.meta.app_id.as_ref()),
        // No phase-1 source: P2 to P4 columns read `ProcessExtrasP2` and later collectors.
        _ => Value::Glyph(Validity::Unsupported),
    }
}

/// The member record App rows read their text columns from: the earliest started member,
/// which is the app's primary process in practice. `records` is sorted by `ProcessKey`, which
/// orders by start time first.
fn first_member<'a>(g: &GroupRow, ctx: &CellCtx<'a>) -> Option<(usize, &'a ProcessRecord)> {
    ctx.records
        .iter()
        .enumerate()
        .find(|(_, r)| r.group == g.key)
}

fn group_value<'a>(g: &'a GroupRow, col: ColumnId, ctx: &'a CellCtx) -> Value<'a> {
    use ColumnId as C;
    use ProcessField as F;

    let v = |f: F| g.validity.get(f);
    let pair = |a: F, b: F| worse(v(a), v(b));
    let member = || first_member(g, ctx);
    let w = ctx.words;

    match col {
        // No GroupRow or model source yet: the desktop file path of an app, and the member
        // text of a unit (follow-up model bead).
        C::ImagePath if g.kind == GroupRowKind::App => Value::Glyph(Validity::Unsupported),
        C::ImagePath | C::User | C::Description | C::Cmdline | C::StartTime
            if g.kind == GroupRowKind::Unit =>
        {
            Value::Glyph(Validity::Unsupported)
        }
        C::User | C::Description | C::Cmdline | C::StartTime if g.kind != GroupRowKind::App => {
            Value::Blank
        }
        C::Name | C::NameDisplay => Value::Arc(&g.name),
        C::Type => Value::Arc(match g.kind {
            GroupRowKind::App => &w.type_app,
            GroupRowKind::Unit => &w.type_unit,
            GroupRowKind::KernelThreads => &w.type_kernel_thread,
            GroupRowKind::Pseudo => &w.type_pseudo,
        }),
        C::User => member().map_or(Value::Blank, |(_, r)| Value::Arc(&r.meta.user)),
        C::Description => member()
            .and_then(|(_, r)| r.meta.description.as_ref())
            .map_or(Value::Blank, Value::Arc),
        C::Cmdline => member()
            .and_then(|(_, r)| r.meta.cmdline.as_ref())
            .map_or(Value::Blank, Value::Cmdline),
        C::StartTime => member()
            .and_then(|(i, _)| ctx.extras.get(i))
            .map_or(Value::Blank, |x| {
                Value::Time(start_unix_s(ctx, x.start_boot_ns))
            }),
        C::Cpu => Value::Percent(g.cpu_pct, v(F::Cpu)),
        C::Memory => Value::Bytes(g.memory_bytes, v(F::RssAnon)),
        C::WsTotal => Value::Bytes(g.rss_bytes, v(F::Rss)),
        C::Swap => Value::Bytes(g.swap_bytes, v(F::Swap)),
        C::Pss => Value::Bytes(g.pss_bytes, v(F::Pss)),
        C::Uss => Value::Bytes(g.uss_bytes, v(F::Uss)),
        C::Disk | C::DiskTotalRate => Value::DiskRate(g.disk_bps, pair(F::IoRead, F::IoWrite)),
        C::DiskReadRate => Value::DiskRate(g.io_read_bps, v(F::IoRead)),
        C::DiskWriteRate => Value::DiskRate(g.io_write_bps, v(F::IoWrite)),
        C::Network => Value::NetRate(g.net_bps, pair(F::NetRx, F::NetTx)),
        C::NetSendRate => Value::NetRate(g.net_tx_bps, v(F::NetTx)),
        C::NetRecvRate => Value::NetRate(g.net_rx_bps, v(F::NetRx)),
        C::Gpu => Value::Percent(g.gpu_pct, v(F::GpuUtil)),
        C::GpuMemDedicated => Value::Bytes(g.gpu_mem_bytes, v(F::GpuMem)),
        C::Threads => Value::Count(g.threads.into(), v(F::Threads)),
        C::Unit => match &g.node {
            NodeId::Unit(u) => Value::Arc(&u.name),
            _ => Value::Blank,
        },
        C::AppId => match &g.class {
            GroupKind::App { app_id } if g.kind == GroupRowKind::App => Value::Arc(app_id),
            _ => Value::Blank,
        },
        // PID, Parent PID and Image name are blank on group rows (PROC-008,
        // PROC-018). The GroupRow has no sums for the other columns.
        _ => Value::Blank,
    }
}
