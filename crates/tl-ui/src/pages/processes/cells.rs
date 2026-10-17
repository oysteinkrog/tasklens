//! Process table columns: the registry of every column with its label, picker group, kind,
//! width template, default width, sort rule, heat reference and phase (PROC-022 to PROC-036,
//! PROC-040, PROC-047, R-006, R-007, R-059, R-065, R-191, UI-PRC-005, UI-PRC-007).
//!
//! Labels, aliases and header tooltips are Fluent keys (R-132). Callers translate them with
//! `tr` and cache the text per `strings_version`.

use std::fmt::Write;
use std::hash::BuildHasher;
use std::sync::{Arc, OnceLock};

use gpui_kit::{App, Hsla, SharedString};
use rustc_hash::{FxBuildHasher, FxHashMap};
use tl_model::actions::SchedPolicy;
use tl_model::ids::{ColumnId, GroupKey, NodeId, PseudoKind};
use tl_model::process::{
    GroupKind, GroupRow, GroupRowKind, PackageStatus, ProcFlags, ProcState, ProcessExtras,
    ProcessExtrasP2, ProcessRecord,
};
use tl_model::settings::{NetworkUnit, Settings};
use tl_model::system::{CpuSet, Metric, ZramTotals};
use tl_model::table::{ProcessTableView, TableRow, UnitRef};
use tl_model::time::TimeNs;
use tl_model::units::{self, LocalDateTime, NetUnit};
use tl_model::validity::{ProcessField, Validity};

use crate::components::motion::{M_SLOW, ease_out_cubic};
use crate::components::validity::copy_text;
use crate::i18n::{tr, tr_en};
use crate::pages::performance::details::Template;
use crate::table::cells::{CellOut, CellSuffix};
use crate::table::columns::ColumnAlign;
use crate::table::tree_table::RowKind;
use crate::theme::TlTheme;

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
    /// Power usage classes (PROC-034), lowest first.
    pub power: [Arc<str>; 5],
    /// Package status words (PROC-027), in `PackageStatus` declaration order.
    pub package_status: [Arc<str>; 8],
    /// Column labels, index aligned with [`PROCESS_COLUMNS`], for the copy commands.
    pub labels: Arc<[Arc<str>]>,
    /// Copy details labels that are not column labels: "Private bytes", "Working set".
    pub copy_private_bytes: Arc<str>,
    pub copy_working_set: Arc<str>,
    /// The pseudo row names (PROC-012), shown in place of the engine's text.
    pub pseudo_interrupts: Arc<str>,
    pub pseudo_memory_compression: Arc<str>,
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
            power: [
                text("processes-power-very-low"),
                text("processes-power-low"),
                text("processes-power-moderate"),
                text("processes-power-high"),
                text("processes-power-very-high"),
            ],
            package_status: [
                text("processes-package-status-packaged"),
                text("processes-package-status-modified"),
                text("processes-package-status-not-packaged"),
                text("processes-package-status-deleted"),
                text("processes-package-status-flatpak"),
                text("processes-package-status-snap"),
                text("processes-package-status-appimage"),
                text("processes-package-status-wine"),
            ],
            labels: PROCESS_COLUMNS.iter().map(|c| text(c.label)).collect(),
            copy_private_bytes: text("processes-copy-private-bytes"),
            copy_working_set: text("processes-copy-working-set"),
            pseudo_interrupts: text("processes-pseudo-interrupts"),
            pseudo_memory_compression: text("processes-pseudo-memory-compression"),
        }
    }

    /// The label of column `id`.
    pub fn label(&self, id: ColumnId) -> &str {
        PROCESS_COLUMNS
            .iter()
            .position(|c| c.id == id)
            .and_then(|i| self.labels.get(i))
            .map_or("", |l| l.as_ref())
    }

    /// The translated name of a group row: the pseudo rows by kind, every other row as the
    /// engine names it.
    pub fn group_name<'a>(&'a self, g: &'a GroupRow) -> &'a Arc<str> {
        match g.node {
            NodeId::Pseudo(PseudoKind::Interrupts) => &self.pseudo_interrupts,
            NodeId::Pseudo(PseudoKind::MemoryCompression) => &self.pseudo_memory_compression,
            _ => &g.name,
        }
    }
}

/// The earliest started member record of each app group and each unit, so app and unit rows
/// read their text columns with one lookup. Rebuilt only when the view or the records change.
#[derive(Default)]
pub struct GroupMembers {
    /// `view.header.version`, `records` and `extras` address and length of the last build.
    built_for: Option<(u64, usize, usize, usize, usize)>,
    apps: FxHashMap<GroupKey, u32>,
    units: FxHashMap<UnitRef, u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    builds: u64,
}

impl GroupMembers {
    /// Rebuilds the index when `view` or `records` changed since the last call; otherwise does
    /// nothing. Call once per frame before filling cells.
    pub fn refresh(
        &mut self,
        view: &ProcessTableView,
        records: &[ProcessRecord],
        extras: &[ProcessExtras],
    ) {
        let key = (
            view.header.version,
            records.as_ptr() as usize,
            records.len(),
            extras.as_ptr() as usize,
            extras.len(),
        );
        if self.built_for == Some(key) {
            return;
        }
        self.built_for = Some(key);
        self.apps.clear();
        self.units.clear();
        // `records` is sorted by `ProcessKey`, which orders by start time first, so the first
        // record seen per group is the earliest started. That is the app's primary process in
        // practice.
        for (i, r) in records.iter().enumerate() {
            let i = i as u32;
            self.apps.entry(r.group).or_insert(i);
            if let Some(unit) = extras.get(i as usize).and_then(|x| x.unit.as_ref())
                && !self.units.contains_key(unit)
            {
                self.units.insert(unit.clone(), i);
            }
        }
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.builds += 1;
        }
    }

    /// How many times `refresh` rebuilt the index.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn builds(&self) -> u64 {
        self.builds
    }

    /// Record index of the earliest member of an app or unit row.
    fn first(&self, g: &GroupRow) -> Option<usize> {
        let i = match (&g.kind, &g.node) {
            (GroupRowKind::App, _) => self.apps.get(&g.key),
            (GroupRowKind::Unit, NodeId::Unit(unit)) => self.units.get(unit),
            _ => None,
        };
        i.map(|&i| i as usize)
    }
}

