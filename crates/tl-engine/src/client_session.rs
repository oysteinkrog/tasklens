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
//! their snapshots through their `SessionLink`. The engine service opens them through
//! `EngineHandle::open_session`, which sends `ClientMsg::OpenPort`: the session then also shares
//! a `PortState` with its `SessionPort`. Dropping the port closes the session through that
//! state, not through a queued message, so a full message queue never leaks a session
//! (`ClientSessions::sync_ports`).
//!
//! Each session also has its own `Demand`. The workers collect for the combined demand
//! (`ClientSessions::combined`): every session's demand plus the background demand of the host
//! settings (`background_demand`). Its lists are filled round robin across the sessions in
//! connect order, the local session first in each round, up to the same caps a single window
//! has (`DETAILS_MAX`, `GRAPH_TARGETS_MAX` and the others), so a later session is never starved
//! to 0 while the first holds a whole list. A session that closes leaves the combined demand at
//! the next publish. The `MetricHistory` worker answers each distinct `device_history_range`
//! once, and each session shows the answer for its own range in its own `device_history`. Each
//! session's `history` shows only the answers to its own `HistoryQuery`s.
//!
//! Ids a client picks (actions, string scans, handle searches, ejects, history queries) are its
//! own. The reducer gives each one a new engine-wide id from one counter before it reaches the
//! workers or the action gate, and maps it back on the way out, so two windows never see each
//! other's results. Actions keep no mapping here: the gate routes each one by engine id until
//! `ActionFinished`.
//!
//! Request lifecycle (scans, handle searches, ejects, history queries). Every request a session
//! sends ends with exactly one `RequestEvent` on the session's own request channel, and one
//! owner sends it:
//! - the reducer, as queue owner, for a request no worker took: `Busy` at admission (no mapping
//!   is made), `Lost` when the control watch is closed or its command leaves the worker queue
//!   before a worker claimed it (`CommandProgress::evict`, under the lock the worker's
//!   `CommandProgress::claim` takes, so a command is either evicted or claimed);
//! - the worker for a request it claimed: a terminal state in its update, `ScanDropped`,
//!   `EjectDropped`, or a newer request it reported in the same slot (the workers handle a slot's
//!   requests in engine id order and keep one per slot);
//! - the new worker generation, for the requests the stopped one claimed (`RequestsLost`,
//!   `EjectsLost`): `Lost`;
//! - for history queries, the reducer: `Lost` when the history sink dropped the query or a new
//!   history worker generation started after the one that took it (`HistoryUpdate::Started`),
//!   `Done` when the session's answer arrives;
//! - the engine supervisor, for every session's requests when the reducer stops
//!   (`Reducer::stopped`): `Lost`. The local session outlives the reducer; the others end with
//!   it.
//!
//! Actions end in the gate: its task sends the end, or the gate sends `Lost` for each action it
//! aborts when it is dropped (`ActionGate`).
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
//! An end is never dropped for a session that stays open. The local session's request channel
//! comes from `Engine::start`, is unbounded and is read through `EngineHandle::request_events`;
//! admission bounds what it holds (`ACTIVE_MAX` per kind, so 256 ends at most, actions
//! included). Its event is only lost when the UI dropped the receiver, which is counted in
//! `ClientSession::dropped`. A linked session's channel comes from the server and should be
//! unbounded too; when it is full or closed, the session ends. History queries have their own
//! limit, `HISTORY_ACTIVE_MAX` (ADR-018 "4 history windows").
//!
//! `crate::session` is the ADR-004 restart session, which is something else. The registry lives
//! in the reducer, so a reducer restart drops every session but the local one.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use tl_model::actions::ActionEvent;
use tl_model::command::Command;
use tl_model::demand::{
    AFFINITY_TARGETS_MAX, CpuDemand, DETAIL_KEYS_MAX, DETAILS_MAX, Demand, DeviceHistoryRequest,
    GRAPH_TARGETS_MAX, UNIT_DETAILS_MAX,
};
use tl_model::details::{
    DetailTab, DetailTarget, DetailsSnapshot, HandlePhase, HandleSearchState, ScanId, ScanPhase,
    SearchId, StringScanState, TabPayload, TabSlot,
};
use tl_model::devices::DeviceIdentity;
use tl_model::health::SourceId;
use tl_model::history::{HistoryTop, HistoryView};
use tl_model::ids::{DeviceIdx, ProcessKey};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ConnectionsSnapshot, DeviceDetailsSnapshot, DeviceHistory,
    DiskDetails, EjectState, ProcessSnapshot, ServicesSnapshot, Snapshot, StartupSnapshot,
};
use tl_model::table::{ProcessTableView, SysTable, SysTableQuery, TableQuery};
use tl_model::validity::Validity;

use crate::reducer::sys_table::{self, SysTableInput};
use crate::reducer::{Components, bump, sys_table_index};
use crate::source::CommandProgress;

/// Sessions at most, the local one included (ADR-018): 8 connections plus the local session.
pub const SESSIONS_MAX: usize = 9;

/// The six snapshot parts each session builds itself, in `ClientSession::published` order.
pub const SESSION_BITS: [ComponentMask; 6] = [
    ComponentMask::TABLE,
    ComponentMask::SERVICES,
    ComponentMask::STARTUP,
    ComponentMask::CONNECTIONS,
    ComponentMask::HISTORY,
    ComponentMask::DEVICE_HISTORY,
];

/// The session parts that a change of the shared component does not rebuild for every
/// session: each session checks whether its own answer changed.
const OWN_ANSWERS: ComponentMask =
    ComponentMask(ComponentMask::HISTORY.0 | ComponentMask::DEVICE_HISTORY.0);

