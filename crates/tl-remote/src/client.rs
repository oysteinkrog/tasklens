//! `RemoteClient`: the window's `EngineClient` over the engine protocol (ADR-018 Queues and
//! back-pressure, Identity and reconnects, Journal, Implementation contracts "Reconnect").
//!
//! One connection thread per client connects through a `Connector`, sends `Hello`, reads
//! `Welcome` and then reads the session's messages:
//! - a snapshot frame is applied to the installed snapshot (`delta::apply`), stored with its
//!   session number, and then the wake token is sent with `try_send`. `Ack { frame }` goes out
//!   after that, so the server never builds on a frame the client has not installed;
//! - action events go to `action_events`, request ends to `request_events`, journal batches to
//!   their stream's handle. Only progress (`Started`) is dropped when the UI falls behind.
//!
//! A writer thread per connection sends what the client queues, in this order: the control path
//! (`Ack`, `JournalClose`), which has no limit, so it works when the command queue is full; the
//! newest demand; then the command queue of `COMMAND_QUEUE` messages. `send` never blocks: a full
//! queue is `Err(Busy)`, no connection is `Err(EngineStopped)`. A `JournalClose` that overtakes
//! its stream's `JournalOpen` is held back by the writer, which then sends neither.
//!
//! Every action, scan, handle search, eject and history query sent is tracked by `IdKind` and id
//! until its `RequestEvent` arrives. When the connection drops, each one still open ends here
//! instead: an action gets an `OutcomeUnknown("connection to <host> lost")` summary and then
//! `Lost`, every other kind gets `Lost`. So each request ends exactly once. Commands are never
//! replayed. The newest demand and table queries are session state, not commands: they are sent
//! again on each new connection.
//!
//! After a drop the client connects again after 1, 2, 4, 8, 16, then every 30 s. Each new
//! session starts from a complete frame. Its component versions are the new session's, so each
//! snapshot is stored together with its session number as `generation`, and the hub compares
//! nothing across two of them. Until that first snapshot `latest` keeps the last data.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use arc_swap::ArcSwap;
use tl_model::actions::{ActionEvent, ActionId, ActionState};
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, DeviceCommand};
use tl_model::demand::Demand;
use tl_model::journal::{JournalControl, JournalEvent, JournalHandle, JournalQuery};
use tl_model::remote_proto::{
    BootPrefix, ClientMsg, Hello, HelloTail, HostIdentity, PROTO, ServerBootstrap, ServerMsg,
    StreamId, Welcome, decode_server_bootstrap, encode_hello,
};
use tl_model::request::{IdKind, RequestEnd, RequestEvent};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::table::{SysTableQuery, TableQuery};
use tl_model::time::BootId;

use crate::delta;
use crate::envelope::{FrameReader, FrameWriter};

/// Commands queued for the writer at most. One more is `Err(Busy)`.
pub const COMMAND_QUEUE: usize = 64;
/// Action events waiting for the UI, as in the in-process engine. Beyond this a progress event
/// (`Started`) is dropped. A final state is never dropped: an action's summary must reach the UI.
pub const ACTION_EVENTS: usize = 256;
/// Journal events waiting for the UI per stream, the server's per-stream batch queue.
pub const JOURNAL_EVENTS: usize = 64;
/// Journal control messages waiting per stream. A full channel drops the message, as
/// `JournalHandle` documents.
const JOURNAL_CONTROLS: usize = 8;
/// Waits before each reconnect, in order. The last one repeats.
pub const BACKOFF: [Duration; 6] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(30),
];

/// One step of the client, reported by `RemoteClient::observe` (`test-hooks`). A test waits for
/// the exact step it then asserts on.
///
/// Each step is reported before the store, queue push, channel send or write that lets another
/// thread act on it. So a step another thread takes because of it is always reported later.
/// `SessionEnded` is the one step reported after its effects, because a test waits for it and
/// then checks them.
#[cfg(feature = "test-hooks")]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Observed {
    /// A frame was applied and is about to be stored; the wake token and the `Ack` follow.
    SnapshotInstalled { generation: u64, frame: u64 },
    /// The first snapshot of session `generation` is about to be stored. `generation()` returns
    /// it once `AckQueued` for that frame is reported and the state lock is free.
    Generation(u64),
    /// `Ack { frame }` is about to be pushed on the control path. The reader holds the client's
    /// state lock from this report until the Ack is pushed and the writer is rung, so any client
    /// call that takes the lock and returns after this report finds the Ack queued.
    AckQueued(u64),
    /// `Ack { frame }` is about to be written to the stream.
    AckWritten(u64),
    /// A request end is about to be passed to `request_events`.
    RequestEnded(RequestEvent),
    /// The session lost its link, and the ends of its open requests were passed on.
    SessionEnded(u64),
}