/// What `fill_cell` reads besides the row.
pub struct CellCtx<'a> {
    pub view: &'a ProcessTableView,
    /// `ProcessSnapshot.records`.
    pub records: &'a [ProcessRecord],
    /// `ProcessSnapshot.extras`, index aligned with `records`. Empty gives WarmingUp cells.
    pub extras: &'a [ProcessExtras],
    /// `ProcessSnapshot.extras_p2`, index aligned with `records`. Empty gives Unsupported cells
    /// in the phase-2 columns.
    pub extras_p2: &'a [ProcessExtrasP2],
    /// `units.network`, `units.network_auto_scale`.
    pub settings: &'a Settings,
    pub words: &'a CellWords,
    /// Refreshed by the caller for this `view` and `records`.
    pub members: &'a GroupMembers,
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
    /// The text before the " <email>" of a packager.
    Packager(&'a Arc<str>),
    /// ELF class (32 or 64) and `e_machine`.
    Architecture(u8, u16),
    /// Estimated watts, shown as a class.
    Power(f32),
}

/// Fills `out` for one cell. Sets `bits` and `validity` always; writes `text` only when
/// `out.want_text` is set and the cell has a value to show, so a cache hit formats nothing
/// (ADR-009). Arc texts go to `out.arc` and never to `text`.
///
/// A Σ column of a row with a [`subtree_total`] shows the engine's subtree sum (PROC-019).
pub fn fill_cell(row: RowRef, col: &ProcColumn, ctx: &CellCtx, out: &mut CellOut) {
    let value = match row {
        RowRef::Group(_, RowKind::GroupHeader) => Value::Blank,
        RowRef::Group(g, _) => pseudo_na(g, col.id, group_value(g, col.id, ctx)),
        RowRef::Record(r, t, _) => match subtree_total(row, ctx) {
            Some(g) if subtree_column(col.id) => group_value(g, col.id, ctx),
            _ => {
                let x = t.record.and_then(|i| ctx.extras.get(i as usize));
                record_value(r, t, x, col.id, ctx)
            }
        },
    };
    write_value(value, ctx, out);
}

// ---- Subtree totals (PROC-019, R-005, UI-PRC-002) ----

/// Drawn in `text_secondary` before a subtree sum: U+03A3 and a space.
pub const SUBTREE_PREFIX: &str = "\u{03a3} ";

/// The columns that show a subtree sum: CPU (all percent kinds), Disk and its rates, Network,
/// GPU, GPU dedicated memory, Threads, Handles and Memory. These are the columns the engine's
/// subtree row holds a sum for; the others keep the process's own value and get no prefix.
pub fn subtree_column(id: ColumnId) -> bool {
    use ColumnId as C;
    matches!(
        id,
        C::Cpu
            | C::CpuKernel
            | C::CpuUser
            | C::CpuRelative
            | C::Memory
            | C::Disk
            | C::DiskReadRate
            | C::DiskWriteRate
            | C::DiskTotalRate
            | C::Network
            | C::NetSendRate
            | C::NetRecvRate
            | C::Gpu
            | C::GpuMemDedicated
            | C::Threads
            | C::Handles
    )
}

/// The engine's subtree sum for `row`: `Some` only for a process row with children while
/// `processes.subtree_totals` is on and the view holds its sum. App, unit, pseudo and group
/// rows always aggregate and have none. The UI never sums.
pub fn subtree_total<'a>(row: RowRef<'a>, ctx: &CellCtx<'a>) -> Option<&'a GroupRow> {
    let RowRef::Record(_, t, _) = row else {
        return None;
    };
    if !ctx.settings.processes.subtree_totals || !t.has_children {
        return None;
    }
    t.subtree.and_then(|i| ctx.view.subtree.get(i as usize))
}

/// The coloured end of a filled cell's text (UI-PRC-004): the PID's " +N" of a collapsed parent
/// in `text_secondary` (`text_selected_secondary` when selected), an Image path's " (deleted)"
/// in `status.danger`. Reads `out.bits` and `out.arc`, so it needs no text. Call after
/// [`fill_cell`].
pub fn cell_suffix(col: ColumnId, out: &CellOut, theme: &TlTheme) -> Option<CellSuffix> {
    if !shows_value(out.validity) {
        return None;
    }
    match col {
        ColumnId::Pid => {
            let plus = (out.bits >> 32) as u32;
            (plus > 0).then(|| CellSuffix {
                // " +" and the digits of N.
                len: 2 + plus.checked_ilog10().unwrap_or(0) as usize + 1,
                color: theme.text_secondary,
                selected_color: theme.text_selected_secondary,
            })
        }
        ColumnId::ImagePath => out
            .arc
            .as_deref()
            .filter(|path| path.ends_with(DELETED_SUFFIX))
            .map(|_| CellSuffix {
                len: DELETED_SUFFIX.len(),
                color: theme.status.danger,
                selected_color: theme.status.danger,
            }),
        _ => None,
    }
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
        Value::Packager(a) => (arc_bits(a), Validity::Valid),
        Value::Architecture(class, machine) => (
            u64::from(*class) << 16 | u64::from(*machine),
            Validity::Valid,
        ),
        Value::Power(w) => (u64::from(w.to_bits()), Validity::Valid),
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
        Value::Packager(a) => s.push_str(a.split(" <").next().unwrap_or(a).trim_end()),
        Value::Architecture(class, machine) => write_architecture(s, class, machine),
        Value::Power(w) => s.push_str(&ctx.words.power[power_class(w)]),
    }
}

/// Index into `CellWords::power` (PROC-034): below 0.5, 2, 8 and 25 W, then from 25 W.
fn power_class(watts: f32) -> usize {
    [0.5, 2.0, 8.0, 25.0]
        .iter()
        .position(|&limit| watts < limit)
        .unwrap_or(4)
}

