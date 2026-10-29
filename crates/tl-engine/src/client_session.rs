//! Client sessions (ADR-018): one per connected window, up to `SESSIONS_MAX`.
//!
//! Each session has its own `TableQuery` and process table, and its own `SysTableQuery` for
//! Services, Startup and Connections with their rows and counts. The data under those tables
//! (processes, units, startup entries, sockets) is shared and lives in the reducer's
//! `Components`. At publish the reducer joins the shared data with each session's rows, and each
//! session versions its four parts itself, so one session's query never bumps another's.
//!
//! The local session (`ClientSessionId::LOCAL`) always exists and is first. The in-process
//! `EngineHandle` is that session: its commands come as `EngineMsg::Command` and its snapshots
//! go to the reducer's own `Publisher`. Other sessions are opened with `ClientMsg::Open` and get
//! their snapshots through their `SessionLink`.
//!
//! `crate::session` is the ADR-004 restart session, which is something else. The registry lives
//! in the reducer, so a reducer restart drops every session but the local one.

use std::fmt;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tl_model::command::Command;
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ConnectionsSnapshot, ServicesSnapshot, Snapshot,
    StartupSnapshot,
};
use tl_model::table::{ProcessTableView, SysTable, SysTableQuery, TableQuery};

use crate::reducer::sys_table::{self, SysTableInput};
use crate::reducer::{Components, bump, sys_table_index};

/// Sessions at most, the local one included (ADR-018).
pub const SESSIONS_MAX: usize = 8;

/// The four snapshot parts each session builds itself, in `ClientSession::published` order.
pub const SESSION_BITS: [ComponentMask; 4] = [
    ComponentMask::TABLE,
    ComponentMask::SERVICES,
    ComponentMask::STARTUP,
    ComponentMask::CONNECTIONS,
];

/// The server picks the id of every session but the local one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ClientSessionId(pub u32);

impl ClientSessionId {
    /// The in-process `EngineHandle`'s session.
    pub const LOCAL: Self = Self(0);
}

/// Where a session's snapshots go: the same slot and one-token wake channel as the reducer's
/// `Publisher` (ADR-005).
#[derive(Clone)]
pub struct SessionLink {
    pub latest: Arc<ArcSwap<Snapshot>>,
    pub wake_tx: async_channel::Sender<()>,
}

impl fmt::Debug for SessionLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionLink").finish_non_exhaustive()
    }
}

impl SessionLink {
    /// Stores `next`, then sends one wake token. Returns false when the wake receiver is gone.
    fn publish(&self, next: Snapshot) -> bool {
        self.latest.store(Arc::new(next));
        match self.wake_tx.try_send(()) {
            Ok(()) | Err(async_channel::TrySendError::Full(())) => true,
            Err(async_channel::TrySendError::Closed(())) => false,
        }
    }
}

/// What the server sends the reducer about a session.
#[derive(Clone, Debug)]
pub enum ClientMsg {
    Open {
        id: ClientSessionId,
        link: SessionLink,
    },
    Close(ClientSessionId),
    /// A command from session `id`. Table and system table queries apply to that session only;
    /// every other command acts as it does for the local session.
    Command {
        id: ClientSessionId,
        cmd: Box<Command>,
    },
}

/// Why `ClientSessions::open` refused a session.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
pub enum OpenError {
    #[error("{SESSIONS_MAX} sessions are open")]
    Full,
    #[error("session id in use")]
    Duplicate,
}

/// One connected window's queries and the table parts built for them.
pub struct ClientSession {
    id: ClientSessionId,
    pub query: TableQuery,
    /// The last `SetSysTableQuery` per system table: Services, Startup, Connections.
    pub sys_queries: [Option<SysTableQuery>; 3],
    table: Arc<ProcessTableView>,
    /// The session's rows, query generation and counts. The other fields are refreshed from
    /// the shared component at publish.
    services: Arc<ServicesSnapshot>,
    startup: Arc<StartupSnapshot>,
    connections: Arc<ConnectionsSnapshot>,
    /// Parts this session rebuilt since its last publish.
    dirty: ComponentMask,
    /// Header version of each part at the last publish, in `SESSION_BITS` order.
    published: [u64; SESSION_BITS.len()],
    /// `None` for the local session, which publishes through the reducer's `Publisher`.
    link: Option<SessionLink>,
}

