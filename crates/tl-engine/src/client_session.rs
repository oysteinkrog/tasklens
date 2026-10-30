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
//! Ids a client picks (actions, string scans, handle searches, ejects) are its own. The reducer
//! gives each one a new engine-wide id from one counter before it reaches the workers or the
//! action gate, and maps it back on the way out: action events go to the session's own event
//! channel with the client's id, and each session's details and device details show only its
//! own scans, handle search and ejects, under its own ids. The local session is mapped the same
//! way, so two windows never see each other's results.
//!
//! `crate::session` is the ADR-004 restart session, which is something else. The registry lives
//! in the reducer, so a reducer restart drops every session but the local one.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use arc_swap::ArcSwap;
use rustc_hash::FxHashMap;
use tl_model::actions::ActionEvent;
use tl_model::command::Command;
use tl_model::details::{
    DetailTab, DetailTarget, DetailsSnapshot, HandleSearchState, ScanId, SearchId, StringScanState,
    TabPayload, TabSlot,
};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ConnectionsSnapshot, DeviceDetailsSnapshot, DiskDetails,
    ServicesSnapshot, Snapshot, StartupSnapshot,
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

/// Client ids kept per kind and session beyond the live ones. A mapping whose engine id the
/// shared details or device details still show is live and always kept, so a long scan keeps
/// its results and can be cancelled however many requests come after it. The newest `IDS_KEPT`
/// are kept too, for cancels of requests the workers have not reported on yet.
/// The shared parts hold only the latest scan per target, one handle search and the latest
/// eject per disk, so the live ones are few.
pub const IDS_KEPT: usize = 64;

/// The client-chosen ids the reducer maps into the engine-wide namespace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IdKind {
    /// `ScanId` of `StringScan` and `CancelStringScan`.
    Scan,
    /// `SearchId` of `HandleSearch` and `CancelHandleSearch`.
    Search,
    /// `seq` of `DeviceCommand::Eject`.
    Eject,
}

impl IdKind {
    fn index(self) -> usize {
        self as usize
    }
}

/// One session's client ids of one kind and the engine ids they got, oldest first. `prune`
/// keeps the live ones and the newest `IDS_KEPT`.
#[derive(Default)]
struct IdMap(VecDeque<(u64, u64)>);

impl IdMap {
    fn insert(&mut self, client: u64, engine: u64) {
        self.0.push_back((client, engine));
    }

    /// Drops the mappings older than the newest `IDS_KEPT` whose engine id is not in `live`.
    fn prune(&mut self, live: &[u64]) {
        let old = self.0.len().saturating_sub(IDS_KEPT);
        let mut i = 0;
        self.0.retain(|(_, engine)| {
            i += 1;
            i > old || live.contains(engine)
        });
    }

    /// The newest engine id given to `client`.
    fn engine(&self, client: u64) -> Option<u64> {
        self.0
            .iter()
            .rev()
            .find(|(c, _)| *c == client)
            .map(|(_, e)| *e)
    }

    fn client(&self, engine: u64) -> Option<u64> {
        self.0.iter().find(|(_, e)| *e == engine).map(|(c, _)| *c)
    }
}

/// The last shared `Arc`s this session rewrote and what they became, so an unchanged shared
/// payload keeps its rewritten `Arc` (ADR-003 pointer equality). `None` means hidden.
#[derive(Default)]
struct ViewCache {
    strings: FxHashMap<DetailTarget, (Arc<TabPayload>, Option<Arc<TabPayload>>)>,
    handle_search: Option<(Arc<HandleSearchState>, Option<Arc<HandleSearchState>>)>,
    disks: Option<(DiskList, DiskList)>,
}

