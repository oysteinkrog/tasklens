//! Services copy commands (UI-SYS-020, UI-SYS-006, SYS-009, R-012, ADR-015).
//!
//! "Copy row" writes the seven-line unit block, or the process "Copy details" block for a child
//! process row. "Copy rows as table" writes the visible columns. The block has no command line.

use std::sync::Arc;

use gpui_kit::SharedString;
use smallvec::SmallVec;
use tl_model::ids::{ColumnId, NodeId};
use tl_model::process::ProcessRecord;
use tl_model::settings::Settings;
use tl_model::snapshot::{ServiceStatus, StartupType, UnitRecord};
use tl_model::system::CpuSet;
use tl_model::table::{
    ProcessTableView, ServiceColumn, SysColumnId, SysNodeId, TableRow, UnitType,
};
use tl_model::validity::Validity;

use super::delegate::ServicesDelegate;
use crate::i18n::tr_en;
use crate::pages::processes::cells::{CellCtx, CellWords, GroupMembers, RowRef, copy_details};
use crate::pages::sys_copy::{CopyTexts, SysCopySource};
use crate::pages::sys_status::{StatusWords, SysStatus};
use crate::table::TreeTableDelegate as _;
use crate::table::cells::CellOut;
use crate::table::tree_table::{RowKind, TlTreeTable};

/// Writes the "Copy row" block of one unit (no trailing newline). Labels and words are en-US.
pub fn block_service(unit: &UnitRecord, out: &mut String) {
    let status = StatusWords::default();
    let pid = if unit.main_pid == 0 {
        String::new()
    } else {
        unit.main_pid.to_string()
    };
    let binary = if unit.kind == UnitType::Service {
        unit.path.as_ref()
    } else {
        ""
    };
    let lines: [(&str, &str); 7] = [
        ("services-sys020-name", unit.unit.name.as_ref()),
        ("services-sys020-display-name", unit.description.as_ref()),
        (
            "services-sys020-status",
            status.label(status_word(unit.status)),
        ),
        ("services-sys020-start-type", startup_word(unit.startup)),
        ("services-sys020-pid", &pid),
        ("services-sys020-user", unit.user.as_ref()),
        ("services-sys020-binary", binary),
    ];
    for (i, (key, value)) in lines.into_iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&tr_en(key, &[]));
        out.push_str(": ");
        out.push_str(value);
    }
}

/// The toast texts of the Services copy commands.
pub fn services_copy_texts() -> CopyTexts {
    CopyTexts {
        row_ok: tr_en("services-sys020-service-details-copied", &[]),
        row_failed: tr_en("services-sys020-could-not-copy-service-details", &[]),
        nothing_selected: tr_en("services-sys020-select-a-service-row-first", &[]),
    }
}

pub(crate) fn status_word(status: ServiceStatus) -> SysStatus {
    match status {
        ServiceStatus::Running => SysStatus::Running,
        ServiceStatus::Active => SysStatus::Active,
        ServiceStatus::Starting => SysStatus::Starting,
        ServiceStatus::Stopping => SysStatus::Stopping,
        ServiceStatus::Reloading => SysStatus::Reloading,
        ServiceStatus::Stopped => SysStatus::Stopped,
        ServiceStatus::Failed => SysStatus::Failed,
        ServiceStatus::Maintenance => SysStatus::Maintenance,
    }
}

/// The SYS-003 startup word.
fn startup_word(startup: StartupType) -> &'static str {
    match startup {
        StartupType::Enabled => "Enabled",
        StartupType::EnabledRuntime => "Enabled (runtime)",
        StartupType::Disabled => "Disabled",
        StartupType::Static => "Static",
        StartupType::Indirect => "Indirect",
        StartupType::Masked => "Masked",
        StartupType::Generated => "Generated",
        StartupType::Transient => "Transient",
        StartupType::Alias => "Alias",
        StartupType::Linked => "Linked",
        StartupType::NoUnitFile => "-",
    }
}

/// The process "Copy details" block for a child process row. A missing record writes nothing.
fn write_process_block(delegate: &ServicesDelegate, record: u32, out: &mut String) {
    let processes = delegate.processes();
    let Some(rec) = processes.records.get(record as usize) else {
        return;
    };
    let row = process_row(rec, record);
    let words = CellWords::from_fn(|key| Arc::from(tr_en(key, &[]).as_ref()));
    let view = ProcessTableView::default();
    let settings = Settings::default();
    let members = GroupMembers::default();
    let online = CpuSet::default();
    let ctx = CellCtx {
        view: &view,
        records: &processes.records,
        extras: &processes.extras,
        settings: &settings,
        words: &words,
        members: &members,
        ncpu: 0,
        online: &online,
        mem_total: 0,
        vram_total: 0,
        boot_time_s: 0,
        clk_tck: 0,
    };
    out.push_str(&copy_details(
        &[RowRef::Record(rec, &row, RowKind::Process)],
        &ctx,
    ));
}

fn process_row(rec: &ProcessRecord, record: u32) -> TableRow {
    TableRow {
        node: NodeId::Process(rec.key),
        depth: 0,
        expanded: false,
        has_children: false,
        record: Some(record),
        group: None,
        descendants: 0,
        subtree: None,
    }
}

