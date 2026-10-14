//! Connections copy commands (UI-SYS-006, UI-SYS-063 and UI-SYS-066).

use std::fmt::Write as _;

use gpui_kit::{App, SharedString};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::table::{ConnColumn, SysColumnId};
use tl_model::validity::Validity;

use super::ConnectionsPage;
use super::delegate::id;
use crate::pages::sys_copy::{CopyTexts, SysCopySource};
use crate::table::TreeTableDelegate as _;
use crate::table::cells::CellOut;

#[derive(Default)]
pub(super) struct PreparedCopy {
    rows: SmallVec<[u64; 8]>,
    blocks: SmallVec<[(u64, String); 8]>,
    columns: SmallVec<[(SharedString, ColumnId); 24]>,
    cells: Vec<(u64, ColumnId, String)>,
}

impl SysCopySource for ConnectionsPage {
    type RowId = u64;

    fn copy_rows(&self) -> SmallVec<[Self::RowId; 8]> {
        self.copy_prepared.rows.clone()
    }

    fn write_block(&self, row: Self::RowId, out: &mut String) {
        if let Some((_, block)) = self
            .copy_prepared
            .blocks
            .iter()
            .find(|(cookie, _)| *cookie == row)
        {
            out.push_str(block);
        }
    }

    fn visible_columns(&self) -> SmallVec<[(SharedString, ColumnId); 24]> {
        self.copy_prepared.columns.clone()
    }

    fn write_cell(&self, row: Self::RowId, col: ColumnId, out: &mut String) {
        if let Some((_, _, text)) = self
            .copy_prepared
            .cells
            .iter()
            .find(|(cookie, column, _)| *cookie == row && *column == col)
        {
            out.push_str(text);
        }
    }

    fn copy_texts(&self) -> &CopyTexts {
        &self.copy_texts
    }
}

impl ConnectionsPage {
    pub(super) fn prepare_copy(&mut self, cx: &App, extra: Option<u64>) {
        let table = self.frame.table.read(cx);
        let mut rows: SmallVec<[u64; 8]> = table
            .selection
            .ids()
            .filter(|cookie| table.delegate.row_index(*cookie).is_some())
            .collect();
        rows.sort_unstable_by_key(|cookie| table.delegate.row_index(*cookie));
        let delegate = &table.delegate;
        let columns: SmallVec<[(SharedString, ColumnId); 24]> = delegate
            .columns()
            .visible_columns()
            .iter()
            .filter_map(|&column| {
                let SysColumnId::Connections(connection) = column else {
                    return None;
                };
                Some((
                    delegate.column_spec(column).label.clone(),
                    column_alias(connection),
                ))
            })
            .collect();
        let mut prepared_rows = rows.clone();
        if let Some(cookie) = extra
            && !prepared_rows.contains(&cookie)
        {
            prepared_rows.push(cookie);
        }
        let mut blocks = SmallVec::new();
        let mut cells = Vec::with_capacity(prepared_rows.len() * columns.len());
        for cookie in prepared_rows {
            let Some(ix) = delegate.row_index(cookie) else {
                continue;
            };
            let Some(record) = delegate.record(ix) else {
                continue;
            };
            let mut block = String::with_capacity(256);
            write_block(delegate, ix, record, &mut block);
            blocks.push((cookie, block));
            for &(_, alias) in &columns {
                let mut text = String::new();
                write_cell(delegate, ix, record, alias_column(alias), &mut text);
                cells.push((cookie, alias, text));
            }
        }
        self.copy_prepared = PreparedCopy {
            rows,
            blocks,
            columns,
            cells,
        };
    }
}

fn write_block(
    delegate: &super::delegate::ConnectionsDelegate,
    ix: usize,
    record: &tl_model::snapshot::ConnRecord,
    out: &mut String,
) {
    let (local_address, local_port) = address_parts(&record.local);
    let (foreign_address, foreign_port) = address_parts(&record.foreign);
    let _ = writeln!(out, "Protocol: {}", proto(record.proto));
    let _ = writeln!(out, "Local address: {local_address}");
    let _ = writeln!(out, "Local port: {local_port}");
    let _ = writeln!(out, "Foreign address: {foreign_address}");
    let _ = writeln!(out, "Foreign port: {foreign_port}");
    let mut state = String::new();
    write_cell(delegate, ix, record, ConnColumn::State, &mut state);
    let _ = writeln!(out, "State: {state}");
    out.push_str("PID: ");
    if record.owner_validity == Validity::Restricted {
        out.push('?');
    } else if kernel_owned(record.inode, record.owners.is_empty()) {
        out.push('-');
    } else if record.owners.is_empty() {
        out.push('?');
    } else {
        let mut pids: SmallVec<[u32; 4]> = record.owners.iter().map(|key| key.pid).collect();
        pids.sort_unstable();
        for (position, pid) in pids.iter().enumerate() {
            if position > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "{pid}");
        }
    }
    out.push_str("\nProcess: ");
    if record.owner_validity == Validity::Restricted {
        out.push_str("Restricted");
    } else if kernel_owned(record.inode, record.owners.is_empty()) {
        out.push_str("No owner (kernel)");
    } else {
        out.push_str(&delegate.name_cell(ix).primary);
    }
}