/// PROC-027 Architecture: "x86-64 (64-bit)", "i386 (32-bit)", "AArch64 (64-bit)". Machines
/// without a name show "machine N".
fn write_architecture(out: &mut String, class: u8, machine: u16) {
    match machine {
        3 => out.push_str("i386"),
        40 => out.push_str("ARM"),
        62 => out.push_str("x86-64"),
        183 => out.push_str("AArch64"),
        243 => out.push_str("RISC-V"),
        n => {
            let _ = write!(out, "machine {n}");
        }
    }
    let _ = write!(out, " ({class}-bit)");
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
        SchedPolicy::Other | SchedPolicy::Batch | SchedPolicy::Deadline => match nice {
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
    // A column read from `ProcessExtrasP2`; Unsupported when the row has no entry or the value
    // was not read.
    let p2_row = t.record.and_then(|i| ctx.extras_p2.get(i as usize));
    let p2 = |f: &dyn Fn(&'a ProcessExtrasP2) -> Option<Value<'a>>| {
        p2_row
            .and_then(f)
            .unwrap_or(Value::Glyph(Validity::Unsupported))
    };
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
        // PROC-027, PROC-028, PROC-029, PROC-034, PROC-036 phase-2 columns.
        C::FileMtime => p2(&|x| x.exe_mtime_s.map(Value::Time)),
        C::FileSize => p2(&|x| x.exe_size_bytes.map(|n| Value::Bytes(n, Validity::Valid))),
        C::Architecture => p2(&|x| Some(Value::Architecture(x.elf_class?, x.elf_machine?))),
        C::Session => p2(&|x| {
            x.session_id.map(|id| {
                if id == u32::MAX {
                    Value::Blank
                } else {
                    Value::Id(id.into())
                }
            })
        }),
        C::SecurityLabel => p2(&|x| x.security_label.as_ref().map(Value::Arc)),
        C::Sandbox => p2(&|x| x.sandbox.as_ref().map(Value::Arc)),
        C::SchedPolicy => p2(&|x| x.sched_policy_text.as_ref().map(Value::Arc)),
        C::OomScore => p2(&|x| x.oom_score.map(|n| Value::Id(n.into()))),
        C::OomScoreAdj => p2(&|x| x.oom_score_adj.map(|n| Value::Id(n.into()))),
        C::Tty => p2(&|x| x.tty.as_ref().map(Value::Arc)),
        C::Cwd => p2(&|x| x.cwd.as_ref().map(Value::Arc)),
        C::CtxSwitches => p2(
            &|x| match (x.ctx_switches_voluntary, x.ctx_switches_involuntary) {
                (None, None) => None,
                (a, b) => Some(Value::Count(
                    a.unwrap_or(0).saturating_add(b.unwrap_or(0)),
                    Validity::Valid,
                )),
            },
        ),
        C::CtxSwitchesDelta => p2(&|x| {
            x.ctx_switches_delta
                .map(|n| Value::Count(n, Validity::Valid))
        }),
        C::Power => p2(&|x| x.power_w.map(Value::Power)),
        C::PowerTrend => p2(&|x| x.power_trend_w.map(Value::Power)),
        C::Version => p2(&|x| x.package.as_ref().map(|p| Value::Arc(&p.version))),
        C::Timestamp => p2(&|x| x.package.as_ref()?.build_date_s.map(Value::Time)),
        C::Company => p2(&|x| x.package.as_ref().map(|p| Value::Arc(&p.name))),
        C::Signer => p2(&|x| x.package.as_ref()?.packager.as_ref().map(Value::Packager)),
        C::Signature => p2(&|x| {
            x.package_status.map(|s| {
                Value::Arc(
                    &ctx.words.package_status[match s {
                        PackageStatus::Packaged => 0,
                        PackageStatus::PackagedModified => 1,
                        PackageStatus::NotPackaged => 2,
                        PackageStatus::DeletedUpgraded => 3,
                        PackageStatus::Flatpak => 4,
                        PackageStatus::Snap => 5,
                        PackageStatus::AppImage => 6,
                        PackageStatus::Wine => 7,
                    }],
                )
            })
        }),
        // No source yet: Cycles, GPU columns and the later phases.
        _ => Value::Glyph(Validity::Unsupported),
    }
}

/// The earliest started member of an app or unit row, from `ctx.members`.
fn first_member<'a>(g: &GroupRow, ctx: &CellCtx<'a>) -> Option<(usize, &'a ProcessRecord)> {
    let i = ctx.members.first(g)?;
    ctx.records.get(i).map(|r| (i, r))
}

/// User of the pseudo rows (PROC-012): the kernel's work, shown as root.
fn root_user() -> &'static Arc<str> {
    static ROOT: OnceLock<Arc<str>> = OnceLock::new();
    ROOT.get_or_init(|| Arc::from("root"))
}

/// A pseudo row shows `n/a` in every column it has no data for (PROC-012). PID stays blank.
fn pseudo_na<'a>(g: &GroupRow, col: ColumnId, value: Value<'a>) -> Value<'a> {
    if g.kind == GroupRowKind::Pseudo && col != ColumnId::Pid && matches!(value, Value::Blank) {
        Value::Glyph(Validity::Unsupported)
    } else {
        value
    }
}

