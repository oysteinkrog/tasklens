//! Connections copy commands (UI-SYS-006, UI-SYS-063 and UI-SYS-066).

use std::fmt::Write as _;

use gpui_kit::{App, SharedString};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::process::ProcessRecord;
use tl_model::snapshot::ConnRecord;
use tl_model::table::{ConnColumn, SysColumnId};
use tl_model::units;
use tl_model::validity::Validity;

use super::ConnectionsPage;
use super::delegate::id;
use crate::pages::sys_copy::{CopyTexts, SysCopySource};
use crate::table::TreeTableDelegate as _;
use crate::table::cells::CellOut;

#[derive(Default)]
pub(super) struct PreparedCopy {
    rows: SmallVec<[u64; 8]>,
    columns: SmallVec<[(SharedString, ColumnId); 24]>,
    states: SmallVec<[(u64, SharedString); 8]>,
    processes: SmallVec<[(u64, SharedString); 8]>,
}

impl SysCopySource for ConnectionsPage {
    type RowId = u64;

    fn copy_rows(&self) -> SmallVec<[Self::RowId; 8]> {
        self.copy_prepared.rows.clone()
    }

    fn write_block(&self, row: Self::RowId, out: &mut String) {
        let Some(record) = self.copy_record(row) else {
            return;
        };
        write_block(self, record, out);
    }

    fn visible_columns(&self) -> SmallVec<[(SharedString, ColumnId); 24]> {
        self.copy_prepared.columns.clone()
    }

    fn write_cell(&self, row: Self::RowId, col: ColumnId, out: &mut String) {
        let Some(record) = self.copy_record(row) else {
            return;
        };
        write_cell(self, record, alias_column(col), out);
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
        let mut states = SmallVec::new();
        let mut processes = SmallVec::new();
        for cookie in prepared_rows {
            let Some(ix) = delegate.row_index(cookie) else {
                continue;
            };
            let Some(record) = delegate.record(ix) else {
                continue;
            };
            let mut state = CellOut::default();
            delegate.cell(ix, id(ConnColumn::State), &mut state);
            states.push((
                cookie,
                state.arc.map(SharedString::from).unwrap_or_default(),
            ));
            let process = if record.owner_validity == Validity::Restricted {
                SharedString::default()
            } else {
                delegate.name_cell(ix).primary
            };
            processes.push((cookie, process));
        }
        self.copy_prepared = PreparedCopy {
            rows,
            columns,
            states,
            processes,
        };
    }

    fn copy_record(&self, cookie: u64) -> Option<&ConnRecord> {
        self.copy_connections
            .records
            .iter()
            .find(|record| record.cookie == cookie)
    }

    fn prepared_state(&self, cookie: u64) -> &str {
        self.copy_prepared
            .states
            .iter()
            .find_map(|(row, text)| (*row == cookie).then_some(text.as_ref()))
            .unwrap_or_else(|| {
                self.copy_record(cookie)
                    .map_or("", |record| record.state.label())
            })
    }

    fn prepared_process(&self, cookie: u64) -> &str {
        self.copy_prepared
            .processes
            .iter()
            .find_map(|(row, text)| (*row == cookie).then_some(text.as_ref()))
            .unwrap_or("")
    }

    fn owner<'a>(&'a self, record: &ConnRecord) -> Option<&'a ProcessRecord> {
        record
            .owners
            .iter()
            .filter_map(|key| self.copy_processes.get(*key))
            .min_by_key(|process| process.key.pid)
    }

    fn user<'a>(&'a self, record: &ConnRecord) -> Option<&'a str> {
        self.owner(record)
            .map(|process| process.meta.user.as_ref())
            .or_else(|| {
                self.copy_processes
                    .records
                    .iter()
                    .find(|process| process.uid == record.uid)
                    .map(|process| process.meta.user.as_ref())
            })
    }
}

fn write_block(page: &ConnectionsPage, record: &ConnRecord, out: &mut String) {
    let (local_address, local_port) = address_parts(&record.local);
    let (foreign_address, foreign_port) = address_parts(&record.foreign);
    let _ = writeln!(out, "Protocol: {}", proto(record.proto));
    let _ = writeln!(out, "Local address: {local_address}");
    let _ = writeln!(out, "Local port: {local_port}");
    let _ = writeln!(out, "Foreign address: {foreign_address}");
    let _ = writeln!(out, "Foreign port: {foreign_port}");
    let _ = writeln!(out, "State: {}", page.prepared_state(record.cookie));
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
        out.push_str(page.prepared_process(record.cookie));
    }
}

fn write_cell(page: &ConnectionsPage, record: &ConnRecord, column: ConnColumn, out: &mut String) {
    match column {
        ConnColumn::Proto => out.push_str(proto(record.proto)),
        ConnColumn::Local => out.push_str(&record.local),
        ConnColumn::Foreign => out.push_str(&record.foreign),
        ConnColumn::State => out.push_str(page.prepared_state(record.cookie)),
        ConnColumn::Pid => {
            if record.owner_validity == Validity::Restricted {
                out.push('?');
            } else if kernel_owned(record.inode, record.owners.is_empty()) {
                out.push('-');
            } else if let Some(owner) = page.owner(record) {
                let _ = write!(out, "{}", owner.key.pid);
                let extra = record.owners.len().saturating_sub(1);
                if extra > 0 {
                    let _ = write!(out, " +{extra}");
                }
            } else {
                out.push('?');
            }
        }
        ConnColumn::Process => out.push_str(page.prepared_process(record.cookie)),
        ConnColumn::User => {
            if let Some(user) = page.user(record) {
                out.push_str(user);
            }
        }
        ConnColumn::Family => out.push_str(match record.family {
            tl_model::actions::Family::Inet => "IPv4",
            tl_model::actions::Family::Inet6 => "IPv6",
        }),
        ConnColumn::LocalPort => {
            let _ = write!(out, "{}", record.four_tuple.local.port());
        }
        ConnColumn::ForeignPort => {
            if !record.foreign.ends_with(":*") {
                let _ = write!(out, "{}", record.four_tuple.remote.port());
            }
        }
        ConnColumn::RecvQ => units::count(out, u64::from(record.recv_q)),
        ConnColumn::SendQ => units::count(out, u64::from(record.send_q)),
        ConnColumn::Interface => {
            if let Some(interface) = &record.interface {
                out.push_str(interface);
            }
        }
        ConnColumn::Inode => {
            let _ = write!(out, "{}", record.inode);
        }
        ConnColumn::Service => {
            if let Some(unit) = &record.unit {
                out.push_str(&unit.name);
            }
        }
        ConnColumn::RemoteHost => {
            if let Some(host) = record.remote_host.name() {
                out.push_str(host);
            }
        }
        ConnColumn::Netns => {
            if let Some(netns) = &record.netns {
                out.push_str(&netns.name);
            }
        }
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