/// Active requests per kind and session (scans, handle searches, ejects). One more is refused
/// with `RequestEnd::Busy` until one of them ends.
pub const ACTIVE_MAX: usize = 64;

/// History queries in flight per session (ADR-018 "4 history windows"). The fifth is refused
/// with `RequestEnd::Busy` until one is answered.
pub const HISTORY_ACTIVE_MAX: usize = 4;

pub use tl_model::request::{IdKind, RequestEnd, RequestEvent};

/// The kinds the reducer maps here. Actions keep no mapping: the gate routes them.
const MAPPED: [IdKind; 4] = [IdKind::Scan, IdKind::Search, IdKind::Eject, IdKind::History];

/// The place of `kind` in `ClientSession::requests`. The action list stays empty.
fn index(kind: IdKind) -> usize {
    match kind {
        IdKind::Scan => 0,
        IdKind::Search => 1,
        IdKind::Eject => 2,
        IdKind::Action => 3,
        IdKind::History => 4,
    }
}

/// Active requests of `kind` a session may have.
fn active_max(kind: IdKind) -> usize {
    match kind {
        IdKind::History => HISTORY_ACTIVE_MAX,
        _ => ACTIVE_MAX,
    }
}

/// The worker whose command queue the requests of `kind` go to. `None` for actions and for
/// history queries, which go through the history sink.
fn source(kind: IdKind) -> Option<SourceId> {
    match kind {
        IdKind::Scan | IdKind::Search => Some(SourceId::ProcessDetail),
        IdKind::Eject => Some(SourceId::Udisks),
        IdKind::Action | IdKind::History => None,
    }
}