fn group_value<'a>(g: &'a GroupRow, col: ColumnId, ctx: &'a CellCtx) -> Value<'a> {
    use ColumnId as C;
    use ProcessField as F;

    let v = |f: F| g.validity.get(f);
    let pair = |a: F, b: F| worse(v(a), v(b));
    let member = || first_member(g, ctx);
    let w = ctx.words;
    // A sum of `ProcessExtras` columns; WarmingUp until the extras arrive. Pseudo rows have none.
    let sum = |value: Value<'a>| {
        if g.kind == GroupRowKind::Pseudo {
            Value::Blank
        } else if ctx.extras.is_empty() {
            Value::Glyph(Validity::WarmingUp)
        } else {
            value
        }
    };
    let cpu_time = |ticks: u64| {
        if ctx.clk_tck == 0 {
            return Value::Glyph(Validity::Unsupported);
        }
        sum(Value::Duration(ticks / u64::from(ctx.clk_tck)))
    };

    match col {
        // App rows: the desktop file. Unit rows have no image path.
        C::ImagePath if matches!(g.kind, GroupRowKind::App | GroupRowKind::Unit) => g
            .desktop_path
            .as_ref()
            .map_or(Value::Glyph(Validity::Unsupported), Value::Arc),
        C::Name | C::NameDisplay => Value::Arc(w.group_name(g)),
        // Only a frozen app or unit row has a state word (PROC-036, PROC-064).
        C::State if g.frozen => Value::Arc(&w.state[7]),
        C::User if g.kind == GroupRowKind::Pseudo => Value::Arc(root_user()),
        C::Type => Value::Arc(match g.kind {
            GroupRowKind::App => &w.type_app,
            GroupRowKind::Unit => &w.type_unit,
            GroupRowKind::KernelThreads => &w.type_kernel_thread,
            GroupRowKind::Pseudo => &w.type_pseudo,
        }),
        // The unit's own User and Description first, then the earliest member's.
        C::User => g.user.as_ref().map_or_else(
            || member().map_or(Value::Blank, |(_, r)| Value::Arc(&r.meta.user)),
            Value::Arc,
        ),
        C::Description => g
            .description
            .as_ref()
            .or_else(|| member().and_then(|(_, r)| r.meta.description.as_ref()))
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
        C::CpuKernel => sum(Value::Percent(g.cpu_kernel_pct, v(F::Cpu))),
        C::CpuUser => sum(Value::Percent(g.cpu_user_pct, v(F::Cpu))),
        C::CpuRelative => sum(Value::Percent(g.cpu_relative_pct, v(F::Cpu))),
        C::CpuTime => cpu_time(g.utime_ticks.saturating_add(g.stime_ticks)),
        C::CpuTimeKernel => cpu_time(g.stime_ticks),
        C::CpuTimeUser => cpu_time(g.utime_ticks),
        C::Commit => sum(Value::Bytes(g.commit_bytes, Validity::Valid)),
        C::PageFaults => sum(Value::Count(g.page_faults, Validity::Valid)),
        C::PageFaultsDelta => sum(Value::Count(g.page_faults_delta, Validity::Valid)),
        C::MajorFaults => sum(Value::Count(g.major_faults, Validity::Valid)),
        C::Handles => sum(g.handles.map_or(Value::Glyph(Validity::Restricted), |h| {
            Value::Count(h.into(), Validity::Valid)
        })),
        C::Unit => match &g.node {
            NodeId::Unit(u) => Value::Arc(&u.name),
            _ => Value::Blank,
        },
        C::AppId => match &g.class {
            GroupKind::App { app_id } if g.kind == GroupRowKind::App => Value::Arc(app_id),
            _ => Value::Blank,
        },
        // PID, Parent PID and Image name are blank on group rows (PROC-008,
        // PROC-018). The other columns have no group value.
        _ => Value::Blank,
    }
}

// ---- Copy commands (PROC-070, ADR-015) ----

/// The text one cell copies: its displayed text, or the ADR-015 copy word of a value that was
/// not read ("Restricted", "n/a", "…", "-", "!"), never a 0.
fn copy_value(row: RowRef, col: &ProcColumn, ctx: &CellCtx, out: &mut CellOut) -> String {
    out.reset();
    out.want_text = true;
    fill_cell(row, col, ctx, out);
    let text = match out.arc.take() {
        Some(arc) => arc.to_string(),
        None => std::mem::take(&mut out.text),
    };
    copy_text(out.validity, &text.into()).to_string()
}

/// Copy on a cell: the displayed value of `col` for each row, one per line, in view order.
pub fn copy_cell(rows: &[RowRef], col: &ProcColumn, ctx: &CellCtx) -> String {
    let mut out = CellOut::default();
    rows.iter()
        .map(|&row| copy_value(row, col, ctx, &mut out))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Copy details (Ctrl+C): a fixed block per row, blocks separated by one blank line.
///
/// ```text
/// Name: firefox
/// PID: 4321
/// Parent PID: 2010
/// CPU: 3.27%
/// Memory: 812 MiB
/// Private bytes: 1490 MiB
/// Working set: 1104 MiB
/// Threads: 97
/// Handles: 412
/// Priority: Normal (0)
/// ```
///
/// CPU is the share of the whole machine with two decimals (R-094). Memory is RssAnon, Private
/// bytes VmData + VmStk (Commit size), Working set VmRSS, each in whole MiB. Handles is the fd
/// count. App and unit rows leave PID, Parent PID and Priority empty.
pub fn copy_details(rows: &[RowRef], ctx: &CellCtx) -> String {
    let w = ctx.words;
    let mut text = String::new();
    for (n, &row) in rows.iter().enumerate() {
        if n > 0 {
            text.push_str("\n\n");
        }
        let d = Details::of(row, ctx);
        let lines: [(&str, String); 10] = [
            (w.label(ColumnId::Name), d.name.to_string()),
            (
                w.label(ColumnId::Pid),
                d.pid.map_or_else(String::new, |p| p.to_string()),
            ),
            (
                w.label(ColumnId::Ppid),
                d.ppid.map_or_else(String::new, |p| p.to_string()),
            ),
            (w.label(ColumnId::Cpu), d.cpu.text(|p| format!("{p:.2}%"))),
            (w.label(ColumnId::Memory), d.memory.text(whole_mib)),
            (w.copy_private_bytes.as_ref(), d.private.text(whole_mib)),
            (w.copy_working_set.as_ref(), d.working_set.text(whole_mib)),
            (
                w.label(ColumnId::Threads),
                d.threads.text(|n| n.to_string()),
            ),
            (
                w.label(ColumnId::Handles),
                d.handles.text(|n| n.to_string()),
            ),
            (w.label(ColumnId::Priority), d.priority),
        ];
        for (i, (label, value)) in lines.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            let _ = write!(text, "{label}: {value}");
        }
    }
    text
}