type DiskList = Arc<[(DeviceIdx, DiskDetails)]>;

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
    /// The session's action events, with its own action ids.
    pub events_tx: async_channel::Sender<ActionEvent>,
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
    /// A command from session `id`. Table and system table queries apply to that session only,
    /// and its client ids are mapped into the engine-wide namespace; every other command acts as
    /// it does for the local session.
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
    /// The shared details and device details with only this session's scans, handle search and
    /// ejects, under its own ids. The header is the shared one.
    details: Arc<DetailsSnapshot>,
    device_details: Arc<DeviceDetailsSnapshot>,
    /// Client ids to engine ids, in `IdKind` order.
    ids: [IdMap; 3],
    cache: ViewCache,
    /// Parts this session rebuilt since its last publish, plus `DETAILS` and `DEVICE_DETAILS`
    /// when its views of those must be built.
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
            details: Arc::clone(&empty.details),
            device_details: Arc::clone(&empty.device_details),
            ids: Default::default(),
            cache: ViewCache::default(),
            dirty: ComponentMask::DETAILS | ComponentMask::DEVICE_DETAILS,
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

    pub fn details(&self) -> &Arc<DetailsSnapshot> {
        &self.details
    }

    pub fn device_details(&self) -> &Arc<DeviceDetailsSnapshot> {
        &self.device_details
    }

    /// The session's action event channel. `None` for the local session, whose events go to
    /// the reducer's own channel.
    pub fn events(&self) -> Option<&async_channel::Sender<ActionEvent>> {
        self.link.as_ref().map(|l| &l.events_tx)
    }

    /// The engine id the session's `client` id of `kind` got last, if it is still kept.
    pub fn engine_id(&self, kind: IdKind, client: u64) -> Option<u64> {
        self.ids[kind.index()].engine(client)
    }

    /// The session's own id for engine id `engine` of `kind`, if the session owns it.
    pub fn client_id(&self, kind: IdKind, engine: u64) -> Option<u64> {
        self.ids[kind.index()].client(engine)
    }

    /// Prunes each id map, keeping the mappings whose engine id `c` still shows: the scan in
    /// each target's Strings tab, the handle search and each disk's eject.
    fn prune_ids(&mut self, c: &Components) {
        if self.ids.iter().all(|m| m.0.len() <= IDS_KEPT) {
            return;
        }
        let scans: Vec<u64> = c
            .details
            .targets
            .values()
            .filter_map(
                |detail| match detail.tab(DetailTab::Strings).payload.as_deref()? {
                    TabPayload::Strings(state) => Some(state.scan.0),
                    _ => None,
                },
            )
            .collect();
        let searches: Vec<u64> = c.details.handle_search.iter().map(|s| s.search.0).collect();
        let ejects: Vec<u64> = c
            .device_details
            .disks
            .iter()
            .filter_map(|(_, disk)| disk.eject.as_ref().map(|(seq, _)| *seq))
            .collect();
        self.ids[IdKind::Scan.index()].prune(&scans);
        self.ids[IdKind::Search.index()].prune(&searches);
        self.ids[IdKind::Eject.index()].prune(&ejects);
    }

    /// `shared` with this session's string scans and handle search under its own ids, and the
    /// other sessions' hidden. Returns `shared` itself when nothing changes.
    fn view_details(&mut self, shared: &Arc<DetailsSnapshot>) -> Arc<DetailsSnapshot> {
        let scans = &self.ids[IdKind::Scan.index()];
        let mut old = std::mem::take(&mut self.cache.strings);
        let mut targets = None;
        for (target, detail) in &shared.targets {
            let slot = detail.tab(DetailTab::Strings);
            let Some(src) = slot.payload.as_ref() else {
                continue;
            };
            let TabPayload::Strings(state) = &**src else {
                continue;
            };
            let out = match old.remove(target) {
                Some((s, out)) if Arc::ptr_eq(&s, src) => out,
                _ => scans.client(state.scan.0).map(|client| {
                    Arc::new(TabPayload::Strings(Arc::new(StringScanState {
                        scan: ScanId(client),
                        ..StringScanState::clone(state)
                    })))
                }),
            };
            self.cache
                .strings
                .insert(*target, (Arc::clone(src), out.clone()));
            let mut next = (**detail).clone();
            let tab = &mut next.tabs[DetailTab::Strings.index()];
            match out {
                Some(payload) => tab.payload = Some(payload),
                None => {
                    *tab = TabSlot {
                        version: tab.version,
                        ..TabSlot::default()
                    };
                }
            }
            targets
                .get_or_insert_with(|| shared.targets.clone())
                .insert(*target, Arc::new(next));
        }

        let searches = &self.ids[IdKind::Search.index()];
        let handle_search = shared.handle_search.as_ref().map(|src| {
            let out = match self.cache.handle_search.take() {
                Some((s, out)) if Arc::ptr_eq(&s, src) => out,
                _ => searches.client(src.search.0).map(|client| {
                    Arc::new(HandleSearchState {
                        search: SearchId(client),
                        ..HandleSearchState::clone(src)
                    })
                }),
            };
            self.cache.handle_search = Some((Arc::clone(src), out.clone()));
            out
        });
        if handle_search.is_none() {
            self.cache.handle_search = None;
        }

        if targets.is_none() && handle_search.is_none() {
            return Arc::clone(shared);
        }
        let mut next = (**shared).clone();
        if let Some(targets) = targets {
            next.targets = targets;
        }
        if let Some(search) = handle_search {
            next.handle_search = search;
        }
        Arc::new(next)
    }

    /// `shared` with this session's ejects under its own `seq` and the other sessions' hidden.
    /// Returns `shared` itself when no disk has an eject.
    fn view_device_details(
        &mut self,
        shared: &Arc<DeviceDetailsSnapshot>,
    ) -> Arc<DeviceDetailsSnapshot> {
        let src = &shared.disks;
        if src.iter().all(|(_, d)| d.eject.is_none()) {
            self.cache.disks = None;
            return Arc::clone(shared);
        }
        let disks = match self.cache.disks.take() {
            Some((s, out)) if Arc::ptr_eq(&s, src) => out,
            _ => {
                let ejects = &self.ids[IdKind::Eject.index()];
                src.iter()
                    .map(|(idx, disk)| {
                        let mut disk = disk.clone();
                        disk.eject = disk.eject.take().and_then(|(seq, state)| {
                            ejects.client(seq).map(|client| (client, state))
                        });
                        (*idx, disk)
                    })
                    .collect()
            }
        };
        self.cache.disks = Some((Arc::clone(src), Arc::clone(&disks)));
        Arc::new(DeviceDetailsSnapshot {
            disks,
            ..(**shared).clone()
        })
    }

    /// Brings the four parts up to date for a publish. A part is rebuilt when this session
    /// changed it or when `shared` (the reducer's dirty mask) has its bit; the system tables
    /// then take every field but their rows, generation and counts from `c`. A rebuilt part
    /// gets version one above its last published one. The views of details and device details
    /// are rebuilt from `c` when their bit is set; they keep the shared header.
    pub(crate) fn prepare(&mut self, shared: ComponentMask, c: &Components) {
        self.prune_ids(c);
        let dirty = std::mem::replace(&mut self.dirty, ComponentMask::NONE) | shared;
        if dirty.contains(ComponentMask::DETAILS) {
            self.details = self.view_details(&c.details);
        }
        if dirty.contains(ComponentMask::DEVICE_DETAILS) {
            self.device_details = self.view_device_details(&c.device_details);
        }
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

    /// `base` with this session's four parts and its views of details and device details.
    pub fn snapshot(&self, base: Snapshot) -> Snapshot {
        Snapshot {
            table: Arc::clone(&self.table),
            services: Arc::clone(&self.services),
            startup: Arc::clone(&self.startup),
            connections: Arc::clone(&self.connections),
            details: Arc::clone(&self.details),
            device_details: Arc::clone(&self.device_details),
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
    /// The last engine-wide id given out, for actions and every `IdKind`. Never reused, so an
    /// engine id names one request of one session.
    last_engine_id: u64,
}

impl ClientSessions {
    /// Only the local session, with the parts of `empty`.
    pub fn new(empty: &Snapshot) -> Self {
        let mut sessions = Vec::with_capacity(SESSIONS_MAX);
        sessions.push(ClientSession::new(ClientSessionId::LOCAL, None, empty));
        ClientSessions {
            sessions,
            last_engine_id: 0,
        }
    }

    /// A new engine-wide id. Ids only grow, which the eject worker relies on: it ignores a
    /// `seq` at or below the last one of a disk.
    pub fn next_engine_id(&mut self) -> u64 {
        self.last_engine_id += 1;
        self.last_engine_id
    }

    /// Gives session `id`'s `client` id of `kind` a new engine id and returns it. A client id
    /// sent again gets a new engine id; the newest one is used for cancels. `None` when the
    /// session is not open.
    pub fn map_new(&mut self, id: ClientSessionId, kind: IdKind, client: u64) -> Option<u64> {
        let index = self.sessions.iter().position(|s| s.id == id)?;
        let engine = self.next_engine_id();
        self.sessions[index].ids[kind.index()].insert(client, engine);
        Some(engine)
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
