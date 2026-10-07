//! Shared frame for the Services, Startup Apps and Connections tables (UI-SYS-001, UI-SYS-002, UI-SYS-004, UI-SYS-005, UI-SYS-007).
//!
//! `SysQuery` owns one table's query and its generation (ADR-009): the engine sorts, filters and
//! flattens, and a page shows a view only when `accepts` says its generation is the last one
//! sent. `SysPageState` turns the page's demand on while its tab is active and asks for a
//! refresh on the first visit (ADR-008, R-011).

use std::sync::Arc;

use gpui_kit::{App, Entity, EntityId};
use smallvec::SmallVec;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, Tab};
use tl_model::demand::Demand;
use tl_model::health::SourceId;
use tl_model::table::{ServiceFilter, SysNodeId, SysSortSpec, SysTable, SysTableQuery};
use tl_model::validity::Validity;
use tracing::debug;

use crate::app::EngineGlobal;
use crate::demand::DemandAggregator;

/// Query owner for one system table. Generations start at 1 and only grow.
pub struct SysQuery {
    current: SysTableQuery,
    /// Generation of the last query handed to `EngineClient::send`, 0 before the first.
    last_sent: u64,
    /// The last send returned `Err(CommandError::Busy)`.
    pending_resend: bool,
}

impl SysQuery {
    /// Nothing is sent until a setter, `resend` or the first `SysPageState::on_activated`.
    pub fn new(table: SysTable, sort: SysSortSpec, services: Option<ServiceFilter>) -> Self {
        Self {
            current: SysTableQuery {
                table,
                generation: 0,
                sort,
                filter: Arc::from(""),
                expanded: Arc::from([]),
                services,
            },
            last_sent: 0,
            pending_resend: false,
        }
    }

    pub fn query(&self) -> &SysTableQuery {
        &self.current
    }

    pub fn last_sent(&self) -> u64 {
        self.last_sent
    }

    /// `text` arrives debounced by the shell's `FilterBox` (UI-SHL-024). It is never logged.
    pub fn set_filter(&mut self, text: &str, client: &dyn EngineClient) {
        self.current.filter = Arc::from(text.to_lowercase());
        self.send(client);
    }

    pub fn set_sort(&mut self, sort: SysSortSpec, client: &dyn EngineClient) {
        self.current.sort = sort;
        self.send(client);
    }

    pub fn set_expanded(&mut self, expanded: Arc<[SysNodeId]>, client: &dyn EngineClient) {
        self.current.expanded = expanded;
        self.send(client);
    }

    pub fn set_services_filter(&mut self, f: ServiceFilter, client: &dyn EngineClient) {
        self.current.services = Some(f);
        self.send(client);
    }

    /// Sends the current query again with a new generation (after `Busy` and on first visit).
    pub fn resend(&mut self, client: &dyn EngineClient) {
        self.send(client);
    }

    /// Call on every snapshot install: resends once if the last send was refused as busy.
    pub fn on_snapshot(&mut self, client: &dyn EngineClient) {
        if self.pending_resend {
            self.send(client);
        }
    }

    /// True when a view with this generation may be shown.
    pub fn accepts(&self, query_generation: u64) -> bool {
        query_generation == self.last_sent
    }

    /// Sends `current` with the next generation. The generation counts as sent even when the
    /// engine is busy, so older views are refused; `on_snapshot` then resends once (B8-E27-1).
    fn send(&mut self, client: &dyn EngineClient) {
        let generation = self.last_sent + 1;
        self.current.generation = generation;
        self.last_sent = generation;
        self.pending_resend = false;
        match client.send(Command::SetSysTableQuery(self.current.clone())) {
            Ok(()) => {}
            Err(CommandError::Busy) => self.pending_resend = true,
            Err(e) => {
                debug!(target: "tasklens::ui", generation, error = ?e, "system table query not sent");
            }
        }
    }
}

/// Per-page session state shared by the three pages.
pub struct SysPageState {
    /// `Tab::Services`, `Tab::Startup` or `Tab::Connections`.
    pub tab: Tab,
    /// Services: `SystemdSystem`, `SystemdUser`. Startup: `Startup`. Connections: `Connections`.
    pub source_ids: SmallVec<[SourceId; 2]>,
    pub visible: bool,
    pub first_visit_done: bool,
    /// Never resets in the session (APP-059).
    pub first_data_seen: bool,
}

impl SysPageState {
    pub fn new(tab: Tab, source_ids: &[SourceId]) -> Self {
        Self {
            tab,
            source_ids: source_ids.iter().copied().collect(),
            visible: false,
            first_visit_done: false,
            first_data_seen: false,
        }
    }

    /// The tab became active: turns the page's demand on. The first call of the session also
    /// sends `RefreshNow` for each source and the query.
    pub fn on_activated(
        &mut self,
        query: &mut SysQuery,
        demand: &Entity<DemandAggregator>,
        cx: &mut App,
    ) {
        self.visible = true;
        set_table_demand(self.tab, true, demand, cx);
        if self.first_visit_done {
            return;
        }
        self.first_visit_done = true;
        let Some(client) = cx.try_global::<EngineGlobal>().map(|g| Arc::clone(&g.0)) else {
            return;
        };
        for id in &self.source_ids {
            if let Err(e) = client.send(Command::RefreshNow(*id)) {
                debug!(target: "tasklens::ui", source = ?id, error = ?e, "refresh not sent");
            }
        }
        query.resend(&*client);
    }

    /// The tab was left: withdraws the page's demand.
    pub fn on_deactivated(&mut self, demand: &Entity<DemandAggregator>, cx: &mut App) {
        self.visible = false;
        set_table_demand(self.tab, false, demand, cx);
    }

    /// Marks first data seen when `validity` is not `WarmingUp` or there are rows. True only on
    /// the transition.
    pub fn note_data(&mut self, validity: Validity, rows_len: usize) -> bool {
        if self.first_data_seen || (validity == Validity::WarmingUp && rows_len == 0) {
            return false;
        }
        self.first_data_seen = true;
        true
    }
}

/// The aggregator source of each page's part. `EntityId` is a slotmap key, and these indexes
/// near `u32::MAX` are never reached by a live entity.
fn demand_source(tab: Tab) -> EntityId {
    EntityId::from(0xFFFF_FF00 + tab as u64)
}

/// Turns the Demand flag a page owns on or off: Services `services`, Startup `startup`,
/// Connections `connections`. On, the part also sets `any_surface_visible`, without which the
/// aggregator drops it, and carries the main window, so a hidden window turns it off. Other
/// tabs own no system-table flag and change nothing.
pub fn set_table_demand(tab: Tab, on: bool, demand: &Entity<DemandAggregator>, cx: &mut App) {
    let mut part = Demand {
        any_surface_visible: true,
        ..Demand::default()
    };
    match tab {
        Tab::Services => part.services = true,
        Tab::Startup => part.startup = true,
        Tab::Connections => part.connections = true,
        _ => return,
    }
    let source = demand_source(tab);
    demand.update(cx, |d, cx| {
        if on {
            let window = d.main_window();
            d.set(source, window, part, cx);
        } else {
            d.remove(source, cx);
        }
    });
}