/// Copy all metrics: per row one `Label: value` line for every registry column with a Valid
/// value and non-empty text, in picker order, then `Command line: <full command line>` last.
/// Rows are separated by one blank line. The environment is never read.
pub fn copy_all_metrics(rows: &[RowRef], ctx: &CellCtx) -> String {
    let w = ctx.words;
    let mut out = CellOut::default();
    let mut text = String::new();
    for (n, &row) in rows.iter().enumerate() {
        if n > 0 {
            text.push_str("\n\n");
        }
        for (col, label) in PROCESS_COLUMNS.iter().zip(w.labels.iter()) {
            if col.id == ColumnId::Cmdline {
                continue;
            }
            out.reset();
            out.want_text = true;
            fill_cell(row, col, ctx, &mut out);
            if out.validity != Validity::Valid {
                continue;
            }
            let value = match out.arc.take() {
                Some(arc) => arc.to_string(),
                // PID without the "+N" of a collapsed parent.
                None if col.id == ColumnId::Pid => split_pid_text(&out.text).0.to_owned(),
                None => std::mem::take(&mut out.text),
            };
            if !value.is_empty() {
                let _ = writeln!(text, "{label}: {value}");
            }
        }
        let cmdline = match row {
            RowRef::Record(r, _, _) => r.meta.cmdline.as_deref().map_or_else(
                || copy_text(Validity::Restricted, &SharedString::default()).to_string(),
                str::to_owned,
            ),
            RowRef::Group(g, _) => first_member(g, ctx)
                .and_then(|(_, r)| r.meta.cmdline.as_deref())
                .unwrap_or_default()
                .to_owned(),
        };
        let _ = write!(text, "{}: {cmdline}", w.label(ColumnId::Cmdline));
    }
    text
}

/// Bytes in whole MiB, rounded to the nearest integer: "812 MiB".
fn whole_mib(bytes: u64) -> String {
    format!("{} MiB", (bytes as f64 / MIB).round() as u64)
}

/// A Copy details value and its validity.
struct Field<T>(T, Validity);

impl<T: Copy> Field<T> {
    /// The value through `fmt` when it was read, else the ADR-015 copy word.
    fn text(&self, fmt: impl Fn(T) -> String) -> String {
        if shows_value(self.1) {
            fmt(self.0)
        } else {
            copy_text(self.1, &SharedString::default()).to_string()
        }
    }
}

/// The Copy details values of one row.
struct Details<'a> {
    name: &'a str,
    pid: Option<u32>,
    ppid: Option<u32>,
    cpu: Field<f32>,
    memory: Field<u64>,
    private: Field<u64>,
    working_set: Field<u64>,
    threads: Field<u64>,
    handles: Field<u64>,
    priority: String,
}

impl<'a> Details<'a> {
    fn of(row: RowRef<'a>, ctx: &'a CellCtx) -> Self {
        use ProcessField as F;
        let warming = Field(0, Validity::WarmingUp);
        let handles = |h: Option<u32>| {
            h.map_or(Field(0, Validity::Restricted), |h| {
                Field(u64::from(h), Validity::Valid)
            })
        };
        match row {
            RowRef::Record(r, t, _) => {
                let v = |f: F| r.validity.get(f);
                let x = t.record.and_then(|i| ctx.extras.get(i as usize));
                let priority = x.map_or_else(String::new, |x| {
                    let mut s = String::new();
                    write_priority(&mut s, ctx.words, x.policy, x.rt_prio, r.nice);
                    let _ = write!(s, " ({})", r.nice);
                    s
                });
                Self {
                    name: &r.meta.name,
                    pid: Some(r.key.pid),
                    ppid: Some(r.ppid),
                    cpu: Field(r.cpu_pct, v(F::Cpu)),
                    memory: Field(r.rss_anon_bytes, v(F::RssAnon)),
                    private: x.map_or(warming, |x| Field(x.commit_bytes, Validity::Valid)),
                    working_set: Field(r.rss_bytes, v(F::Rss)),
                    threads: Field(r.threads.into(), v(F::Threads)),
                    handles: x.map_or(Field(0, Validity::WarmingUp), |x| handles(x.handles)),
                    priority,
                }
            }
            RowRef::Group(g, _) => {
                let v = |f: F| g.validity.get(f);
                let sum = |field: Field<u64>| {
                    if ctx.extras.is_empty() {
                        Field(0, Validity::WarmingUp)
                    } else {
                        field
                    }
                };
                Self {
                    name: ctx.words.group_name(g),
                    pid: None,
                    ppid: None,
                    cpu: Field(g.cpu_pct, v(F::Cpu)),
                    memory: Field(g.memory_bytes, v(F::RssAnon)),
                    private: sum(Field(g.commit_bytes, Validity::Valid)),
                    working_set: Field(g.rss_bytes, v(F::Rss)),
                    threads: Field(g.threads.into(), v(F::Threads)),
                    handles: sum(handles(g.handles)),
                    priority: String::new(),
                }
            }
        }
    }
}

// ---- Cell tooltips (UI-PRC-004) ----

/// The Command line tooltip breaks its text into lines of this many characters.
pub const CMDLINE_TIP_COLUMNS: usize = 80;

const KEY_TIP_APP_MEMORY: &str = "processes-cell-tooltip-app-memory";
const KEY_TIP_NICE: &str = "processes-cell-tooltip-nice";
const KEY_TIP_POWER_ESTIMATE: &str = "processes-cell-tooltip-power-estimate";
const KEY_TIP_POWER_NEEDS_SETUP: &str = "processes-cell-tooltip-power-needs-setup";
const KEY_TIP_POWER_NO_SENSOR: &str = "processes-cell-tooltip-power-no-sensor";
const KEY_TIP_ZRAM: &str = "processes-cell-tooltip-zram";
const KEY_TIP_OOM_SCORE_ADJ: &str = "processes-cell-tooltip-oom-score-adj";
const KEY_TIP_DISK_OPS: &str = "processes-cell-tooltip-disk-ops";
const KEY_TIP_IRQ: &str = "processes-cell-tooltip-irq-in-task-time";
const KEY_TIP_EUID: &str = "processes-cell-tooltip-euid";
const KEY_TIP_NET_SHARED: &str = "processes-cell-tooltip-net-shared";
const KEY_TIP_CANCELLED: &str = "processes-cell-tooltip-cancelled-writes";
const KEY_TIP_GPU_VRAM: &str = "processes-cell-tooltip-gpu-vram";

/// The cell tooltip texts, translated once per `strings_version`. `Default` is en-US.
#[derive(Clone, Debug)]
pub struct CellTipWords {
    pub app_memory: SharedString,
    /// "nice {$nice}".
    pub nice: Template,
    /// "≈ {$watts} W (estimate)".
    pub power_estimate: Template,
    pub power_needs_setup: SharedString,
    pub power_no_sensor: SharedString,
    /// "original {$original}, compressed {$compressed}, ratio {$ratio}".
    pub zram: Template,
    /// "oom_score_adj {$adj}".
    pub oom_score_adj: Template,
    pub disk_ops: SharedString,
    pub irq_in_task_time: SharedString,
    /// "euid {$euid}".
    pub euid: Template,
    pub net_shared: SharedString,
    /// "cancelled: {$size}/s".
    pub cancelled: Template,
    /// "VRAM on GPU {$n}: {$size}".
    pub gpu_vram: Template,
}