/// The snapshot part that shows requests of `kind` in a slot. None for actions and history
/// queries, whose answers the session shows itself.
fn view_bit(kind: IdKind) -> ComponentMask {
    match kind {
        IdKind::Scan | IdKind::Search => ComponentMask::DETAILS,
        IdKind::Eject => ComponentMask::DEVICE_DETAILS,
        IdKind::Action | IdKind::History => ComponentMask::NONE,
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
    /// A history query. Queries have no slot of their own: each is answered.
    History,
}

impl IdSlot {
    pub fn kind(&self) -> IdKind {
        match self {
            IdSlot::Scan(_) => IdKind::Scan,
            IdSlot::Search => IdKind::Search,
            IdSlot::Eject(_) => IdKind::Eject,
            IdSlot::History => IdKind::History,
        }
    }
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
    #[error("the session's limit of active requests of this kind is reached")]
    Busy,
}

/// One active request: a mapping from the session's id to the engine's.
struct Request {
    client: u64,
    engine: u64,
    slot: IdSlot,
    /// The worker queue `seq` of its command, 0 until it is forwarded. For a history query,
    /// the history worker generation that took it.
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
    /// The end of each of the session's actions, scans, handle searches and ejects, with its
    /// own ids. Make it unbounded: admission bounds it. A full or closed channel ends the
    /// session.
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

/// What a `SessionPort` and the reducer share about one session (ADR-018). The reducer reads
/// it on every pass, so nothing here can be lost to a full message queue.
#[derive(Debug, Default)]
pub struct PortState {
    /// The port was dropped: the session closes at the reducer's next pass.
    closed: AtomicBool,
    /// The session left the registry, or was never opened. Its slot is free again.
    ended: AtomicBool,
    /// The newest demand the port set that the reducer has not taken yet. A newer one replaces
    /// it.
    demand: Mutex<Option<Demand>>,
    /// The reducer's answer to the open: `Ok` once the session is in the registry, the reason
    /// once it refused. `None` until then.
    admission: Mutex<Option<Result<(), OpenError>>>,
    admitted: Condvar,
    /// The app id a window on this host announced in its `Hello`. Its own-window KWin commands
    /// act only on its pid's windows with this app id (ADR-018, q4x76.12). `None` for a bridge
    /// session and a session with no `Hello`.
    own_app_id: Option<Arc<str>>,
    /// The session's `Hello` said it is a window on this host. Every other session counts as
    /// remote, so it fails closed: a bridge session and a session with no `Hello` need remote
    /// admin for root actions (ADR-018 Remote root actions).
    local: bool,
}

impl PortState {
    /// The state of a window on this host whose own-window commands act on its windows with
    /// `own_app_id`. The only state that is not remote.
    pub fn for_window(own_app_id: Arc<str>) -> Self {
        Self {
            own_app_id: Some(own_app_id),
            local: true,
            ..Self::default()
        }
    }

    /// The state of a session that comes through `tasklens bridge`: remote, with no own windows
    /// on this host. The same as `default`, which a session with no `Hello` gets.
    pub fn for_bridge() -> Self {
        Self::default()
    }

    pub fn own_app_id(&self) -> Option<&Arc<str>> {
        self.own_app_id.as_ref()
    }

    /// True for every session but a window on this host (`for_window`).
    pub fn is_remote(&self) -> bool {
        !self.local
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn end(&self) {
        self.ended.store(true, Ordering::Release);
    }

    pub fn is_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    pub fn set_demand(&self, demand: Demand) {
        *self.demand.lock().unwrap_or_else(PoisonError::into_inner) = Some(demand);
    }

    /// The reducer's answer to the open, if it gave one yet.
    pub fn admission(&self) -> Option<Result<(), OpenError>> {
        *self
            .admission
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for the reducer's answer to the open until `deadline`. Wakes every `slice` to ask
    /// `gone` whether the engine stopped, since a stopped engine drops the open unanswered.
    /// `None` when the deadline passed or `gone` said so first.
    pub fn wait_admission(
        &self,
        deadline: Instant,
        slice: Duration,
        gone: impl Fn() -> bool,
    ) -> Option<Result<(), OpenError>> {
        let mut answer = self
            .admission
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(result) = *answer {
                return Some(result);
            }
            let now = Instant::now();
            if now >= deadline || gone() {
                return None;
            }
            let wait = slice.min(deadline - now);
            answer = self
                .admitted
                .wait_timeout(answer, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Records the reducer's answer. A refusal also ends the port, so its slot frees.
    fn answer(&self, result: Result<(), OpenError>) {
        if result.is_err() {
            self.end();
        }
        *self
            .admission
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(result);
        self.admitted.notify_all();
    }

    fn take_demand(&self) -> Option<Demand> {
        self.demand
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// What the server sends the reducer about a session.
#[derive(Clone, Debug)]
pub enum ClientMsg {
    Open {
        id: ClientSessionId,
        link: SessionLink,
    },
    /// `Open` from `EngineHandle::open_session`: the session also shares `port` with its
    /// `SessionPort`. A port closed before this message arrives is not opened.
    OpenPort {
        id: ClientSessionId,
        link: SessionLink,
        port: Arc<PortState>,
    },
    Close(ClientSessionId),
    /// Only wakes the reducer, so it reads the ports' shared state now instead of at the next
    /// tick.
    Wake,
    /// The demand of session `id` (ADR-018). It joins the combined demand at the next publish.
    /// The local session's comes from the reducer's demand watch instead.
    Demand {
        id: ClientSessionId,
        demand: Box<Demand>,
    },
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
    /// The port was dropped before the reducer opened its session.
    #[error("the session's port is closed")]
    Closed,
    /// `EngineHandle::open_session` found the engine's message queue full, or the reducer did
    /// not answer within `bridge::OPEN_WAIT`.
    #[error("the engine is busy")]
    Busy,
    #[error("the engine has stopped")]
    Stopped,
}

/// One connected window's queries and the table parts built for them.
pub struct ClientSession {
    id: ClientSessionId,
    /// The window's demand (ADR-018), part of the combined demand. The local session's comes
    /// from the reducer's demand watch at each publish.
    pub demand: Demand,
    pub query: TableQuery,
    /// The last `SetSysTableQuery` per system table: Services, Startup, Connections.
    pub sys_queries: [Option<SysTableQuery>; 3],
    table: Arc<ProcessTableView>,
    /// The processes `table` was built from. A table built from other processes than the ones
    /// published with it names the wrong rows (`ClientSessions::stale_tables`).
    table_of: Weak<ProcessSnapshot>,
    /// The session's rows, query generation and counts. The other fields are refreshed from
    /// the shared component at publish.
    services: Arc<ServicesSnapshot>,
    startup: Arc<StartupSnapshot>,
    connections: Arc<ConnectionsSnapshot>,
    /// The shared details and device details with only this session's scans, handle search and
    /// ejects, under its own ids. The header is the shared one.
    details: Arc<DetailsSnapshot>,
    device_details: Arc<DeviceDetailsSnapshot>,
    /// The last answer to one of the session's history queries, under its own ids, with the
    /// shared header.
    history: Arc<HistoryView>,
    /// Engine id to the session's own id of the history queries its views name: the last
    /// Frame and TopAt queries, kept after they end so a view sent again after a delete maps.
    history_ids: Vec<(u64, u64)>,
    /// The worker's top list the session shows, and the session's copy of it.
    history_top: Option<(Arc<HistoryTop>, Arc<HistoryTop>)>,
    /// The answer for the session's range, with the shared header, and the shared answer it
    /// was made from. The session keeps it while no answer for its range has arrived.
    device_history: Arc<DeviceHistory>,
    device_history_src: Option<Arc<DeviceHistory>>,
    /// The active requests by `index` of their kind, oldest first.
    requests: [Vec<Request>; 5],
    /// Per slot, the newest request the session sent there.
    shown: Vec<Shown>,
    /// The shared disk list and the session's view of it, while no eject view changed.
    disks: Option<(DiskList, DiskList)>,
    /// `None` for the local session until `ClientSessions::set_local_requests`.
    requests_tx: Option<async_channel::Sender<RequestEvent>>,
    /// The request channel was full or closed: the session ends at the next publish.
    broken: bool,
    /// Events the local session's request channel did not take: it was closed or not set.
    dropped: u64,
    /// Parts this session rebuilt since its last publish, plus `DETAILS` and `DEVICE_DETAILS`
    /// when its views of those must be built.
    dirty: ComponentMask,
    /// Header version of each part at the last publish, in `SESSION_BITS` order.
    published: [u64; SESSION_BITS.len()],
    /// `None` for the local session, which publishes through the reducer's `Publisher`.
    link: Option<SessionLink>,
    /// The state shared with the session's `SessionPort`, for a session opened with
    /// `ClientMsg::OpenPort`. Dropping the session ends it.
    port: Option<Arc<PortState>>,
    /// The parts the session started with. `snapshot` sends these, the same `Arc` each time, in
    /// place of a part the session's demand does not ask for.
    empty: Snapshot,
}

impl Drop for ClientSession {
    /// However the session leaves (closed, broken, gone, or with its reducer), its port learns it
    /// ended and its slot frees.
    fn drop(&mut self) {
        if let Some(port) = &self.port {
            port.end();
        }
    }
}

impl ClientSession {
    /// Default queries and the parts of `empty`.
    fn new(id: ClientSessionId, link: Option<SessionLink>, empty: &Snapshot) -> Self {
        ClientSession {
            id,
            demand: Demand::default(),
            query: TableQuery::default(),
            sys_queries: [None, None, None],
            table: Arc::clone(&empty.table),
            table_of: Weak::new(),
            services: Arc::clone(&empty.services),
            startup: Arc::clone(&empty.startup),
            connections: Arc::clone(&empty.connections),
            details: Arc::clone(&empty.details),
            device_details: Arc::clone(&empty.device_details),
            history: Arc::clone(&empty.history),
            history_ids: Vec::new(),
            history_top: None,
            device_history: Arc::clone(&empty.device_history),
            device_history_src: None,
            requests: Default::default(),
            shown: Vec::new(),
            disks: None,
            requests_tx: link.as_ref().map(|l| l.requests_tx.clone()),
            broken: false,
            dropped: 0,
            dirty: ComponentMask::DETAILS | ComponentMask::DEVICE_DETAILS,
            published: [0; SESSION_BITS.len()],
            link,
            port: None,
            empty: empty.clone(),
        }
    }

    pub fn id(&self) -> ClientSessionId {
        self.id
    }

    /// The app id this session announced as a window on this host (`PortState::own_app_id`).
    pub fn own_app_id(&self) -> Option<&Arc<str>> {
        self.port.as_ref().and_then(|p| p.own_app_id())
    }

    /// True unless the session is known to be on this host: the in-process local session, or a
    /// port whose `Hello` named a window here (`PortState::is_remote`). A bridge session, a
    /// session with no `Hello` and any other session without a port are remote (fails closed).
    pub fn is_remote(&self) -> bool {
        match &self.port {
            Some(port) => port.is_remote(),
            None => self.id != ClientSessionId::LOCAL,
        }
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

    /// Replaces the process table, built from `processes`. Its header version should be 0;
    /// publish sets it.
    pub(crate) fn set_table(&mut self, view: ProcessTableView, processes: &Arc<ProcessSnapshot>) {
        self.table = Arc::new(view);
        self.table_of = Arc::downgrade(processes);
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

    pub fn history(&self) -> &Arc<HistoryView> {
        &self.history
    }

    pub fn device_history(&self) -> &Arc<DeviceHistory> {
        &self.device_history
    }

    /// Takes the shared answer for the session's range when it is new, and the shared
    /// validity when it changed. Without an answer for its range the session keeps what it
    /// shows, as one window did while its new range was read.
    fn select_device_history(&mut self, c: &Components) {
        let key = self.demand.device_history_range;
        let found = c
            .device_histories
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, h)| h);
        let validity = c.device_history.header.validity;
        let same_src = match (found, &self.device_history_src) {
            (None, _) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (Some(_), None) => false,
        };
        if same_src && self.device_history.header.validity == validity {
            return;
        }
        let src = found.unwrap_or(&self.device_history);
        self.device_history = Arc::new(DeviceHistory {
            header: ComponentHeader {
                version: 0,
                validity,
                ..src.header
            },
            ..DeviceHistory::clone(src)
        });
        if let Some(found) = found {
            self.device_history_src = Some(Arc::clone(found));
        }
        self.dirty |= ComponentMask::DEVICE_HISTORY;
    }

    /// Takes the shared validity of `history` when it changed.
    fn select_history_validity(&mut self, c: &Components) {
        let validity = c.history.header.validity;
        if self.history.header.validity == validity {
            return;
        }
        self.history = Arc::new(HistoryView {
            header: ComponentHeader {
                version: 0,
                validity,
                ..self.history.header
            },
            ..HistoryView::clone(&self.history)
        });
        self.dirty |= ComponentMask::HISTORY;
    }

    /// The session's own id of history query `engine`, from the queries its views name. 0, the
    /// id of no query, when the session does not know it: it ended before its answer came.
    fn history_client(&self, engine: u64) -> u64 {
        self.history_ids
            .iter()
            .find(|(e, _)| *e == engine)
            .map_or(0, |(_, c)| *c)
    }

    /// Shows `view` under the session's own query ids, with the shared validity. A top list
    /// the session showed already keeps its `Arc` (ADR-003).
    fn show_history(&mut self, view: &HistoryView, validity: Validity) {
        let top = view.top.as_ref().map(|src| match &self.history_top {
            Some((old, out)) if Arc::ptr_eq(old, src) => Arc::clone(out),
            _ => Arc::new(HistoryTop {
                query_id: self.history_client(src.query_id),
                ..HistoryTop::clone(src)
            }),
        });
        self.history_top = view.top.clone().zip(top.clone());
        let query_id = self.history_client(view.query_id);
        let named = [view.query_id, view.top.as_ref().map_or(0, |t| t.query_id)];
        self.history_ids.retain(|(e, _)| named.contains(e));
        self.history = Arc::new(HistoryView {
            header: ComponentHeader {
                version: 0,
                validity,
                ..view.header
            },
            query_id,
            top,
            ..view.clone()
        });
        self.dirty |= ComponentMask::HISTORY;
    }

    /// The session's action event channel. `None` for the local session, whose events go to
    /// the reducer's own channel.
    pub fn events(&self) -> Option<&async_channel::Sender<ActionEvent>> {
        self.link.as_ref().map(|l| &l.events_tx)
    }

    /// The engine id of the session's newest active request with `client` id of `kind`.
    pub fn engine_id(&self, kind: IdKind, client: u64) -> Option<u64> {
        self.requests[index(kind)]
            .iter()
            .rev()
            .find(|r| r.client == client)
            .map(|r| r.engine)
    }

    /// The session's own id for active request `engine` of `kind`.
    pub fn client_id(&self, kind: IdKind, engine: u64) -> Option<u64> {
        self.requests[index(kind)]
            .iter()
            .find(|r| r.engine == engine)
            .map(|r| r.client)
    }

    /// The session's active requests of `kind`, each with its mapping.
    pub fn active(&self, kind: IdKind) -> usize {
        self.requests[index(kind)].len()
    }

    /// The slots of `kind` the session shows a request in, active or ended.
    pub fn shown(&self, kind: IdKind) -> usize {
        self.shown.iter().filter(|s| s.slot.kind() == kind).count()
    }

    fn is_active(&self, kind: IdKind, engine: u64) -> bool {
        self.requests[index(kind)]
            .iter()
            .any(|r| r.engine == engine)
    }

    /// Events the local session's request channel did not take because the UI dropped its
    /// receiver or no channel was set. Always 0 for a linked session, which ends instead.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The channel the session's request ends go to. `None` for the local session until
    /// `ClientSessions::set_local_requests`.
    pub fn requests_tx(&self) -> Option<&async_channel::Sender<RequestEvent>> {
        self.requests_tx.as_ref()
    }

    /// Sends `event` on the request channel. The local channel is unbounded, so it only refuses
    /// when the UI is gone; the event is then dropped, logged and counted. A full or closed
    /// linked channel ends the session at the next publish, as a full action event queue does
    /// (ADR-018).
    fn send(&mut self, event: RequestEvent) {
        if self.broken {
            return;
        }
        let Some(tx) = self.requests_tx.as_ref() else {
            self.dropped += 1;
            tracing::warn!(target: "tasklens::engine", ?event, "local session has no request channel: event dropped");
            return;
        };
        if let Err(e) = tx.try_send(event) {
            if self.link.is_some() {
                tracing::debug!(target: "tasklens::engine", id = self.id.0, ?event, full = e.is_full(), "request channel full or closed: session ends");
                self.broken = true;
            } else {
                self.dropped += 1;
                tracing::warn!(target: "tasklens::engine", ?event, full = e.is_full(), "local request channel refused the event: event dropped");
            }
        }
    }

    /// Ends active request `engine` of `kind`: removes its mapping and sends `end`. The slot
    /// then shows `last` when the request is still the newest there, else it keeps what it
    /// shows. False when the session does not have the request.
    fn finish(&mut self, kind: IdKind, engine: u64, end: RequestEnd, last: Option<View>) -> bool {
        let list = &mut self.requests[index(kind)];
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
        self.dirty |= view_bit(kind);
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
            if !shared.contains(view_bit(kind)) || !self.is_active(kind, shown.engine) {
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
                IdSlot::History => None,
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
                IdSlot::History => true,
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

    /// Brings the six parts up to date for a publish. A part is rebuilt when this session
    /// changed it or when `shared` (the reducer's dirty mask) has its bit; the system tables
    /// then take every field but their rows, generation and counts from `c`. The history parts
    /// are the session's own answers: they change only when the session's answer or the shared
    /// header changed. A rebuilt part gets version one above its last published one. The views
    /// of details and device details are rebuilt from `c` when their bit is set; they keep the
    /// shared header.
    pub(crate) fn prepare(&mut self, shared: ComponentMask, c: &Components) {
        self.refresh(shared, c);
        if shared.contains(ComponentMask::HISTORY) {
            self.select_history_validity(c);
        }
        // A session whose range changed picks its answer even when no new one arrived.
        self.select_device_history(c);
        let shared = ComponentMask(shared.0 & !OWN_ANSWERS.0);
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
                ComponentMask::HISTORY => bump(&mut self.history, p),
                ComponentMask::DEVICE_HISTORY => bump(&mut self.device_history, p),
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

    /// `base` with this session's six parts and its views of details and device details, each
    /// part only while the session's demand asks for it (ADR-018 Sessions, leader decisions on
    /// tasklens-q4x76.15). In place of a part it does not ask for the session gets its empty
    /// one, the same `Arc` on every publish, so its frames do not carry that part. When the
    /// demand comes back, the part is the session's current one again and its next frame
    /// carries it whole.
    ///
    /// Each part is gated by the narrowest `Demand` field that names it, plus the field of each
    /// page that reads it without asking for it: the Services page reads `table`, the
    /// Processes page reads `services` (unit tooltip, row menu). Window visibility
    /// (`window_visible`) gates only the parts no field names, and `inventory`, which many
    /// pages and windows read with no field for it (Start time, card titles, About) and which
    /// changes only on a rescan. "Tray graphs" is any of the tray's cpu, memory, disks, nets
    /// and gpus flags; "own" is a scan, handle search or eject of the session's own that it
    /// still shows.
    ///
    /// | Part | Gate |
    /// |---|---|
    /// | `sequence`, `epoch`, `boot_id`, `published_ns` | always (frame header) |
    /// | `system` | always: it holds the title bar value, all a host that is not shown asks for |
    /// | `series`, `health` | a window or tray graphs |
    /// | `devices` | a window, tray graphs or own (`device_details` names its disks by index) |
    /// | `processes` | a window, `graph_targets` or the `table` gate (table rows index into it) |
    /// | `process_series` | `graph_targets` |
    /// | `table` | `process_table`, `services` (the Services page's cells read it) or `details` |
    /// | `inventory` | `inventory` or a window |
    /// | `windows` | a window |
    /// | `services` | `services`, `service_cgroup_stats`, `unit_details` or `process_table` |
    /// | `startup` | `startup` |
    /// | `connections` | `connections` or `connections_all_netns` |
    /// | `details` | `details`, `affinity_targets` or own |
    /// | `device_details` | a window or own |
    /// | `device_history` | a window or `device_history_range` |
    /// | `history` | always: only the answers to the session's own history queries |
    ///
    /// The reducer rebuilds a table built from older processes before the publish
    /// (`ClientSessions::stale_tables`), so the first snapshot that carries the table again
    /// holds the current one.
    pub fn snapshot(&self, base: Snapshot) -> Snapshot {
        let d = &self.demand;
        let e = &self.empty;
        let window = window_visible(d);
        let tray = d.tray.cpu || d.tray.memory || d.tray.disks || d.tray.nets || d.tray.gpus;
        let targets = !d.graph_targets.is_empty();
        let own = !self.shown.is_empty();
        let table = table_demanded(d);
        let services =
            d.services || d.service_cgroup_stats || !d.unit_details.is_empty() || d.process_table;
        let details = !d.details.is_empty() || !d.affinity_targets.is_empty() || own;
        Snapshot {
            series: gate(window || tray, base.series, &e.series),
            devices: gate(window || tray || own, base.devices, &e.devices),
            health: gate(window || tray, base.health, &e.health),
            processes: gate(window || targets || table, base.processes, &e.processes),
            process_series: gate(targets, base.process_series, &e.process_series),
            table: gate(table, Arc::clone(&self.table), &e.table),
            inventory: gate(d.inventory || window, base.inventory, &e.inventory),
            windows: gate(window, base.windows, &e.windows),
            services: gate(services, Arc::clone(&self.services), &e.services),
            startup: gate(d.startup, Arc::clone(&self.startup), &e.startup),
            connections: gate(
                d.connections || d.connections_all_netns,
                Arc::clone(&self.connections),
                &e.connections,
            ),
            details: gate(details, Arc::clone(&self.details), &e.details),
            device_details: gate(
                window || own,
                Arc::clone(&self.device_details),
                &e.device_details,
            ),
            device_history: gate(
                window || d.device_history_range.is_some(),
                Arc::clone(&self.device_history),
                &e.device_history,
            ),
            history: Arc::clone(&self.history),
            ..base
        }
    }
}

/// The session's demand asks for its process table: the Processes tab, the Services page,
/// whose cells read the table, or an open Properties window.
fn table_demanded(d: &Demand) -> bool {
    d.process_table || d.services || !d.details.is_empty()
}

/// `part` when `on`, else `empty`.
fn gate<T>(on: bool, part: Arc<T>, empty: &Arc<T>) -> Arc<T> {
    if on { part } else { Arc::clone(empty) }
}

/// A window surface is visible. The UI's merge counts shown tray icons as a visible surface
/// too (tl-ui `DemandAggregator`), so a demand whose only visible surface is the tray, one that
/// asks for nothing but the tray, graph targets, recording and device history, is not a window.
fn window_visible(d: &Demand) -> bool {
    let tray_only = Demand {
        any_surface_visible: true,
        tray: d.tray,
        graph_targets: Arc::clone(&d.graph_targets),
        recording: d.recording,
        device_history: d.device_history,
        ..Demand::default()
    };
    d.any_surface_visible && !(d.tray.icons_shown && *d == tray_only)
}

fn unversioned(header: ComponentHeader) -> ComponentHeader {
    ComponentHeader {
        version: 0,
        ..header
    }
}

/// What the engine collects with no window connected (ADR-018 "Background demand"): the device
/// series at 1 s, the memory group sweep every 10 s and the event detector's journal sources,
/// all of which `Demand.device_history` turns on. With `history.device_metrics` off it is
/// empty, so nothing is collected for history and the memory group sweep runs only on demand.
pub fn background_demand(settings: &EngineSettings) -> Demand {
    Demand {
        device_history: settings.history.device_metrics,
        ..Demand::default()
    }
}

fn cpu_rank(cpu: CpuDemand) -> u8 {
    match cpu {
        CpuDemand::Off => 0,
        CpuDemand::Total => 1,
        CpuDemand::PerCore => 2,
    }
}

/// Field-wise union of `parts`, which come in fill order: bools OR, `cpu` maximum, column sets
/// union, lists filled round robin up to their caps (`fill`). `device_history_range` is the
/// first part's: the `MetricHistory` worker answers it from the demand watch, and the other
/// sessions' ranges through `ClientSessions::device_history_ranges`.
pub fn combine(parts: &[&Demand]) -> Demand {
    let mut m = Demand::default();
    for p in parts {
        m.any_surface_visible |= p.any_surface_visible;
        if cpu_rank(p.cpu) > cpu_rank(m.cpu) {
            m.cpu = p.cpu;
        }
        m.cpu_fast |= p.cpu_fast;
        m.memory |= p.memory;
        m.disks |= p.disks;
        m.nets |= p.nets;
        m.gpus |= p.gpus;
        m.sensors |= p.sensors;
        m.process_table |= p.process_table;
        m.process_top |= p.process_top;
        for (word, part) in m.process_columns.0.iter_mut().zip(p.process_columns.0) {
            *word |= part;
        }
        m.gpu_process |= p.gpu_process;
        m.connections |= p.connections;
        m.services |= p.services;
        m.startup |= p.startup;
        m.inventory |= p.inventory;
        m.service_cgroup_stats |= p.service_cgroup_stats;
        m.connections_all_netns |= p.connections_all_netns;
        m.tray.icons_shown |= p.tray.icons_shown;
        m.tray.cpu |= p.tray.cpu;
        m.tray.memory |= p.tray.memory;
        m.tray.disks |= p.tray.disks;
        m.tray.nets |= p.tray.nets;
        m.tray.gpus |= p.tray.gpus;
        m.recording |= p.recording;
        m.device_history |= p.device_history;
        m.device_timeline |= p.device_timeline;
    }
    m.device_history_range = parts.first().and_then(|p| p.device_history_range);
    m.detail_keys = fill(parts.iter().map(|p| &p.detail_keys), DETAIL_KEYS_MAX);
    m.details = fill(parts.iter().map(|p| &p.details), DETAILS_MAX);
    m.affinity_targets = fill(
        parts.iter().map(|p| &p.affinity_targets),
        AFFINITY_TARGETS_MAX,
    );
    m.unit_details = fill(parts.iter().map(|p| &p.unit_details), UNIT_DETAILS_MAX);
    m.remote_host_cookies = fill(parts.iter().map(|p| &p.remote_host_cookies), usize::MAX);
    m.graph_targets = fill(parts.iter().map(|p| &p.graph_targets), GRAPH_TARGETS_MAX);
    m
}

/// Fills one list of at most `max` entries round robin from `lists`, in their order. In each
/// round every list adds its next entry that is not in the result yet, so with 3 lists of 8 and
/// `max` 16 they get 6, 5 and 5. Entries past a list's share are not collected, as entries past
/// the cap of a single window are not (`Demand::clamp_lists`). A single non-empty list keeps its
/// `Arc` when it fits.
fn fill<'a, T: Clone + PartialEq + 'a>(
    lists: impl Iterator<Item = &'a Arc<[T]>>,
    max: usize,
) -> Arc<[T]> {
    let lists: Vec<&Arc<[T]>> = lists.filter(|l| !l.is_empty()).collect();
    match lists.as_slice() {
        [] => return Arc::from([]),
        [one] if one.len() <= max => return Arc::clone(one),
        _ => {}
    }
    let mut out: Vec<T> = Vec::new();
    let mut next = vec![0; lists.len()];
    let mut added = true;
    while added && out.len() < max {
        added = false;
        for (list, at) in lists.iter().zip(next.iter_mut()) {
            if out.len() == max {
                break;
            }
            while let Some(item) = list.get(*at) {
                *at += 1;
                if !out.contains(item) {
                    out.push(item.clone());
                    added = true;
                    break;
                }
            }
        }
    }
    out.into()
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

    /// Gives the local session its request channel. `Engine::start` makes it, and
    /// `EngineHandle::request_events` hands out the receiver.
    pub fn set_local_requests(&mut self, tx: async_channel::Sender<RequestEvent>) {
        self.sessions[0].requests_tx = Some(tx);
    }

    /// A new engine-wide id. Ids only grow, which the eject worker relies on: it ignores a
    /// `seq` at or below the last one of a disk.
    pub fn next_engine_id(&mut self) -> u64 {
        self.last_engine_id += 1;
        self.last_engine_id
    }

    /// The last engine id given out. `engine_session` hands it to the next reducer.
    pub fn last_engine_id(&self) -> u64 {
        self.last_engine_id
    }

    /// Gives out engine ids after `last`, the stopped reducer's `last_engine_id`. Ids then grow
    /// across reducers too, so a command the stopped one left in the worker queue never names a
    /// newer request.
    pub fn resume_engine_ids(&mut self, last: u64) {
        self.last_engine_id = self.last_engine_id.max(last);
    }

    /// Reports and ends that arrived after their request had ended or its session had closed.
    /// For debugging only.
    pub fn late(&self) -> u64 {
        self.late
    }

    /// Admits session `id`'s request `client` in `slot`: maps it to a new engine id and returns
    /// that. A client id sent again gets a new engine id; cancels use the newest active one.
    /// When the session has `ACTIVE_MAX` active requests of the slot's kind
    /// (`HISTORY_ACTIVE_MAX` history queries), it gets `RequestEnd::Busy` and no mapping is
    /// made.
    pub fn admit(
        &mut self,
        id: ClientSessionId,
        slot: IdSlot,
        client: u64,
    ) -> Result<u64, MapError> {
        let at = self
            .sessions
            .iter()
            .position(|s| s.id == id)
            .ok_or(MapError::NoSession)?;
        let kind = slot.kind();
        if self.sessions[at].active(kind) >= active_max(kind) {
            self.sessions[at].send(RequestEvent {
                kind,
                id: client,
                end: RequestEnd::Busy,
            });
            return Err(MapError::Busy);
        }
        let engine = self.next_engine_id();
        let session = &mut self.sessions[at];
        session.requests[index(kind)].push(Request {
            client,
            engine,
            slot: slot.clone(),
            queued: 0,
        });
        if kind == IdKind::History {
            // Each query is answered: there is no slot to show.
            return Ok(engine);
        }
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
            s.requests[index(kind)]
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
            .flat_map(|s| s.requests[index(kind)].iter())
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

    /// The history worker sent client session `owner` a view. A view that `answers` a query
    /// ends it as `Done`; the session then shows the view under its own ids. A view for a
    /// session that is gone, or that answers a query no longer active, is counted as late; the
    /// latter is still shown.
    pub fn history_view(
        &mut self,
        owner: ClientSessionId,
        answers: Option<u64>,
        view: &HistoryView,
        validity: Validity,
    ) {
        let Some(session) = self.sessions.iter_mut().find(|s| s.id == owner) else {
            self.late += 1;
            tracing::debug!(target: "tasklens::engine", owner = owner.0, "history view for a closed session: dropped");
            return;
        };
        if let Some(engine) = answers {
            match session.client_id(IdKind::History, engine) {
                Some(client) => {
                    session.history_ids.push((engine, client));
                    session.finish(IdKind::History, engine, RequestEnd::Done, None);
                }
                None => {
                    self.late += 1;
                    tracing::debug!(target: "tasklens::engine", engine, "answer to a history query that already ended");
                }
            }
        }
        session.show_history(view, validity);
    }

    /// A new worker generation started after an older one stopped. The active requests of
    /// `kinds` whose command was queued at or before `through`, the last one the older
    /// generation claimed, died with it and end as `Lost`. Later ones the new generation runs.
    pub fn restarted(&mut self, kinds: &[IdKind], through: u64) {
        for s in &mut self.sessions {
            for &kind in kinds {
                let lost: Vec<u64> = s.requests[index(kind)]
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

    /// The reducer stops: it restarts or the engine shuts down. Each active scan, handle search,
    /// eject and history query of every session ends as `Lost`; the local session outlives the reducer, the
    /// others end with it right after. A command still in the worker queue is evicted first,
    /// under the lock a worker's claim takes, so the next reducer's workers never run a request
    /// that already ended; the unclaimed commands of that worker queued before it are skipped
    /// with it. A command a worker claimed died with that worker. The actions end in the gate
    /// (`Reducer::stopped`).
    pub fn reducer_stopped(&mut self, progress: &CommandProgress) {
        for session in &mut self.sessions {
            for kind in MAPPED {
                let ended: Vec<(u64, u64)> = session.requests[index(kind)]
                    .iter()
                    .map(|r| (r.engine, r.queued))
                    .collect();
                for (engine, queued) in ended {
                    if let Some(source) = source(kind).filter(|_| queued != 0) {
                        progress.evict(source, queued);
                    }
                    session.finish(kind, engine, RequestEnd::Lost, None);
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

    /// `open` for a session that shares `port` with its `SessionPort`. A port that is already
    /// closed is not opened. The port gets the answer, so `EngineHandle::open_session` returns
    /// it; on any refusal the port is also ended, so its slot frees. The port's newest demand is
    /// the session's first.
    pub fn open_port(
        &mut self,
        id: ClientSessionId,
        link: SessionLink,
        port: Arc<PortState>,
        empty: &Snapshot,
    ) -> Result<&mut ClientSession, OpenError> {
        if port.is_closed() {
            port.answer(Err(OpenError::Closed));
            return Err(OpenError::Closed);
        }
        match self.open(id, link, empty) {
            Ok(session) => {
                if let Some(demand) = port.take_demand() {
                    session.demand = demand;
                }
                port.answer(Ok(()));
                session.port = Some(port);
                Ok(session)
            }
            Err(e) => {
                port.answer(Err(e));
                Err(e)
            }
        }
    }

    /// Reads every port's shared state (ADR-018): closes the sessions whose port was dropped and
    /// takes each open port's newest demand. The reducer calls it on every pass. Returns true
    /// when a session was closed.
    pub fn sync_ports(&mut self) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|s| match &s.port {
            Some(port) if port.is_closed() => {
                tracing::debug!(target: "tasklens::engine", id = s.id.0, "client session closed: port dropped");
                false
            }
            _ => true,
        });
        for session in &mut self.sessions {
            if let Some(demand) = session.port.as_ref().and_then(|p| p.take_demand()) {
                session.demand = demand;
            }
        }
        self.sessions.len() != before
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

    /// The local window's demand, from the reducer's demand watch.
    pub fn set_local_demand(&mut self, demand: Demand) {
        self.sessions[0].demand = demand;
    }

    /// Each session's device history range, in connect order with the local session first.
    /// `None` stands for a session that shows no range: it needs the empty answer, so a session
    /// that clears its range stops showing its old points. The `MetricHistory` worker answers
    /// the distinct ranges once each (ADR-018).
    pub fn device_history_ranges(&self) -> Vec<(ClientSessionId, Option<DeviceHistoryRequest>)> {
        self.sessions
            .iter()
            .map(|s| (s.id, s.demand.device_history_range))
            .collect()
    }

    /// The demand the workers collect for (ADR-018): `local`, the local window's current demand,
    /// then every other session's in connect order, then `background`. Lists are filled round
    /// robin in that order (`combine`).
    pub fn combined(&self, local: &Demand, background: &Demand) -> Demand {
        let mut parts = Vec::with_capacity(self.sessions.len() + 1);
        parts.push(local);
        parts.extend(self.sessions[1..].iter().map(|s| &s.demand));
        parts.push(background);
        combine(&parts)
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Never true: the local session is always open.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// In connect order, the local session first.
    pub fn iter(&self) -> impl Iterator<Item = &ClientSession> + Clone {
        self.sessions.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ClientSession> {
        self.sessions.iter_mut()
    }

    /// The linked sessions whose snapshot carries their table (`table_demanded`) but whose
    /// table was not built from `processes`, the processes published with it. A sweep while no
    /// session asks for the table updates only the processes (`processes::history_only`), so a
    /// session shown again would get its old table, whose rows name records by index, over the
    /// new processes. The reducer rebuilds these tables before it publishes.
    pub(crate) fn stale_tables(&self, processes: &Arc<ProcessSnapshot>) -> Vec<ClientSessionId> {
        let current = Arc::downgrade(processes);
        self.sessions
            .iter()
            .filter(|s| {
                s.link.is_some()
                    && table_demanded(&s.demand)
                    && !Weak::ptr_eq(&s.table_of, &current)
            })
            .map(|s| s.id)
            .collect()
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