impl SysCopySource for TlTreeTable<ServicesDelegate> {
    /// The table row key (`row_key`), because `SysNodeId` is not `Copy`.
    type RowId = u64;

    fn copy_rows(&self) -> SmallVec<[u64; 8]> {
        let delegate = &self.delegate;
        let mut rows: SmallVec<[(usize, u64); 8]> = self
            .selection
            .ids()
            .filter_map(|id| Some((delegate.row_index(id)?, id)))
            .filter(|&(ix, _)| {
                !matches!(
                    delegate.services().rows[ix].node,
                    SysNodeId::ServiceGroup(_)
                )
            })
            .collect();
        rows.sort_unstable_by_key(|&(ix, _)| ix);
        rows.into_iter().map(|(_, id)| id).collect()
    }

    fn write_block(&self, row: u64, out: &mut String) {
        let delegate = &self.delegate;
        let Some(ix) = delegate.row_index(row) else {
            return;
        };
        let services = delegate.services();
        let sys_row = &services.rows[ix];
        let Some(record) = sys_row.record else {
            return;
        };
        match sys_row.node {
            SysNodeId::Unit(_) => {
                if let Some(unit) = services.units.get(record as usize) {
                    block_service(unit, out);
                }
            }
            SysNodeId::UnitProcess(_, _) => write_process_block(delegate, record, out),
            _ => {}
        }
    }

    fn visible_columns(&self) -> SmallVec<[(SharedString, ColumnId); 24]> {
        let delegate = &self.delegate;
        delegate
            .columns()
            .visible_columns()
            .iter()
            .filter_map(|&column| {
                let SysColumnId::Services(service) = column else {
                    return None;
                };
                Some((
                    delegate.column_spec(column).label.clone(),
                    column_alias(service),
                ))
            })
            .collect()
    }

    fn write_cell(&self, row: u64, col: ColumnId, out: &mut String) {
        let delegate = &self.delegate;
        let Some(ix) = delegate.row_index(row) else {
            return;
        };
        let column = alias_column(col);
        if column == ServiceColumn::Name {
            out.push_str(&delegate.name_cell(ix).primary);
            return;
        }
        let mut cell = CellOut {
            want_text: true,
            ..CellOut::default()
        };
        delegate.cell(ix, SysColumnId::Services(column), &mut cell);
        if cell.validity != Validity::Valid {
            return;
        }
        if let Some(arc) = &cell.arc {
            out.push_str(arc);
        } else {
            out.push_str(&cell.text);
        }
    }

    fn copy_texts(&self) -> &CopyTexts {
        static TEXTS: std::sync::OnceLock<CopyTexts> = std::sync::OnceLock::new();
        TEXTS.get_or_init(services_copy_texts)
    }
}

// `SysCopySource` predates system-table column ids. These aliases are private transport values;
// clipboard labels and text always come from the Services columns.
pub fn column_alias(column: ServiceColumn) -> ColumnId {
    match column {
        ServiceColumn::Name => ColumnId::Name,
        ServiceColumn::DisplayName => ColumnId::NameDisplay,
        ServiceColumn::Status => ColumnId::State,
        ServiceColumn::StartupType => ColumnId::Type,
        ServiceColumn::Pid => ColumnId::Pid,
        ServiceColumn::User => ColumnId::User,
        ServiceColumn::Path => ColumnId::ImagePath,
        ServiceColumn::Manager => ColumnId::Company,
        ServiceColumn::UnitType => ColumnId::Version,
        ServiceColumn::SubState => ColumnId::Session,
        ServiceColumn::LoadState => ColumnId::Signer,
        ServiceColumn::Cpu => ColumnId::Cpu,
        ServiceColumn::Memory => ColumnId::Memory,
        ServiceColumn::Disk => ColumnId::Disk,
        ServiceColumn::Tasks => ColumnId::Threads,
        ServiceColumn::ActiveSince => ColumnId::Timestamp,
        ServiceColumn::UnitFile => ColumnId::Cmdline,
        ServiceColumn::TriggeredBy => ColumnId::Signature,
        ServiceColumn::Description => ColumnId::Description,
    }
}

pub fn alias_column(alias: ColumnId) -> ServiceColumn {
    match alias {
        ColumnId::NameDisplay => ServiceColumn::DisplayName,
        ColumnId::State => ServiceColumn::Status,
        ColumnId::Type => ServiceColumn::StartupType,
        ColumnId::Pid => ServiceColumn::Pid,
        ColumnId::User => ServiceColumn::User,
        ColumnId::ImagePath => ServiceColumn::Path,
        ColumnId::Company => ServiceColumn::Manager,
        ColumnId::Version => ServiceColumn::UnitType,
        ColumnId::Session => ServiceColumn::SubState,
        ColumnId::Signer => ServiceColumn::LoadState,
        ColumnId::Cpu => ServiceColumn::Cpu,
        ColumnId::Memory => ServiceColumn::Memory,
        ColumnId::Disk => ServiceColumn::Disk,
        ColumnId::Threads => ServiceColumn::Tasks,
        ColumnId::Timestamp => ServiceColumn::ActiveSince,
        ColumnId::Cmdline => ServiceColumn::UnitFile,
        ColumnId::Signature => ServiceColumn::TriggeredBy,
        ColumnId::Description => ServiceColumn::Description,
        _ => ServiceColumn::Name,
    }
}