impl Default for CellTipWords {
    fn default() -> Self {
        Self {
            app_memory: tr_en(KEY_TIP_APP_MEMORY, &[]),
            nice: Template::en(KEY_TIP_NICE, &["nice"]),
            power_estimate: Template::en(KEY_TIP_POWER_ESTIMATE, &["watts"]),
            power_needs_setup: tr_en(KEY_TIP_POWER_NEEDS_SETUP, &[]),
            power_no_sensor: tr_en(KEY_TIP_POWER_NO_SENSOR, &[]),
            zram: Template::en(KEY_TIP_ZRAM, &["original", "compressed", "ratio"]),
            oom_score_adj: Template::en(KEY_TIP_OOM_SCORE_ADJ, &["adj"]),
            disk_ops: tr_en(KEY_TIP_DISK_OPS, &[]),
            irq_in_task_time: tr_en(KEY_TIP_IRQ, &[]),
            euid: Template::en(KEY_TIP_EUID, &["euid"]),
            net_shared: tr_en(KEY_TIP_NET_SHARED, &[]),
            cancelled: Template::en(KEY_TIP_CANCELLED, &["size"]),
            gpu_vram: Template::en(KEY_TIP_GPU_VRAM, &["n", "size"]),
        }
    }
}

impl CellTipWords {
    /// Every text through `tr`.
    pub fn new(cx: &App) -> Self {
        Self {
            app_memory: tr(cx, KEY_TIP_APP_MEMORY),
            nice: Template::new(cx, KEY_TIP_NICE, &["nice"]),
            power_estimate: Template::new(cx, KEY_TIP_POWER_ESTIMATE, &["watts"]),
            power_needs_setup: tr(cx, KEY_TIP_POWER_NEEDS_SETUP),
            power_no_sensor: tr(cx, KEY_TIP_POWER_NO_SENSOR),
            zram: Template::new(cx, KEY_TIP_ZRAM, &["original", "compressed", "ratio"]),
            oom_score_adj: Template::new(cx, KEY_TIP_OOM_SCORE_ADJ, &["adj"]),
            disk_ops: tr(cx, KEY_TIP_DISK_OPS),
            irq_in_task_time: tr(cx, KEY_TIP_IRQ),
            euid: Template::new(cx, KEY_TIP_EUID, &["euid"]),
            net_shared: tr(cx, KEY_TIP_NET_SHARED),
            cancelled: Template::new(cx, KEY_TIP_CANCELLED, &["size"]),
            gpu_vram: Template::new(cx, KEY_TIP_GPU_VRAM, &["n", "size"]),
        }
    }
}

/// Whether the per-process power estimate has a source (PROC-034).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PowerSensor {
    Readable,
    /// RAPL exists but `energy_uj` is not readable.
    NeedsSetup,
    /// No power source at all.
    Absent,
}

impl PowerSensor {
    /// From the validity of the RAPL package power (`CpuTotals::package_power_w`).
    pub fn from_validity(v: Validity) -> Self {
        match v {
            Validity::Restricted => Self::NeedsSetup,
            Validity::Unsupported | Validity::Missing => Self::Absent,
            _ => Self::Readable,
        }
    }
}

/// What [`cell_tooltip`] reads besides [`CellCtx`].
pub struct TipCtx<'a> {
    pub words: &'a CellTipWords,
    /// `ProcessSnapshot.extras_p2`, index aligned with `records`. Empty before the P2 sweep.
    pub extras_p2: &'a [ProcessExtrasP2],
    /// `MemoryTotals::zram`, for the Memory compression row.
    pub zram: Metric<ZramTotals>,
    pub power: PowerSensor,
}

/// Writes the tooltip of one cell to `out` and returns true, or returns false for a cell
/// without one (UI-PRC-004). It formats, so call it only for the hovered cell. The caller keeps
/// the validity tooltip for cells that are not Valid or Unsupported.
pub fn cell_tooltip(
    row: RowRef,
    col: &ProcColumn,
    ctx: &CellCtx,
    tip: &TipCtx,
    out: &mut String,
) -> bool {
    use ColumnId as C;

    let w = tip.words;
    match row {
        RowRef::Group(_, RowKind::GroupHeader) => false,
        RowRef::Group(g, _) => match col.id {
            C::Cmdline => first_member(g, ctx)
                .and_then(|(_, r)| r.meta.cmdline.as_ref())
                .is_some_and(|c| write_cmdline_tip(out, c)),
            C::Memory => match (&g.kind, &g.node) {
                (GroupRowKind::App | GroupRowKind::Unit, _) => {
                    out.push_str(&w.app_memory);
                    true
                }
                (_, NodeId::Pseudo(PseudoKind::MemoryCompression)) => {
                    write_zram_tip(out, w, tip.zram)
                }
                _ => false,
            },
            C::Cpu => {
                let na = g.node == NodeId::Pseudo(PseudoKind::Interrupts)
                    && g.validity.get(ProcessField::Cpu) == Validity::Unsupported;
                if na {
                    out.push_str(&w.irq_in_task_time);
                }
                na
            }
            _ => false,
        },
        RowRef::Record(r, t, _) => {
            let p2 = t.record.and_then(|i| tip.extras_p2.get(i as usize));
            match col.id {
                C::Cmdline => r
                    .meta
                    .cmdline
                    .as_ref()
                    .is_some_and(|c| write_cmdline_tip(out, c)),
                // The Priority cell shows a value only once the extras arrived.
                C::Priority => {
                    let shown = t.record.and_then(|i| ctx.extras.get(i as usize)).is_some();
                    if shown {
                        let _ = w.nice.write(out, &[&r.nice]);
                    }
                    shown
                }
                C::Power | C::PowerTrend => {
                    let watts = p2.and_then(|x| {
                        if col.id == C::Power {
                            x.power_w
                        } else {
                            x.power_trend_w
                        }
                    });
                    match (watts, tip.power) {
                        (Some(watts), _) => {
                            let _ = w.power_estimate.write(out, &[&format_args!("{watts:.1}")]);
                            true
                        }
                        (None, PowerSensor::NeedsSetup) => {
                            out.push_str(&w.power_needs_setup);
                            true
                        }
                        (None, PowerSensor::Absent) => {
                            out.push_str(&w.power_no_sensor);
                            true
                        }
                        (None, PowerSensor::Readable) => false,
                    }
                }
                C::OomScore => match p2.and_then(|x| x.oom_score_adj) {
                    Some(adj) => {
                        let _ = w.oom_score_adj.write(out, &[&adj]);
                        true
                    }
                    None => false,
                },
                C::DiskReads | C::DiskWrites | C::DiskReadsDelta | C::DiskWritesDelta => {
                    out.push_str(&w.disk_ops);
                    true
                }
                // PROC-028: only when the effective UID differs from the real one.
                C::User => match r.euid {
                    Some(euid) if euid != r.uid => {
                        let _ = w.euid.write(out, &[&euid]);
                        true
                    }
                    _ => false,
                },
                C::Network if r.net_shared => {
                    out.push_str(&w.net_shared);
                    true
                }
                // PROC-031: IEC two decimals per second from 1 KiB/s; less is not worth a tooltip.
                C::DiskWriteRate => {
                    let bytes = f64::from(r.cancelled_write_bps).round() as u64;
                    if bytes < 1024 {
                        return false;
                    }
                    w.cancelled
                        .write_each(out, |_, out| units::bytes(out, bytes));
                    true
                }
                // PROC-033: `gpu_vram_bytes` is in GPU N column order.
                id => match gpu_slot(id).and_then(|n| {
                    let bytes = *p2?.gpu_vram_bytes.get(n)?;
                    (bytes > 0).then_some((n, bytes))
                }) {
                    Some((n, bytes)) => {
                        w.gpu_vram.write_each(out, |i, out| {
                            if i == 0 {
                                let _ = write!(out, "{n}");
                            } else {
                                units::bytes(out, bytes);
                            }
                        });
                        true
                    }
                    None => false,
                },
            }
        }
    }
}

