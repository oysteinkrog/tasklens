//! System table builder: Services, Startup Apps and Connections (UI-SYS-002, R-138).
//!
//! The engine sorts, filters and flattens the three system tables from one `SysTableQuery`. The
//! reducer calls `build` whenever the matching component or its query changes (E09-027). E09
//! owns this file and its signatures; E14-A1 fills the body of `build`.

use std::sync::Arc;

use tl_model::settings::EngineSettings;
use tl_model::snapshot::{ConnectionsSnapshot, ProcessSnapshot, ServicesSnapshot, StartupSnapshot};
use tl_model::table::{
    ConnColumn, ServiceColumn, StartupColumn, SysColumnId, SysRow, SysSortSpec, SysTable,
    SysTableQuery,
};

/// Everything `build` reads. Borrowed from the reducer's current state.
pub struct SysTableInput<'a> {
    /// Units, managers, details and backend.
    pub services: &'a ServicesSnapshot,
    /// Startup entries.
    pub startup: &'a StartupSnapshot,
    /// Socket records.
    pub connections: &'a ConnectionsSnapshot,
    /// For the child process rows under a unit.
    pub processes: &'a ProcessSnapshot,
    pub settings: &'a EngineSettings,
}

/// The visible rows of one system table, in display order.
pub struct SysTableOutput {
    pub rows: Arc<[SysRow]>,
    /// Echoes `SysTableQuery::generation` so the UI can tell a stale table.
    pub query_generation: u64,
}

/// Filled by E14-A1. Until then returns empty rows with the query's generation.
pub fn build(input: &SysTableInput<'_>, query: &SysTableQuery) -> SysTableOutput {
    let _ = input;
    SysTableOutput {
        rows: Arc::from([]),
        query_generation: query.generation,
    }
}

/// The default sort of each table (R-011): Services Name ascending, Startup Status ascending,
/// Connections Proto ascending.
///
/// `SysSortSpec` holds one column. Two tables also have a fixed secondary sort that `build`
/// applies after the primary column: Startup then sorts by Name ascending, Connections by local
/// port ascending.
pub fn default_sort(table: SysTable) -> SysSortSpec {
    let column = match table {
        SysTable::Services => SysColumnId::Services(ServiceColumn::Name),
        SysTable::Startup => SysColumnId::Startup(StartupColumn::Status),
        SysTable::Connections => SysColumnId::Connections(ConnColumn::Proto),
    };
    SysSortSpec {
        column,
        ascending: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(table: SysTable, generation: u64) -> SysTableQuery {
        SysTableQuery {
            table,
            generation,
            sort: default_sort(table),
            filter: Arc::from(""),
            expanded: Arc::from([]),
            services: None,
        }
    }

    #[test]
    fn stub_echoes_generation() {
        let services = ServicesSnapshot::default();
        let startup = StartupSnapshot::default();
        let connections = ConnectionsSnapshot::default();
        let processes = ProcessSnapshot::default();
        let settings = EngineSettings::default();
        let input = SysTableInput {
            services: &services,
            startup: &startup,
            connections: &connections,
            processes: &processes,
            settings: &settings,
        };
        for table in [SysTable::Services, SysTable::Startup, SysTable::Connections] {
            let out = build(&input, &query(table, 9));
            assert!(out.rows.is_empty());
            assert_eq!(out.query_generation, 9);
        }
    }

    #[test]
    fn default_sorts_match_r011() {
        let services = default_sort(SysTable::Services);
        assert_eq!(services.column.as_str(), "name");
        assert_eq!(services.column.table(), SysTable::Services);
        assert!(services.ascending);

        let startup = default_sort(SysTable::Startup);
        assert_eq!(startup.column.as_str(), "status");
        assert_eq!(startup.column.table(), SysTable::Startup);
        assert!(startup.ascending);

        let connections = default_sort(SysTable::Connections);
        assert_eq!(connections.column.as_str(), "proto");
        assert_eq!(connections.column.table(), SysTable::Connections);
        assert!(connections.ascending);
    }
}