/// The queue behind `RemoteClient::observe` (`test-hooks`).
#[cfg(feature = "test-hooks")]
#[derive(Clone)]
struct Observer {
    tx: async_channel::Sender<Observed>,
    rx: async_channel::Receiver<Observed>,
}

/// Reports a step to the client's `Observer`. Compiles to nothing without `test-hooks`.
macro_rules! observe {
    ($observer:expr, $step:expr) => {
        #[cfg(feature = "test-hooks")]
        {
            let _ = $observer.tx.try_send($step);
        }
    };
}

/// One open connection to an engine: the two halves of one byte stream.
pub struct Connection {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// Ends the stream so a read or write blocked on it returns, for example
    /// `UnixStream::shutdown`. Called when the client drops and when the session ends.
    pub shutdown: Arc<dyn Fn() + Send + Sync>,
}

impl Connection {
    /// A connection over one Unix stream socket.
    pub fn unix(stream: UnixStream) -> io::Result<Self> {
        let writer = stream.try_clone()?;
        let control = stream.try_clone()?;
        Ok(Self {
            reader: Box::new(stream),
            writer: Box::new(writer),
            shutdown: Arc::new(move || {
                let _ = control.shutdown(std::net::Shutdown::Both);
            }),
        })
    }
}

/// Opens connections to one engine. Called from the client's connection thread, once at start
/// and once per reconnect.
pub trait Connector: Send + Sync + 'static {
    fn connect(&self) -> io::Result<Connection>;
}

impl<F> Connector for F
where
    F: Fn() -> io::Result<Connection> + Send + Sync + 'static,
{
    fn connect(&self) -> io::Result<Connection> {
        self()
    }
}

/// What a `RemoteClient` sends and how it waits.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// The `Hello` tail: pid, app id, the bridge flag and what the client wants.
    pub hello: HelloTail,
    /// Waits before each reconnect; the last repeats. `BACKOFF` in the app. Must not be empty.
    pub backoff: Vec<Duration>,
}

/// Where the client's connection is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LinkStatus {
    /// Connecting, or connected and waiting for the session's first snapshot. Pages keep their
    /// last data and actions are disabled (ADR-018 Reconnect).
    Connecting,
    /// The session's first snapshot is installed.
    Connected,
    /// The last attempt failed for this reason. The next one follows the backoff.
    Failed(Arc<str>),
}

/// An `EngineClient` that talks to an engine over a `Connector` (ADR-018).
pub struct RemoteClient {
    shared: Arc<Shared>,
    /// Dropped with the client, which ends the connection thread's backoff wait at once.
    _stop: mpsc::Sender<()>,
}

struct Shared {
    latest: ArcSwap<Installed>,
    stopped: AtomicBool,
    wake_tx: async_channel::Sender<()>,
    wake_rx: async_channel::Receiver<()>,
    events_tx: async_channel::Sender<ActionEvent>,
    events_rx: async_channel::Receiver<ActionEvent>,
    requests_tx: async_channel::Sender<RequestEvent>,
    requests_rx: async_channel::Receiver<RequestEvent>,
    state: Mutex<State>,
    #[cfg(feature = "test-hooks")]
    observer: Observer,
}

/// The stored snapshot with the session it came from, swapped as one value so a reader never
/// pairs a snapshot with another session's generation.
struct Installed {
    /// The session number; 0 before the first snapshot.
    generation: u64,
    snap: Arc<Snapshot>,
}