/// The adapter slot of a `gpu_0` to `gpu_15` column.
fn gpu_slot(id: ColumnId) -> Option<usize> {
    let n = (id as usize).checked_sub(ColumnId::Gpu0 as usize)?;
    (id as usize <= ColumnId::Gpu15 as usize).then_some(n)
}

/// The first [`CMDLINE_CELL_CHARS`] characters of `cmdline`, with a line break after every
/// [`CMDLINE_TIP_COLUMNS`] characters. False for an empty command line.
fn write_cmdline_tip(out: &mut String, cmdline: &str) -> bool {
    for (i, c) in cmdline.chars().take(CMDLINE_CELL_CHARS).enumerate() {
        if i > 0 && i % CMDLINE_TIP_COLUMNS == 0 {
            out.push('\n');
        }
        out.push(c);
    }
    !cmdline.is_empty()
}

/// "original 1.20 GiB, compressed 400.00 MiB, ratio 3.1" (PROC-012). False when zram holds
/// nothing or was not read.
fn write_zram_tip(out: &mut String, w: &CellTipWords, zram: Metric<ZramTotals>) -> bool {
    let z = zram.value;
    if !shows_value(zram.validity) || z.compr_data == 0 {
        return false;
    }
    w.zram.write_each(out, |i, out| match i {
        0 => units::bytes(out, z.orig_data),
        1 => units::bytes(out, z.compr_data),
        _ => {
            let _ = write!(out, "{:.1}", z.orig_data as f64 / z.compr_data as f64);
        }
    });
    true
}

// ---- Cell painting (E22-004, UI-PRC-004) ----

/// Ending of an Image path whose file was deleted, drawn in `status.danger` (UI-PRC-004).
pub const DELETED_SUFFIX: &str = " (deleted)";

const MIB: f64 = 1_048_576.0;

/// How a row paints its metric cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PaintState {
    pub selected: bool,
    pub exited: bool,
    /// `appearance.heat_coloring`.
    pub heat_on: bool,
    /// `appearance.heat_intensity_pct`.
    pub heat_intensity_pct: u8,
    /// `tables.dim_zero_values` (R-075).
    pub dim_zero: bool,
    /// The row has a [`subtree_total`]: its [`subtree_column`] cells draw [`SUBTREE_PREFIX`].
    pub subtree: bool,
}

/// The kinds whose zero is dimmed: Percent, MemorySize, DiskRate, NetRate, Count and
/// BytesCumulative. Text, ids, durations, times and power classes never are.
pub fn dim_zero_kind(kind: ProcColumnKind) -> bool {
    matches!(
        kind,
        ProcColumnKind::Percent
            | ProcColumnKind::MemorySize
            | ProcColumnKind::DiskRate
            | ProcColumnKind::NetRate
            | ProcColumnKind::Count
            | ProcColumnKind::BytesCumulative
    )
}

/// True when the cell is a measured zero to draw dimmed: the setting is on, the value is Valid
/// and its bits are the kind's zero (0.0 for floats, 0 for integers, both all-zero bits).
pub fn dims_zero(out: &CellOut, col: &ProcColumn, st: PaintState) -> bool {
    st.dim_zero && out.validity == Validity::Valid && out.bits == 0 && dim_zero_kind(col.kind)
}

/// True for the kinds that can carry heat: Percent, MemorySize, DiskRate, NetRate, PowerClass.
pub fn heat_kind(kind: ProcColumnKind) -> bool {
    matches!(
        kind,
        ProcColumnKind::Percent
            | ProcColumnKind::MemorySize
            | ProcColumnKind::DiskRate
            | ProcColumnKind::NetRate
            | ProcColumnKind::PowerClass
    )
}

