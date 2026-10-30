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
//! action gate, and maps it back on the way out, so two windows never see each other's results.
//! Actions keep no mapping here: the gate routes each one by engine id until `ActionFinished`.
//!
//! Request lifecycle (scans, handle searches, ejects). Every request a session sends ends with
//! exactly one `RequestEvent` on the session's own request channel, and one owner sends it:
//! - the reducer, as queue owner, for a request no worker took: `Busy` at admission (no mapping
//!   is made), `Lost` when the control watch is closed or its command leaves the worker queue
//!   before a worker claimed it (`CommandProgress::evict`, under the lock the worker's
//!   `CommandProgress::claim` takes, so a command is either evicted or claimed);
//! - the worker for a request it claimed: a terminal state in its update, `ScanDropped`,
//!   `EjectDropped`, or a newer request it reported in the same slot (the workers handle a slot's
//!   requests in engine id order and keep one per slot);
//! - the new worker generation, for the requests the stopped one claimed (`RequestsLost`,
//!   `EjectsLost`): `Lost`.
//!
//! The mapping of a request is removed exactly when its event is sent. A report about a request
//! that has no mapping is not shown to any session and is counted in `ClientSessions::late`.
//! When a session ends, its mappings go with it and no event is sent. A session has at most
//! `ACTIVE_MAX` active requests per kind.
//!
//! Each session shows, per slot, its newest request: the shared value under the session's own
//! id while the request is active, the value it had when the request ended after that. A request
//! that ended without a value of its own (lost, dropped or replaced) leaves the slot empty. A
//! slot that left the shared data (a detail target or disk gone, the handle search cleared)
//! shows nothing, and is forgotten when it has no active request.
//!
//! `crate::session` is the ADR-004 restart session, which is something else. The registry lives
//! in the reducer, so a reducer restart drops every session but the local one.

use std::fmt;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tl_model::actions::ActionEvent;
use tl_model::command::Command;
use tl_model::details::{
    DetailTab, DetailTarget, DetailsSnapshot, HandlePhase, HandleSearchState, ScanId, ScanPhase,
    SearchId, StringScanState, TabPayload, TabSlot,
};
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, ProcessKey};
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ConnectionsSnapshot, DeviceDetailsSnapshot, DiskDetails,
    EjectState, ServicesSnapshot, Snapshot, StartupSnapshot,
};
use tl_model::table::{ProcessTableView, SysTable, SysTableQuery, TableQuery};
use tl_model::validity::Validity;

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

/// Active requests per kind and session (scans, handle searches, ejects). One more is refused
/// with `RequestEnd::Busy` until one of them ends.
pub const ACTIVE_MAX: usize = 64;

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

    /// The snapshot part that shows requests of this kind.
    fn view_bit(self) -> ComponentMask {
        match self {
            IdKind::Scan | IdKind::Search => ComponentMask::DETAILS,
            IdKind::Eject => ComponentMask::DEVICE_DETAILS,
        }
    }
}

/// What one request works on. The workers keep one request per slot and handle a slot's
/// requests in engine id order, so a newer engine id reported in a slot ends every older one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IdSlot {
    /// A Strings scan of this process.
    Scan(ProcessKey),
    /// The one handle search.
    Search,
    /// An eject of this disk, by kernel name.
    Eject(Arc<str>),
}

impl IdSlot {
    pub fn kind(&self) -> IdKind {
        match self {
            IdSlot::Scan(_) => IdKind::Scan,
            IdSlot::Search => IdKind::Search,
            IdSlot::Eject(_) => IdKind::Eject,
        }
    }
}

/// How a request ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RequestEnd {
    /// A scan `Finished`, `StoppedLimit` or `StoppedExited`; a handle search or eject `Done`.
    Done,
    /// A scan or handle search `Failed`; an eject `Failed` or `Blocked`.
    Failed,
    /// A scan `Cancelled`, a handle search `Stopped`, or the worker dropped the request or ran a
    /// newer one in its slot instead.
    Cancelled,
    /// Refused at admission: `ACTIVE_MAX` requests of its kind were active.
    Busy,
    /// The engine lost it: its command never reached a worker, or the worker that claimed it
    /// stopped.
    Lost,
}