#[derive(Default)]
struct State {
    /// The writer's queues while a session is open.
    link: Option<Link>,
    /// Sessions opened so far. Numbers the current one while `link` is set.
    session: u64,
    /// Requests sent and not yet ended, by kind and id, with how many are open.
    pending: HashMap<(IdKind, u64), u32>,
    demand: Option<Demand>,
    table_query: Option<TableQuery>,
    /// The newest query per system table.
    sys_queries: Vec<SysTableQuery>,
    /// Event senders of the session's open journal streams.
    journals: HashMap<StreamId, async_channel::Sender<JournalEvent>>,
    next_stream: u32,
    status: Option<LinkStatus>,
    host: Option<HostIdentity>,
    /// The current connection's `Connection::shutdown`.
    shutdown: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// The writer's inputs for one session.
struct Link {
    control: async_channel::Sender<ClientMsg>,
    commands: async_channel::Sender<ClientMsg>,
    demand: Arc<Mutex<Option<Demand>>>,
    /// One token: there is something to write. Closing it stops the writer.
    doorbell: async_channel::Sender<()>,
}

impl Link {
    fn ring(&self) {
        let _ = self.doorbell.try_send(());
    }

    /// Queues `msg` on the control path without waking the writer; `ring` does that.
    fn queue_control(&self, msg: ClientMsg) {
        // Unbounded: only a closed channel refuses, and then the writer is gone.
        let _ = self.control.try_send(msg);
    }

    fn control(&self, msg: ClientMsg) {
        self.queue_control(msg);
        self.ring();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RemoteClient {
    /// Starts the connection thread, which connects at once. `latest` is an empty snapshot with
    /// sequence 0 until the first frame is installed.
    pub fn start(connector: impl Connector, config: ClientConfig) -> io::Result<Self> {
        assert!(!config.backoff.is_empty(), "backoff must not be empty");
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        // Unbounded so final states always fit; `send_event` caps progress at `ACTION_EVENTS`.
        let (events_tx, events_rx) = async_channel::unbounded();
        let (requests_tx, requests_rx) = async_channel::unbounded();
        let shared = Arc::new(Shared {
            latest: ArcSwap::from_pointee(Installed {
                generation: 0,
                snap: Arc::new(Snapshot::empty(BootId(0), 0)),
            }),
            stopped: AtomicBool::new(false),
            wake_tx,
            wake_rx,
            events_tx,
            events_rx,
            requests_tx,
            requests_rx,
            state: Mutex::new(State {
                next_stream: 1,
                status: Some(LinkStatus::Connecting),
                ..State::default()
            }),
            #[cfg(feature = "test-hooks")]
            observer: {
                let (tx, rx) = async_channel::unbounded();
                Observer { tx, rx }
            },
        });
        let (stop_tx, stop_rx) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("tl-remote-conn".into())
            .spawn(move || connection_loop(&thread_shared, &connector, &config, &stop_rx))?;
        Ok(Self {
            shared,
            _stop: stop_tx,
        })
    }

    pub fn status(&self) -> LinkStatus {
        lock(&self.shared.state)
            .status
            .clone()
            .unwrap_or(LinkStatus::Connecting)
    }

    /// The engine of the newest session, from its `Welcome`.
    pub fn host(&self) -> Option<HostIdentity> {
        lock(&self.shared.state).host.clone()
    }

    /// Every step the client takes from its start, in order. Each receiver shares one queue,
    /// so a step goes to one of them.
    #[cfg(feature = "test-hooks")]
    pub fn observe(&self) -> async_channel::Receiver<Observed> {
        self.shared.observer.rx.clone()
    }
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        let shutdown = {
            let mut st = lock(&self.shared.state);
            st.link = None;
            st.shutdown.take()
        };
        if let Some(shutdown) = shutdown {
            shutdown();
        }
    }
}

impl EngineClient for RemoteClient {
    fn latest(&self) -> Arc<Snapshot> {
        Arc::clone(&self.shared.latest.load().snap)
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.shared.wake_rx.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.shared.events_rx.clone()
    }

    fn request_events(&self) -> async_channel::Receiver<RequestEvent> {
        self.shared.requests_rx.clone()
    }

    fn generation(&self) -> u64 {
        self.shared.latest.load().generation
    }

    fn latest_with_generation(&self) -> (u64, Arc<Snapshot>) {
        let installed = self.shared.latest.load();
        (installed.generation, Arc::clone(&installed.snap))
    }

    fn set_demand(&self, demand: Demand) {
        let mut st = lock(&self.shared.state);
        if st.demand.as_ref() == Some(&demand) {
            return;
        }
        if let Some(link) = &st.link {
            *lock(&link.demand) = Some(demand.clone());
            link.ring();
        }
        st.demand = Some(demand);
    }