fn write_cell(
    delegate: &super::delegate::ConnectionsDelegate,
    ix: usize,
    record: &tl_model::snapshot::ConnRecord,
    column: ConnColumn,
    out: &mut String,
) {
    if column == ConnColumn::Process {
        if record.owner_validity != Validity::Restricted {
            out.push_str(&delegate.name_cell(ix).primary);
        }
        return;
    }
    let mut cell = CellOut {
        want_text: true,
        ..CellOut::default()
    };
    delegate.cell(ix, id(column), &mut cell);
    if let Some(text) = cell.arc {
        out.push_str(&text);
    } else {
        out.push_str(&cell.text);
    }
}

fn address_parts(value: &str) -> (&str, &str) {
    if value == "*:*" {
        return ("*", "*");
    }
    if let Some(rest) = value.strip_prefix('[')
        && let Some((address, port)) = rest.rsplit_once("]:")
    {
        return (address, port);
    }
    value.rsplit_once(':').unwrap_or((value, "*"))
}

fn kernel_owned(inode: u64, owners_empty: bool) -> bool {
    inode == 0 && owners_empty
}

fn proto(value: tl_model::actions::Proto) -> &'static str {
    match value {
        tl_model::actions::Proto::Tcp => "TCP",
        tl_model::actions::Proto::Udp => "UDP",
    }
}

// `SysCopySource` predates system-table column ids. These aliases are private transport values;
// clipboard labels and text always come from the Connections columns.
pub(super) fn column_alias(column: ConnColumn) -> ColumnId {
    match column {
        ConnColumn::Proto => ColumnId::Type,
        ConnColumn::Local => ColumnId::Network,
        ConnColumn::Foreign => ColumnId::NetRecvRate,
        ConnColumn::State => ColumnId::State,
        ConnColumn::Pid => ColumnId::Pid,
        ConnColumn::Process => ColumnId::Name,
        ConnColumn::User => ColumnId::User,
        ConnColumn::Family => ColumnId::Architecture,
        ConnColumn::LocalPort => ColumnId::IoReadOps,
        ConnColumn::ForeignPort => ColumnId::IoWriteOps,
        ConnColumn::RecvQ => ColumnId::IoReadBytes,
        ConnColumn::SendQ => ColumnId::IoWriteBytes,
        ConnColumn::Interface => ColumnId::Tty,
        ConnColumn::Inode => ColumnId::Handles,
        ConnColumn::Service => ColumnId::Unit,
        ConnColumn::RemoteHost => ColumnId::Description,
        ConnColumn::Netns => ColumnId::Container,
    }
}

fn alias_column(alias: ColumnId) -> ConnColumn {
    match alias {
        ColumnId::Type => ConnColumn::Proto,
        ColumnId::Network => ConnColumn::Local,
        ColumnId::NetRecvRate => ConnColumn::Foreign,
        ColumnId::State => ConnColumn::State,
        ColumnId::Pid => ConnColumn::Pid,
        ColumnId::Name => ConnColumn::Process,
        ColumnId::User => ConnColumn::User,
        ColumnId::Architecture => ConnColumn::Family,
        ColumnId::IoReadOps => ConnColumn::LocalPort,
        ColumnId::IoWriteOps => ConnColumn::ForeignPort,
        ColumnId::IoReadBytes => ConnColumn::RecvQ,
        ColumnId::IoWriteBytes => ConnColumn::SendQ,
        ColumnId::Tty => ConnColumn::Interface,
        ColumnId::Handles => ConnColumn::Inode,
        ColumnId::Unit => ConnColumn::Service,
        ColumnId::Description => ConnColumn::RemoteHost,
        ColumnId::Container => ConnColumn::Netns,
        _ => ConnColumn::Process,
    }
}