/// The one end of one request, sent on its session's request channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RequestEvent {
    pub kind: IdKind,
    /// The session's own id: `ScanId`, `SearchId` or eject `seq`.
    pub id: u64,
    pub end: RequestEnd,
}

/// A worker's report of one request, as the shared data shows it.
#[derive(Clone, Copy, Debug)]
pub enum Report<'a> {
    /// A Strings tab payload. Other payloads are not reports.
    Scan(&'a Arc<TabPayload>),
    Search(&'a Arc<HandleSearchState>),
    Eject(&'a EjectState),
}

impl Report<'_> {
    /// The end this report names, or `None` while the request runs.
    pub fn end(&self) -> Option<RequestEnd> {
        match self {
            Report::Scan(payload) => match &***payload {
                TabPayload::Strings(state) => match state.phase {
                    ScanPhase::Scanning => None,
                    ScanPhase::Finished | ScanPhase::StoppedLimit | ScanPhase::StoppedExited => {
                        Some(RequestEnd::Done)
                    }
                    ScanPhase::Cancelled => Some(RequestEnd::Cancelled),
                    ScanPhase::Failed(_) => Some(RequestEnd::Failed),
                },
                _ => None,
            },
            Report::Search(state) => match state.phase {
                HandlePhase::Initializing | HandlePhase::Retrieving | HandlePhase::Searching => {
                    None
                }
                HandlePhase::Done => Some(RequestEnd::Done),
                HandlePhase::Stopped => Some(RequestEnd::Cancelled),
                HandlePhase::Failed => Some(RequestEnd::Failed),
            },
            Report::Eject(state) => match state {
                EjectState::Running => None,
                EjectState::Done => Some(RequestEnd::Done),
                EjectState::Blocked { .. } | EjectState::Failed(_) => Some(RequestEnd::Failed),
            },
        }
    }

    /// The report under the session's own id `client`.
    fn view(&self, client: u64) -> View {
        match self {
            Report::Scan(src) => {
                let out = match &***src {
                    TabPayload::Strings(state) => {
                        Arc::new(TabPayload::Strings(Arc::new(StringScanState {
                            scan: ScanId(client),
                            ..StringScanState::clone(state)
                        })))
                    }
                    _ => Arc::clone(src),
                };
                View::Scan {
                    src: Arc::clone(src),
                    out,
                }
            }
            Report::Search(src) => View::Search {
                src: Arc::clone(src),
                out: Arc::new(HandleSearchState {
                    search: SearchId(client),
                    ..HandleSearchState::clone(src)
                }),
            },
            Report::Eject(state) => View::Eject((client, (*state).clone())),
        }
    }
}

/// What a session shows in one slot. `src` is the shared value the view was made from, so an
/// unchanged shared value keeps its rewritten `Arc` (ADR-003 pointer equality).
#[derive(Clone)]
enum View {
    Scan {
        src: Arc<TabPayload>,
        out: Arc<TabPayload>,
    },
    Search {
        src: Arc<HandleSearchState>,
        out: Arc<HandleSearchState>,
    },
    Eject(Eject),
}

type Eject = (u64, EjectState);

/// Why `ClientSessions::admit` gave no engine id.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
pub enum MapError {
    #[error("session not open")]
    NoSession,
    /// The session got `RequestEnd::Busy` for it.
    #[error("{ACTIVE_MAX} requests of this kind are active")]
    Busy,
}

/// One active request: a mapping from the session's id to the engine's.
struct Request {
    client: u64,
    engine: u64,
    slot: IdSlot,
    /// The worker queue `seq` of its command, 0 until it is forwarded.
    queued: u64,
}

/// A session's newest request in one slot and what the session shows there.
struct Shown {
    slot: IdSlot,
    engine: u64,
    client: u64,
    /// `None` until the worker reports the request, and after it ended without a value.
    view: Option<View>,
}

/// The index of the disk with kernel name `disk` in the device catalog.
fn disk_idx(c: &Components, disk: &str) -> Option<DeviceIdx> {
    c.devices
        .entries
        .iter()
        .find(|e| matches!(&e.identity, DeviceIdentity::Disk { name, .. } if **name == *disk))
        .map(|e| e.idx)
}

type DiskList = Arc<[(DeviceIdx, DiskDetails)]>;

/// The server picks the id of every session but the local one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ClientSessionId(pub u32);

impl ClientSessionId {
    /// The in-process `EngineHandle`'s session.
    pub const LOCAL: Self = Self(0);
}

/// Where a session's snapshots and events go: the same slot and one-token wake channel as the
/// reducer's `Publisher` (ADR-005), and its two ordered event channels.
#[derive(Clone)]
pub struct SessionLink {
    pub latest: Arc<ArcSwap<Snapshot>>,
    pub wake_tx: async_channel::Sender<()>,
    /// The session's action events, with its own action ids.
    pub events_tx: async_channel::Sender<ActionEvent>,
    /// The end of each of the session's scans, handle searches and ejects, with its own ids.
    /// A full or closed channel ends the session.
    pub requests_tx: async_channel::Sender<RequestEvent>,
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
    /// The active requests, in `IdKind` order, oldest first.
    requests: [Vec<Request>; 3],
    /// Per slot, the newest request the session sent there.
    shown: Vec<Shown>,
    /// The shared disk list and the session's view of it, while no eject view changed.
    disks: Option<(DiskList, DiskList)>,
    /// `None` for the local session until `ClientSessions::set_local_requests`.
    requests_tx: Option<async_channel::Sender<RequestEvent>>,
    /// The request channel was full or closed: the session ends at the next publish.
    broken: bool,
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
            requests: Default::default(),
            shown: Vec::new(),
            disks: None,
            requests_tx: link.as_ref().map(|l| l.requests_tx.clone()),
            broken: false,
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

    /// The engine id of the session's newest active request with `client` id of `kind`.
    pub fn engine_id(&self, kind: IdKind, client: u64) -> Option<u64> {
        self.requests[kind.index()]
            .iter()
            .rev()
            .find(|r| r.client == client)
            .map(|r| r.engine)
    }

    /// The session's own id for active request `engine` of `kind`.
    pub fn client_id(&self, kind: IdKind, engine: u64) -> Option<u64> {
        self.requests[kind.index()]
            .iter()
            .find(|r| r.engine == engine)
            .map(|r| r.client)
    }

    /// The session's active requests of `kind`, each with its mapping.
    pub fn active(&self, kind: IdKind) -> usize {
        self.requests[kind.index()].len()
    }

    /// The slots of `kind` the session shows a request in, active or ended.
    pub fn shown(&self, kind: IdKind) -> usize {
        self.shown.iter().filter(|s| s.slot.kind() == kind).count()
    }

    fn is_active(&self, kind: IdKind, engine: u64) -> bool {
        self.requests[kind.index()]
            .iter()
            .any(|r| r.engine == engine)
    }

    /// Sends `event` on the request channel. A full or closed channel ends a linked session at
    /// the next publish, as a full action event queue does (ADR-018).
    fn send(&mut self, event: RequestEvent) {
        let Some(tx) = self.requests_tx.as_ref() else {
            tracing::debug!(target: "tasklens::engine", ?event, "local session has no request channel: event not sent");
            return;
        };
        if self.broken {
            return;
        }
        if let Err(e) = tx.try_send(event) {
            tracing::debug!(target: "tasklens::engine", id = self.id.0, ?event, full = e.is_full(), "request channel full or closed");
            self.broken = self.link.is_some();
        }
    }

    /// Ends active request `engine` of `kind`: removes its mapping and sends `end`. The slot
    /// then shows `last` when the request is still the newest there, else it keeps what it
    /// shows. False when the session does not have the request.
    fn finish(&mut self, kind: IdKind, engine: u64, end: RequestEnd, last: Option<View>) -> bool {
        let list = &mut self.requests[kind.index()];
        let Some(at) = list.iter().position(|r| r.engine == engine) else {
            return false;
        };
        let request = list.remove(at);
        if let Some(i) = self.shown.iter().position(|s| s.engine == engine) {
            match last {
                Some(view) => self.shown[i].view = Some(view),
                None => {
                    self.shown.swap_remove(i);
                }
            }
            self.view_changed(kind);
        }
        self.send(RequestEvent {
            kind,
            id: request.client,
            end,
        });
        true
    }

    fn view_changed(&mut self, kind: IdKind) {
        self.dirty |= kind.view_bit();
        if kind == IdKind::Eject {
            self.disks = None;
        }
    }

    /// Updates the views of the active requests from the shared data `c`, and forgets the
    /// slots that left it and have no active request.
    fn refresh(&mut self, shared: ComponentMask, c: &Components) {
        let mut changed = Vec::new();
        for (i, shown) in self.shown.iter().enumerate() {
            let kind = shown.slot.kind();
            if !shared.contains(kind.view_bit()) || !self.is_active(kind, shown.engine) {
                continue;
            }
            let next = match &shown.slot {
                IdSlot::Scan(key) => c
                    .details
                    .targets
                    .get(&DetailTarget::Process(*key))
                    .and_then(|d| d.tab(DetailTab::Strings).payload.as_ref())
                    .filter(|p| matches!(&***p, TabPayload::Strings(s) if s.scan.0 == shown.engine))
                    .filter(|p| !matches!(&shown.view, Some(View::Scan { src, .. }) if Arc::ptr_eq(src, p)))
                    .map(|p| Report::Scan(p).view(shown.client)),
                IdSlot::Search => c
                    .details
                    .handle_search
                    .as_ref()
                    .filter(|s| s.search.0 == shown.engine)
                    .filter(|s| !matches!(&shown.view, Some(View::Search { src, .. }) if Arc::ptr_eq(src, s)))
                    .map(|s| Report::Search(s).view(shown.client)),
                IdSlot::Eject(disk) => disk_idx(c, disk)
                    .and_then(|idx| c.device_details.disks.iter().find(|(i, _)| *i == idx))
                    .and_then(|(_, d)| d.eject.as_ref())
                    .filter(|(seq, _)| *seq == shown.engine)
                    .filter(|(_, state)| !matches!(&shown.view, Some(View::Eject((_, s))) if s == state))
                    .map(|(_, state)| Report::Eject(state).view(shown.client)),
            };
            if let Some(view) = next {
                changed.push((i, view));
            }
        }
        for (i, view) in changed {
            let kind = self.shown[i].slot.kind();
            self.shown[i].view = Some(view);
            self.view_changed(kind);
        }

        let gone: Vec<usize> = self
            .shown
            .iter()
            .enumerate()
            .filter(|(_, s)| !self.is_active(s.slot.kind(), s.engine))
            .filter(|(_, s)| match &s.slot {
                IdSlot::Scan(key) => !c.details.targets.contains_key(&DetailTarget::Process(*key)),
                IdSlot::Search => c.details.handle_search.is_none(),
                IdSlot::Eject(disk) => disk_idx(c, disk)
                    .is_none_or(|idx| !c.device_details.disks.iter().any(|(i, _)| *i == idx)),
            })
            .map(|(i, _)| i)
            .collect();
        for i in gone.into_iter().rev() {
            let kind = self.shown[i].slot.kind();
            self.shown.swap_remove(i);
            self.view_changed(kind);
        }
    }

    /// `shared` with this session's string scans and handle search under its own ids, and the
    /// other sessions' hidden. Returns `shared` itself when nothing changes.
    fn view_details(&self, shared: &Arc<DetailsSnapshot>) -> Arc<DetailsSnapshot> {
        let mut targets = None;
        for (target, detail) in &shared.targets {
            let DetailTarget::Process(key) = target else {
                continue;
            };
            let want = self.shown.iter().find_map(|s| match (&s.slot, &s.view) {
                (IdSlot::Scan(k), Some(View::Scan { src, out })) if k == key => Some((src, out)),
                _ => None,
            });
            let tab = detail.tab(DetailTab::Strings);
            let has = tab.payload.as_ref();
            let next_tab = match (want, has) {
                (Some((src, out)), _) => {
                    let from_shared = has.is_some_and(|h| Arc::ptr_eq(h, src));
                    TabSlot {
                        payload: Some(Arc::clone(out)),
                        validity: if from_shared {
                            tab.validity
                        } else {
                            Validity::Valid
                        },
                        ..tab.clone()
                    }
                }
                (None, Some(p)) if matches!(&**p, TabPayload::Strings(_)) => TabSlot {
                    version: tab.version,
                    ..TabSlot::default()
                },
                _ => continue,
            };
            let mut next = (**detail).clone();
            next.tabs[DetailTab::Strings.index()] = next_tab;
            targets
                .get_or_insert_with(|| shared.targets.clone())
                .insert(*target, Arc::new(next));
        }

        // The reducer clears the shared search once it ended and no window shows it.
        let search = self
            .shown
            .iter()
            .find_map(|s| match &s.view {
                Some(View::Search { out, .. }) => Some(Arc::clone(out)),
                _ => None,
            })
            .filter(|_| shared.handle_search.is_some());
        let search_same = match (&search, &shared.handle_search) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if targets.is_none() && search_same {
            return Arc::clone(shared);
        }
        let mut next = (**shared).clone();
        if let Some(targets) = targets {
            next.targets = targets;
        }
        next.handle_search = search;
        Arc::new(next)
    }

    /// `shared` with this session's ejects under its own `seq`, and the other sessions' hidden.
    /// Returns `shared` itself when no disk has an eject.
    fn view_device_details(&mut self, c: &Components) -> Arc<DeviceDetailsSnapshot> {
        let shared = &c.device_details;
        let src = &shared.disks;
        let ejects: Vec<(DeviceIdx, &Eject)> = self
            .shown
            .iter()
            .filter_map(|s| match (&s.slot, &s.view) {
                (IdSlot::Eject(disk), Some(View::Eject(eject))) => {
                    Some((disk_idx(c, disk)?, eject))
                }
                _ => None,
            })
            .collect();
        if ejects.is_empty() && src.iter().all(|(_, d)| d.eject.is_none()) {
            self.disks = None;
            return Arc::clone(shared);
        }
        let disks = match self.disks.take() {
            Some((s, out)) if Arc::ptr_eq(&s, src) => out,
            _ => src
                .iter()
                .map(|(idx, disk)| {
                    let mut disk = disk.clone();
                    disk.eject = ejects
                        .iter()
                        .find(|(i, _)| i == idx)
                        .map(|(_, e)| (*e).clone());
                    (*idx, disk)
                })
                .collect(),
        };
        self.disks = Some((Arc::clone(src), Arc::clone(&disks)));
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
        self.refresh(shared, c);
        let dirty = std::mem::replace(&mut self.dirty, ComponentMask::NONE) | shared;
        if dirty.contains(ComponentMask::DETAILS) {
            self.details = self.view_details(&c.details);
        }
        if dirty.contains(ComponentMask::DEVICE_DETAILS) {
            self.device_details = self.view_device_details(c);
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
    /// Reports and ends of requests that had no mapping any more: they reached no session.
    late: u64,
}

impl ClientSessions {
    /// Only the local session, with the parts of `empty`.
    pub fn new(empty: &Snapshot) -> Self {
        let mut sessions = Vec::with_capacity(SESSIONS_MAX);
        sessions.push(ClientSession::new(ClientSessionId::LOCAL, None, empty));
        ClientSessions {
            sessions,
            last_engine_id: 0,
            late: 0,
        }
    }

    /// Gives the local session a request channel. Without one its request events are logged
    /// and not sent; the in-process window reads its requests from the snapshot.
    pub fn set_local_requests(&mut self, tx: async_channel::Sender<RequestEvent>) {
        self.sessions[0].requests_tx = Some(tx);
    }

    /// A new engine-wide id. Ids only grow, which the eject worker relies on: it ignores a
    /// `seq` at or below the last one of a disk.
    pub fn next_engine_id(&mut self) -> u64 {
        self.last_engine_id += 1;
        self.last_engine_id
    }

    /// Reports and ends that arrived after their request had ended or its session had closed.
    /// For debugging only.
    pub fn late(&self) -> u64 {
        self.late
    }

    /// Admits session `id`'s request `client` in `slot`: maps it to a new engine id and returns
    /// that. A client id sent again gets a new engine id; cancels use the newest active one.
    /// When the session has `ACTIVE_MAX` active requests of the slot's kind, it gets
    /// `RequestEnd::Busy` and no mapping is made.
    pub fn admit(
        &mut self,
        id: ClientSessionId,
        slot: IdSlot,
        client: u64,
    ) -> Result<u64, MapError> {
        let index = self
            .sessions
            .iter()
            .position(|s| s.id == id)
            .ok_or(MapError::NoSession)?;
        let kind = slot.kind();
        if self.sessions[index].active(kind) >= ACTIVE_MAX {
            self.sessions[index].send(RequestEvent {
                kind,
                id: client,
                end: RequestEnd::Busy,
            });
            return Err(MapError::Busy);
        }
        let engine = self.next_engine_id();
        let session = &mut self.sessions[index];
        session.requests[kind.index()].push(Request {
            client,
            engine,
            slot: slot.clone(),
            queued: 0,
        });
        // The new request is the newest in its slot: the slot shows nothing until the worker
        // reports it.
        let had = session.shown.iter().position(|s| s.slot == slot);
        let shown = Shown {
            slot,
            engine,
            client,
            view: None,
        };
        match had {
            Some(i) => session.shown[i] = shown,
            None => session.shown.push(shown),
        }
        session.view_changed(kind);
        Ok(engine)
    }

    /// Records that request `engine` of `kind` went to the worker queue as `seq`.
    pub fn queued(&mut self, kind: IdKind, engine: u64, seq: u64) {
        let request = self.sessions.iter_mut().find_map(|s| {
            s.requests[kind.index()]
                .iter_mut()
                .find(|r| r.engine == engine)
        });
        if let Some(request) = request {
            request.queued = seq;
        }
    }

    /// Ends request `engine` of `kind` with `end`, whichever session has it. Its slot shows
    /// `last`, or nothing. Counted as late when no session has it.
    fn finish_with(&mut self, kind: IdKind, engine: u64, end: RequestEnd, last: Option<View>) {
        if !self
            .sessions
            .iter_mut()
            .any(|s| s.finish(kind, engine, end, last.clone()))
        {
            self.late += 1;
            tracing::debug!(target: "tasklens::engine", ?kind, engine, ?end, "end of a request that already ended");
        }
    }

    /// Ends request `engine` of `kind` with `end` and no value of its own: lost, dropped by
    /// the worker, or refused by a closed control watch.
    pub fn finish(&mut self, kind: IdKind, engine: u64, end: RequestEnd) {
        self.finish_with(kind, engine, end, None);
    }

    /// The worker reported request `engine` in `slot`. Every older active request in the slot,
    /// in every session, ends as `Cancelled`: the worker ran this one instead. When `report`
    /// names an end, `engine` ends with it and its slot keeps showing the report. Returns false,
    /// and counts it as late, when no session has `engine`; the report then reaches no session.
    pub fn report(&mut self, slot: &IdSlot, engine: u64, report: Report<'_>) -> bool {
        let kind = slot.kind();
        let older: Vec<u64> = self
            .sessions
            .iter()
            .flat_map(|s| s.requests[kind.index()].iter())
            .filter(|r| r.engine < engine && r.slot == *slot)
            .map(|r| r.engine)
            .collect();
        for e in older {
            self.finish(kind, e, RequestEnd::Cancelled);
        }
        let Some(session) = self.sessions.iter_mut().find(|s| s.is_active(kind, engine)) else {
            self.late += 1;
            tracing::debug!(target: "tasklens::engine", ?kind, engine, "report of a request that already ended: dropped");
            return false;
        };
        if let Some(end) = report.end() {
            let client = session.client_id(kind, engine).unwrap_or_default();
            session.finish(kind, engine, end, Some(report.view(client)));
        }
        true
    }

    /// A new worker generation started after an older one stopped. The active requests of
    /// `kinds` whose command was queued at or before `through`, the last one the older
    /// generation claimed, died with it and end as `Lost`. Later ones the new generation runs.
    pub fn restarted(&mut self, kinds: &[IdKind], through: u64) {
        for s in &mut self.sessions {
            for &kind in kinds {
                let lost: Vec<u64> = s.requests[kind.index()]
                    .iter()
                    .filter(|r| r.queued != 0 && r.queued <= through)
                    .map(|r| r.engine)
                    .collect();
                for engine in lost {
                    s.finish(kind, engine, RequestEnd::Lost, None);
                }
            }
        }
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

    /// Removes session `id` with its mappings; its requests get no event. The local session
    /// cannot be closed. Returns true when a session was removed.
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
    /// gone, or whose request channel was full or closed, is closed.
    pub(crate) fn publish(&mut self, base: &Snapshot) {
        self.sessions.retain(|s| match &s.link {
            None => true,
            Some(_) if s.broken => {
                tracing::debug!(target: "tasklens::engine", id = s.id.0, "client session closed: request channel full or closed");
                false
            }
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