    /// The engine settings that stay with the window move into the session's table query and
    /// demand, and host settings get their own messages (ADR-018 Settings, tasklens-q4x76.9).
    /// Until then nothing is sent.
    fn set_settings(&self, _settings: EngineSettings) {}

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        let mut st = lock(&self.shared.state);
        match &cmd {
            Command::SetTableQuery(q) => st.table_query = Some(q.clone()),
            Command::SetSysTableQuery(q) => {
                st.sys_queries.retain(|old| old.table != q.table);
                st.sys_queries.push(q.clone());
            }
            _ => {}
        }
        let request = request_id(&cmd);
        let Some(link) = &st.link else {
            return Err(CommandError::EngineStopped);
        };
        match link.commands.try_send(ClientMsg::Command(cmd)) {
            Ok(()) => {
                link.ring();
                // Under the lock, so the reader cannot see the end before the request is
                // counted, and a drop cannot miss it.
                if let Some(key) = request {
                    *st.pending.entry(key).or_insert(0) += 1;
                }
                Ok(())
            }
            Err(async_channel::TrySendError::Full(_)) => Err(CommandError::Busy),
            Err(async_channel::TrySendError::Closed(_)) => Err(CommandError::EngineStopped),
        }
    }

    /// Opens the stream on the server. While disconnected, or when the command queue is full,
    /// the handle's events are closed at once. Dropping the handle sends `JournalClose` on the
    /// control path.
    fn journal(&self, query: JournalQuery) -> JournalHandle {
        let (events_tx, events_rx) = async_channel::bounded(JOURNAL_EVENTS);
        let (control_tx, control_rx) = async_channel::bounded(JOURNAL_CONTROLS);
        let handle = JournalHandle::new(events_rx, control_tx);
        let mut st = lock(&self.shared.state);
        let Some(link) = &st.link else {
            return handle;
        };
        let stream = StreamId(st.next_stream);
        if link
            .commands
            .try_send(ClientMsg::JournalOpen { stream, query })
            .is_err()
        {
            return handle;
        }
        link.ring();
        st.next_stream += 1;
        st.journals.insert(stream, events_tx);
        let session = st.session;
        drop(st);
        let shared = Arc::clone(&self.shared);
        let spawned = thread::Builder::new()
            .name("tl-remote-journal".into())
            .spawn(move || forward_journal(&shared, session, stream, &control_rx));
        if spawned.is_err() {
            // Without its forwarder the stream could never close: close it now.
            let mut st = lock(&self.shared.state);
            close_stream(&mut st, session, stream);
        }
        handle
    }
}

/// The request a command starts, if it is one that ends with a `RequestEvent`.
fn request_id(cmd: &Command) -> Option<(IdKind, u64)> {
    match cmd {
        Command::Action(req) => Some((IdKind::Action, req.id.0)),
        Command::StringScan { scan, .. } => Some((IdKind::Scan, scan.0)),
        Command::HandleSearch { search, .. } => Some((IdKind::Search, search.0)),
        Command::Device(DeviceCommand::Eject { seq, .. }) => Some((IdKind::Eject, *seq)),
        Command::HistoryQuery { id, .. } => Some((IdKind::History, *id)),
        _ => None,
    }
}

/// Sends the handle's control messages until the handle drops, then closes the stream.
fn forward_journal(
    shared: &Shared,
    session: u64,
    stream: StreamId,
    control_rx: &async_channel::Receiver<JournalControl>,
) {
    while let Ok(control) = control_rx.recv_blocking() {
        let st = lock(&shared.state);
        if st.session != session {
            continue;
        }
        if let Some(link) = &st.link {
            // A full queue drops it, as for the in-process engine's control channel.
            if link
                .commands
                .try_send(ClientMsg::JournalCtl { stream, control })
                .is_ok()
            {
                link.ring();
            }
        }
    }
    let mut st = lock(&shared.state);
    close_stream(&mut st, session, stream);
}

/// Forgets `stream` and sends `JournalClose`, if `session` is still open. A later session may
/// use the same stream number, so nothing is touched then.
fn close_stream(st: &mut State, session: u64, stream: StreamId) {
    if st.session != session {
        return;
    }
    if let Some(link) = &st.link {
        st.journals.remove(&stream);
        link.control(ClientMsg::JournalClose { stream });
    }
}

/// Connects, runs sessions and waits out the backoff until the client drops.
fn connection_loop(
    shared: &Shared,
    connector: &dyn Connector,
    config: &ClientConfig,
    stop: &mpsc::Receiver<()>,
) {
    let mut attempt = 0usize;
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            return;
        }
        set_status(shared, LinkStatus::Connecting);
        match connector.connect() {
            Ok(conn) => match run_session(shared, conn, &config.hello) {
                Ok(()) => {
                    attempt = 0;
                    set_status(shared, LinkStatus::Connecting);
                }
                Err(reason) => set_status(shared, LinkStatus::Failed(reason)),
            },
            Err(e) => set_status(shared, LinkStatus::Failed(Arc::from(e.to_string()))),
        }
        let wait = config.backoff[attempt.min(config.backoff.len() - 1)];
        attempt = attempt.saturating_add(1);
        match stop.recv_timeout(wait) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn set_status(shared: &Shared, status: LinkStatus) {
    lock(&shared.state).status = Some(status);
}

/// Runs one connection: the handshake, then the session until the stream ends. `Ok` when a
/// session was opened, `Err` with the reason when the handshake failed.
fn run_session(shared: &Shared, conn: Connection, hello: &HelloTail) -> Result<(), Arc<str>> {
    let Connection {
        mut reader,
        mut writer,
        shutdown,
    } = conn;
    {
        let mut st = lock(&shared.state);
        st.shutdown = Some(Arc::clone(&shutdown));
    }
    // The client may have dropped before the shutdown was stored.
    if shared.stopped.load(Ordering::Acquire) {
        shutdown();
        return Err(Arc::from("client closed"));
    }
    let mut frames = FrameReader::new().map_err(|e| Arc::from(e.to_string()))?;
    let identity = match handshake(&mut reader, &mut writer, &frames, hello) {
        Ok(identity) => identity,
        Err(reason) => {
            shutdown();
            lock(&shared.state).shutdown = None;
            return Err(reason);
        }
    };
    let host_name = Arc::clone(&identity.host_name);
    let (session, link_inputs) = open_link(shared, identity);
    spawn_writer(writer, link_inputs, Arc::clone(&shutdown));

    // Ends with the first read, decode or apply error, EOF included.
    // 0 until the session's first frame, so that frame must be complete.
    let mut installed_frame = 0u64;
    let mut first = true;
    while let Ok(raw) = frames.read_frame(&mut reader) {
        let Ok(msg) = frames.decode_server(&raw) else {
            break;
        };
        match msg {
            ServerMsg::Snapshot(frame) => {
                let installed = Arc::clone(&shared.latest.load().snap);
                let Ok(snap) =
                    delta::apply(&installed, installed_frame, &frame, installed.sequence + 1)
                else {
                    break;
                };
                let first_of_session = std::mem::replace(&mut first, false);
                // Reported before the store that lets the UI read the snapshot.
                observe!(
                    shared.observer,
                    Observed::SnapshotInstalled {
                        generation: session,
                        frame: frame.frame,
                    }
                );
                if first_of_session {
                    observe!(shared.observer, Observed::Generation(session));
                }
                // One store for both, so the hub never sees this session's number with the last
                // session's data (ADR-018 Delta flow).
                shared.latest.store(Arc::new(Installed {
                    generation: session,
                    snap: Arc::new(snap),
                }));
                let _ = shared.wake_tx.try_send(());
                installed_frame = frame.frame;
                let mut st = lock(&shared.state);
                if first_of_session {
                    st.status = Some(LinkStatus::Connected);
                }
                if let Some(link) = &st.link {
                    // Reported before the push: a writer already in its loop can take the Ack
                    // without waiting for `ring`, so `AckWritten` must not be able to come first.
                    observe!(shared.observer, Observed::AckQueued(frame.frame));
                    link.queue_control(ClientMsg::Ack { frame: frame.frame });
                    link.ring();
                }
            }
            ServerMsg::Action(ev) => send_event(shared, ev),
            ServerMsg::Request(ev) => {
                let mut st = lock(&shared.state);
                if take_pending(&mut st.pending, (ev.kind, ev.id)) {
                    // Reported before the send that lets the UI read the end.
                    observe!(shared.observer, Observed::RequestEnded(ev));
                    let _ = shared.requests_tx.try_send(ev);
                }
            }
            ServerMsg::Journal { stream, event } => {
                let st = lock(&shared.state);
                if let Some(tx) = st.journals.get(&stream) {
                    let _ = tx.try_send(event);
                }
            }
            // Host settings replies have no reader yet (tasklens-q4x76.9).
            ServerMsg::HostSettings(_) | ServerMsg::SetHostSettings(_) => {}
        }
    }
    shutdown();
    end_session(shared, &host_name);
    // After its effects: a test waits for this step and then checks that the link is gone and
    // the ends were sent. The next session's steps come from this thread, so they follow it.
    observe!(shared.observer, Observed::SessionEnded(session));
    Ok(())
}

/// Sends `Hello` and reads the server's answer.
fn handshake(
    reader: &mut Box<dyn Read + Send>,
    writer: &mut Box<dyn Write + Send>,
    frames: &FrameReader,
    hello: &HelloTail,
) -> Result<HostIdentity, Arc<str>> {
    let mut out = Vec::new();
    encode_hello(
        &Hello {
            prefix: BootPrefix::ours(),
            tail: Some(hello.clone()),
        },
        &mut out,
    )
    .map_err(|e| Arc::from(e.to_string()))?;
    writer
        .write_all(&out)
        .and_then(|()| writer.flush())
        .map_err(|e| Arc::from(e.to_string()))?;
    let frame = frames
        .read_frame(reader)
        .map_err(|e| Arc::from(e.to_string()))?;
    match decode_server_bootstrap(&frame).map_err(|e| Arc::from(e.to_string()))? {
        ServerBootstrap::Welcome(Welcome {
            tail: Some(identity),
            ..
        }) => Ok(identity),
        ServerBootstrap::Welcome(Welcome { prefix, .. }) => Err(Arc::from(format!(
            "the engine runs tasklens {} (protocol {}), this window speaks protocol {PROTO}",
            prefix.app_version, prefix.proto
        ))),
        ServerBootstrap::Refused(refused) => Err(refused.text),
    }
}

/// The writer's ends of one session's queues.
struct WriterInputs {
    control: async_channel::Receiver<ClientMsg>,
    commands: async_channel::Receiver<ClientMsg>,
    demand: Arc<Mutex<Option<Demand>>>,
    doorbell: async_channel::Receiver<()>,
    #[cfg(feature = "test-hooks")]
    observer: Observer,
}

/// Opens session state for a new connection and queues the session state to send first: the
/// demand and the table queries. Returns the session number.
fn open_link(shared: &Shared, identity: HostIdentity) -> (u64, WriterInputs) {
    let (control_tx, control) = async_channel::unbounded();
    let (commands_tx, commands) = async_channel::bounded(COMMAND_QUEUE);
    let (doorbell_tx, doorbell) = async_channel::bounded(1);
    let mut st = lock(&shared.state);
    let demand = Arc::new(Mutex::new(st.demand.clone()));
    let link = Link {
        control: control_tx,
        commands: commands_tx,
        demand: Arc::clone(&demand),
        doorbell: doorbell_tx,
    };
    // The queue is empty, so these fit: one table query and at most one per system table.
    if let Some(q) = &st.table_query {
        let _ = link
            .commands
            .try_send(ClientMsg::Command(Command::SetTableQuery(q.clone())));
    }
    for q in &st.sys_queries {
        let _ = link
            .commands
            .try_send(ClientMsg::Command(Command::SetSysTableQuery(q.clone())));
    }
    link.ring();
    st.session += 1;
    st.next_stream = 1;
    st.host = Some(identity);
    st.link = Some(link);
    let session = st.session;
    (
        session,
        WriterInputs {
            control,
            commands,
            demand,
            doorbell,
            #[cfg(feature = "test-hooks")]
            observer: shared.observer.clone(),
        },
    )
}

/// Closes the session's queues and ends every request still open: an action with an
/// `OutcomeUnknown` summary and then `Lost`, every other kind with `Lost`. Runs on the
/// connection thread after its last read, so no end from the server can follow.
fn end_session(shared: &Shared, host_name: &str) {
    let pending = {
        let mut st = lock(&shared.state);
        st.link = None;
        st.shutdown = None;
        // Dropping the senders closes each handle's events.
        st.journals.clear();
        std::mem::take(&mut st.pending)
    };
    let lost: Arc<str> = Arc::from(format!("connection to {host_name} lost"));
    for ((kind, id), count) in pending {
        for _ in 0..count {
            if kind == IdKind::Action {
                send_event(
                    shared,
                    ActionEvent {
                        id: ActionId(id),
                        target: None,
                        state: ActionState::OutcomeUnknown(Arc::clone(&lost)),
                    },
                );
            }
            let end = RequestEvent {
                kind,
                id,
                end: RequestEnd::Lost,
            };
            // Reported before the send that lets the UI read the end.
            observe!(shared.observer, Observed::RequestEnded(end));
            let _ = shared.requests_tx.try_send(end);
        }
    }
}

/// Passes an action event to the UI. Progress is dropped once `ACTION_EVENTS` wait; a final
/// state always goes, since the channel has no limit.
fn send_event(shared: &Shared, ev: ActionEvent) {
    if ev.state.is_final() || shared.events_tx.len() < ACTION_EVENTS {
        let _ = shared.events_tx.try_send(ev);
    }
}

/// Counts one end of `key`. False when no such request is open.
fn take_pending(pending: &mut HashMap<(IdKind, u64), u32>, key: (IdKind, u64)) -> bool {
    match pending.get_mut(&key) {
        Some(1) => {
            pending.remove(&key);
            true
        }
        Some(n) => {
            *n -= 1;
            true
        }
        None => false,
    }
}

fn spawn_writer(
    writer: Box<dyn Write + Send>,
    inputs: WriterInputs,
    shutdown: Arc<dyn Fn() + Send + Sync>,
) {
    let on_error = Arc::clone(&shutdown);
    let spawned = thread::Builder::new()
        .name("tl-remote-writer".into())
        .spawn(move || {
            if write_loop(writer, &inputs).is_err() {
                // The reader then fails too, and the session ends.
                on_error();
            }
        });
    if spawned.is_err() {
        // Without a writer nothing would be sent: end the session.
        shutdown();
    }
}

/// Writes queued messages, control path first, until the session's link is dropped.
fn write_loop(mut writer: Box<dyn Write + Send>, inputs: &WriterInputs) -> io::Result<()> {
    let mut frames = FrameWriter::new().map_err(io::Error::other)?;
    let mut out = Vec::new();
    let mut streams = StreamOrder::default();
    while inputs.doorbell.recv_blocking().is_ok() {
        loop {
            let msg = if let Ok(msg) = inputs.control.try_recv() {
                msg
            } else if let Some(demand) = lock(&inputs.demand).take() {
                ClientMsg::SetDemand(demand)
            } else if let Ok(msg) = inputs.commands.try_recv() {
                msg
            } else {
                break;
            };
            if !streams.pass(&msg) {
                continue;
            }
            out.clear();
            frames
                .encode_client(&msg, &mut out)
                .map_err(io::Error::other)?;
            // Reported before the write that lets the server answer the Ack.
            #[cfg(feature = "test-hooks")]
            if let ClientMsg::Ack { frame } = msg {
                let _ = inputs.observer.tx.try_send(Observed::AckWritten(frame));
            }
            writer.write_all(&out)?;
        }
        writer.flush()?;
    }
    Ok(())
}

/// Keeps one session's journal messages in order on the wire. `JournalOpen` and `JournalCtl`
/// wait in the command queue, `JournalClose` on the control path, so a handle dropped at once
/// can close its stream before the open is written. The server would then get the close for a
/// stream it does not know and keep the stream it opens next.
#[derive(Default)]
struct StreamOrder {
    /// Streams whose `JournalOpen` was written and that are not closed.
    open: HashSet<StreamId>,
    /// Streams closed before their `JournalOpen` was written.
    closed_early: HashSet<StreamId>,
}

impl StreamOrder {
    /// Whether to write `msg`. A close that comes before its open is held, and then neither is
    /// written. A control for a stream that is not open is not written either.
    fn pass(&mut self, msg: &ClientMsg) -> bool {
        match msg {
            ClientMsg::JournalOpen { stream, .. } => {
                if self.closed_early.remove(stream) {
                    return false;
                }
                self.open.insert(*stream);
                true
            }
            ClientMsg::JournalCtl { stream, .. } => self.open.contains(stream),
            ClientMsg::JournalClose { stream } => {
                if self.open.remove(stream) {
                    return true;
                }
                self.closed_early.insert(*stream);
                false
            }
            _ => true,
        }
    }
}