impl ClientSession {
    /// Default queries and the parts of `empty`.
    fn new(id: ClientSessionId, link: Option<SessionLink>, empty: &Snapshot) -> Self {
        ClientSession {
            id,
            query: TableQuery::default(),
            sys_queries: [None, None, None],
            table: Arc::clone(&empty.table),
            services: Arc::clone(&empty.services),
            startup: Arc::clone(&empty.startup),
            connections: Arc::clone(&empty.connections),
            dirty: ComponentMask::NONE,
            published: [0; SESSION_BITS.len()],
            link,
        }
    }

    pub fn id(&self) -> ClientSessionId {
        self.id
    }

    /// The stored query of `table`, or the default one: generation 0, default sort, no filter,
    /// nothing expanded (E09-027).
    pub fn sys_query(&self, table: SysTable) -> SysTableQuery {
        self.sys_queries[sys_table_index(table)]
            .clone()
            .unwrap_or_else(|| SysTableQuery {
                table,
                generation: 0,
                sort: sys_table::default_sort(table),
                filter: Arc::from(""),
                expanded: Arc::from([]),
                services: None,
            })
    }

    /// Replaces the process table. Its header version should be 0; publish sets it.
    pub(crate) fn set_table(&mut self, view: ProcessTableView) {
        self.table = Arc::new(view);
        self.dirty |= ComponentMask::TABLE;
    }

    /// Rebuilds the rows of one system table from the shared data and this session's query,
    /// and echoes the query generation and stores the table's counts (E09-027).
    pub(crate) fn rebuild_sys(&mut self, table: SysTable, input: &SysTableInput<'_>) {
        let query = self.sys_query(table);
        match table {
            SysTable::Services => {
                let (out, counts, manager_counts) = sys_table::build_services(input, &query);
                let next = Arc::make_mut(&mut self.services);
                next.rows = out.rows;
                next.query_generation = out.query_generation;
                next.counts = counts;
                next.manager_counts = manager_counts;
                self.dirty |= ComponentMask::SERVICES;
            }
            SysTable::Startup => {
                let (out, counts) = sys_table::build_startup(input, &query);
                let next = Arc::make_mut(&mut self.startup);
                next.rows = out.rows;
                next.query_generation = out.query_generation;
                next.counts = counts;
                self.dirty |= ComponentMask::STARTUP;
            }
            SysTable::Connections => {
                let (out, counts) = sys_table::build_connections(input, &query);
                let next = Arc::make_mut(&mut self.connections);
                next.rows = out.rows;
                next.query_generation = out.query_generation;
                next.counts = counts;
                self.dirty |= ComponentMask::CONNECTIONS;
            }
        }
    }

    pub fn table(&self) -> &Arc<ProcessTableView> {
        &self.table
    }

    pub fn services(&self) -> &Arc<ServicesSnapshot> {
        &self.services
    }

    pub fn startup(&self) -> &Arc<StartupSnapshot> {
        &self.startup
    }

    pub fn connections(&self) -> &Arc<ConnectionsSnapshot> {
        &self.connections
    }