/// Heat alpha of a cell (PROC-040, R-059): `intensity_pct / 100 × clamp(value / reference, 0, 1)`,
/// rounded to a 2% step (UI-PRC-004) so the tint does not change on differences the cell does not
/// show. `value` is in the column's own unit: percent, bytes, bytes per second or watts. A
/// reference of 0 (no MemTotal, no VRAM) gives 0.
pub fn heat_alpha(col: &ProcColumn, value: f64, ctx: &CellCtx, intensity_pct: u8) -> f32 {
    let reference = match col.heat {
        HeatRef::CpuMachine => 400.0 / f64::from(ctx.ncpu.max(1)),
        HeatRef::CpuOneCore => 400.0,
        HeatRef::Memory => ctx.mem_total as f64 * 0.10,
        HeatRef::DiskIo => 100.0 * MIB,
        HeatRef::Network => match ctx.settings.units.network {
            NetworkUnit::Bytes => 10.0 * MIB,
            // 80 Mbps in bytes per second.
            NetworkUnit::Bits => 80_000_000.0 / 8.0,
        },
        HeatRef::GpuPercent => 100.0,
        HeatRef::Vram => ctx.vram_total as f64 * 0.25,
        HeatRef::Power => 25.0,
        HeatRef::None => return 0.0,
    };
    if reference <= 0.0 || value.is_nan() || value <= 0.0 {
        return 0.0;
    }
    let ratio = (value / reference).min(1.0);
    let alpha = f64::from(intensity_pct.min(100)) / 100.0 * ratio;
    ((alpha / HEAT_STEP).round() * HEAT_STEP) as f32
}

/// Heat alpha steps by 2% (UI-PRC-004).
const HEAT_STEP: f64 = 0.02;

/// The heat fill of a filled cell, for `CellOut::background`: `value_heat` at [`heat_alpha`].
/// `None` when heat is off, for kinds without heat, for any validity but Valid and for zero.
/// Call after [`fill_cell`]; it reads the value from `out.bits`.
pub fn heat_background(
    out: &CellOut,
    col: &ProcColumn,
    ctx: &CellCtx,
    st: PaintState,
    theme: &TlTheme,
) -> Option<Hsla> {
    if !st.heat_on || out.validity != Validity::Valid {
        return None;
    }
    let value = match col.kind {
        ProcColumnKind::Percent
        | ProcColumnKind::DiskRate
        | ProcColumnKind::NetRate
        | ProcColumnKind::PowerClass => f64::from(f32::from_bits(out.bits as u32)),
        ProcColumnKind::MemorySize => out.bits as f64,
        _ => return None,
    };
    let alpha = heat_alpha(col, value, ctx, st.heat_intensity_pct);
    (alpha > 0.0).then_some(Hsla {
        a: alpha,
        ..theme.value_heat
    })
}

// ---- NEW and EXITED highlight (PROC-043, UI-PRC-011) ----

/// Alpha of the NEW and EXITED row tints until the fade starts.
pub const TINT_ALPHA: f32 = 0.35;

/// Length of the fade at the end of the linger time (`m_slow`, 300 ms), in ns.
pub const TINT_FADE_NS: TimeNs = M_SLOW.as_nanos() as TimeNs;

/// Which highlight a row shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TintKind {
    /// `status.success`.
    New,
    /// `status.danger`.
    Exited,
}

impl TintKind {
    /// The tint colour at `alpha`.
    pub fn color(self, alpha: f32, theme: &TlTheme) -> Hsla {
        let base = match self {
            TintKind::New => theme.status.success,
            TintKind::Exited => theme.status.danger,
        };
        Hsla { a: alpha, ..base }
    }
}

/// The linger time `L = clamp(2 × interval_ms, 1000, 3000)` ms, in ns (PROC-043).
pub fn linger_ns(interval_ms: u32) -> TimeNs {
    TimeNs::from(interval_ms.saturating_mul(2).clamp(1000, 3000)) * 1_000_000
}

/// The NEW or EXITED tint of a row and its alpha (PROC-043, UI-PRC-011). `flag_since_ns` is
/// the engine time the flag first appeared, `now_ns` the same clock now. The tint holds
/// [`TINT_ALPHA`] for the linger time and fades to 0 with `ease_out_cubic` over its last
/// 300 ms. With `animations` false (`motion.animations` off or reduced motion) it holds
/// [`TINT_ALPHA`] and switches off at L. `None` past L, without a flag, or when `enabled`
/// (`processes.highlight_new_exited`) is false.
pub fn row_tint(
    flags: ProcFlags,
    flag_since_ns: TimeNs,
    now_ns: TimeNs,
    interval_ms: u32,
    animations: bool,
    enabled: bool,
) -> Option<(TintKind, f32)> {
    let kind = tint_kind(flags, enabled)?;
    let left = linger_left(flag_since_ns, now_ns, interval_ms)?;
    if !animations || left >= TINT_FADE_NS {
        return Some((kind, TINT_ALPHA));
    }
    let t = 1.0 - left as f32 / TINT_FADE_NS as f32;
    Some((kind, TINT_ALPHA * (1.0 - ease_out_cubic(t))))
}

/// When the tint of a row next changes with no new snapshot: `Some(now_ns)` while it fades
/// (it changes every frame), the fade start before that, the end of the linger time when
/// `animations` is false, and `None` for a row with no tint. Same arguments as [`row_tint`].
pub fn tint_deadline(
    flags: ProcFlags,
    flag_since_ns: TimeNs,
    now_ns: TimeNs,
    interval_ms: u32,
    animations: bool,
    enabled: bool,
) -> Option<TimeNs> {
    tint_kind(flags, enabled)?;
    let left = linger_left(flag_since_ns, now_ns, interval_ms)?;
    Some(if !animations {
        now_ns + left
    } else if left > TINT_FADE_NS {
        now_ns + left - TINT_FADE_NS
    } else {
        now_ns
    })
}

fn tint_kind(flags: ProcFlags, enabled: bool) -> Option<TintKind> {
    if !enabled {
        None
    } else if flags.contains(ProcFlags::NEW) {
        Some(TintKind::New)
    } else if flags.contains(ProcFlags::EXITED) {
        Some(TintKind::Exited)
    } else {
        None
    }
}

/// Time left of the linger time at `now_ns`, `None` once it is over.
fn linger_left(flag_since_ns: TimeNs, now_ns: TimeNs, interval_ms: u32) -> Option<TimeNs> {
    let age = now_ns.saturating_sub(flag_since_ns);
    linger_ns(interval_ms)
        .checked_sub(age)
        .filter(|&left| left > 0)
}