    /// Brings the four parts up to date for a publish. A part is rebuilt when this session
    /// changed it or when `shared` (the reducer's dirty mask) has its bit; the system tables
    /// then take every field but their rows, generation and counts from `c`. A rebuilt part
    /// gets version one above its last published one.
    pub(crate) fn prepare(&mut self, shared: ComponentMask, c: &Components) {
        let dirty = std::mem::replace(&mut self.dirty, ComponentMask::NONE) | shared;
        for (i, bit) in SESSION_BITS.into_iter().enumerate() {
            if !dirty.contains(bit) {
                continue;
            }
            let p = self.published[i];
            self.published[i] = match bit {
                ComponentMask::TABLE => bump(&mut self.table, p),
                ComponentMask::SERVICES => {
                    let own = &self.services;
                    self.services = Arc::new(ServicesSnapshot {
                        header: unversioned(c.services.header),
                        rows: Arc::clone(&own.rows),
                        query_generation: own.query_generation,
                        counts: own.counts,
                        manager_counts: own.manager_counts,
                        ..(*c.services).clone()
                    });
                    bump(&mut self.services, p)
                }
                ComponentMask::STARTUP => {
                    let own = &self.startup;
                    self.startup = Arc::new(StartupSnapshot {
                        header: unversioned(c.startup.header),
                        rows: Arc::clone(&own.rows),
                        query_generation: own.query_generation,
                        counts: own.counts,
                        ..(*c.startup).clone()
                    });
                    bump(&mut self.startup, p)
                }
                _ => {
                    let own = &self.connections;
                    self.connections = Arc::new(ConnectionsSnapshot {
                        header: unversioned(c.connections.header),
                        rows: Arc::clone(&own.rows),
                        query_generation: own.query_generation,
                        counts: own.counts,
                        ..(*c.connections).clone()
                    });
                    bump(&mut self.connections, p)
                }
            };
        }
    }

    /// `base` with this session's four parts.
    pub fn snapshot(&self, base: Snapshot) -> Snapshot {
        Snapshot {
            table: Arc::clone(&self.table),
            services: Arc::clone(&self.services),
            startup: Arc::clone(&self.startup),
            connections: Arc::clone(&self.connections),
            ..base
        }
    }
}

fn unversioned(header: ComponentHeader) -> ComponentHeader {
    ComponentHeader {
        version: 0,
        ..header
    }
}

/// The open sessions in connect order, the local one first.
pub struct ClientSessions {
    sessions: Vec<ClientSession>,
}

impl ClientSessions {
    /// Only the local session, with the parts of `empty`.
    pub fn new(empty: &Snapshot) -> Self {
        let mut sessions = Vec::with_capacity(SESSIONS_MAX);
        sessions.push(ClientSession::new(ClientSessionId::LOCAL, None, empty));
        ClientSessions { sessions }
    }

    /// Adds a session after the open ones. Its parts start as those of `empty`; the caller
    /// builds them.
    pub fn open(
        &mut self,
        id: ClientSessionId,
        link: SessionLink,
        empty: &Snapshot,
    ) -> Result<&mut ClientSession, OpenError> {
        if self.get(id).is_some() {
            return Err(OpenError::Duplicate);
        }
        if self.sessions.len() >= SESSIONS_MAX {
            return Err(OpenError::Full);
        }
        self.sessions
            .push(ClientSession::new(id, Some(link), empty));
        Ok(self.sessions.last_mut().expect("just pushed"))
    }

    /// Removes session `id`. The local session cannot be closed. Returns true when a session
    /// was removed.
    pub fn close(&mut self, id: ClientSessionId) -> bool {
        if id == ClientSessionId::LOCAL {
            return false;
        }
        let before = self.sessions.len();
        self.sessions.retain(|s| s.id != id);
        self.sessions.len() != before
    }

    pub fn get(&self, id: ClientSessionId) -> Option<&ClientSession> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: ClientSessionId) -> Option<&mut ClientSession> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub fn local(&self) -> &ClientSession {
        &self.sessions[0]
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Never true: the local session is always open.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// In connect order, the local session first.
    pub fn iter(&self) -> impl Iterator<Item = &ClientSession> {
        self.sessions.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ClientSession> {
        self.sessions.iter_mut()
    }

    /// Publishes `base` with each linked session's parts. A session whose wake receiver is
    /// gone is closed.
    pub(crate) fn publish(&mut self, base: &Snapshot) {
        self.sessions.retain(|s| match &s.link {
            None => true,
            Some(link) => {
                let kept = link.publish(s.snapshot(base.clone()));
                if !kept {
                    tracing::debug!(target: "tasklens::engine", id = s.id.0, "client session gone");
                }
                kept
            }
        });
    }
}
