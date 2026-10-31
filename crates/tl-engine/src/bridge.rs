//! `EngineHandle`, the UI's only view of the engine, and the `EngineHooks` later epics fill in
//! (ADR-005, ADR-001 rule 2).
//!
//! No public signature here names an asupersync type (D-001). The one exception is `HookCtx`,
//! which is `#[doc(hidden)]` and used only inside the crate and by E11's gate factory, and
//! `Engine::start_with_workers`, which is `#[doc(hidden)]` and used only by tests.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::runtime::{RootDrainOutcome, Runtime, RuntimeHandle};
use tl_collect::sysroot::{SysRoot, read_trimmed};
use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, KWinCommand};
use tl_model::demand::Demand;
use tl_model::history::RecordSink;
use tl_model::journal::{JournalError, JournalEvent, JournalHandle, JournalQuery};
use tl_model::remote_proto::{HostSettings, HostSettingsChanges, SetHostSettingsReply};
use tl_model::request::RequestEvent;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{ComponentHeader, Snapshot, WindowsSnapshot};
use tl_model::time::BootId;
use tl_model::validity::Validity;

use crate::EngineError;
use crate::client_session::{
    ClientMsg, ClientSessionId, OpenError, PortState, SESSIONS_MAX, SessionLink,
};
use crate::config::EngineConfig;
use crate::dbus::SharedHandles;
use crate::desktop::{APP_ID, DesktopSession, WindowGuard};
use crate::helper::LinkEnv;
use crate::reducer::{ActionGate, Publisher};
use crate::session::{RootEndpoints, engine_root};
use crate::source::WorkerControl;
use crate::update::EngineMsg;
use crate::workers::{WorkerSpec, workers_for};

/// Capacity of the action event channel (ADR-005).
pub const EVENTS_CAPACITY: usize = 256;
/// Capacity of the command and update queue (ADR-005).
pub const MSGS_CAPACITY: usize = 256;
/// How long `shutdown` waits for the root region to drain (ADR-004 step 2).
pub const DRAIN_BUDGET: Duration = Duration::from_millis(1000);
/// How long `shutdown` waits for runtime teardown (ADR-004 step 4).
pub const TEARDOWN_BUDGET: Duration = Duration::from_millis(500);
/// How long `open_session` waits for the reducer to admit or refuse the session. A reducer pass
/// takes milliseconds, so only a stuck engine reaches it.
pub const OPEN_WAIT: Duration = Duration::from_secs(2);
/// How often `open_session` checks for a stopped engine while it waits.
const OPEN_POLL: Duration = Duration::from_millis(50);

/// One engine's shutdown diagnostics: when its `shutdown` began, and which tasks and blocking
/// jobs are still running. Every thread of the engine's runtime holds it in a thread-local, set
/// by the runtime's thread start hook, so two engines in one process never mix their times.
#[derive(Default)]
pub(crate) struct ShutdownTrace {
    began: OnceLock<Instant>,
    running: Mutex<Vec<(u64, Running)>>,
    next_id: AtomicU64,
}

/// A task or blocking job that `ShutdownTrace` tracks.
#[derive(Clone, Copy)]
struct Running {
    kind: RunKind,
    name: &'static str,
    since: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Task,
    Job,
}

thread_local! {
    static TRACE: std::cell::RefCell<Option<Arc<ShutdownTrace>>> =
        const { std::cell::RefCell::new(None) };
}

impl ShutdownTrace {
    /// Makes `trace` the current thread's trace. The engine runtime calls it on every thread it
    /// starts: the async worker and each blocking pool thread.
    pub(crate) fn enter(trace: &Arc<ShutdownTrace>) {
        TRACE.with(|slot| *slot.borrow_mut() = Some(Arc::clone(trace)));
    }

    fn current() -> Option<Arc<ShutdownTrace>> {
        TRACE.with(|slot| slot.borrow().clone())
    }

    fn begin(&self) {
        let _ = self.began.set(Instant::now());
    }

    /// Time since this engine's `shutdown` began, or `None` before it.
    fn since_began(&self) -> Option<Duration> {
        self.began.get().map(Instant::elapsed)
    }

    fn running(&self) -> std::sync::MutexGuard<'_, Vec<(u64, Running)>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// "name (job, 1234 ms)" for everything still running, oldest first.
    fn still_running(&self) -> String {
        let mut text = String::new();
        for (_, run) in self.running().iter() {
            let kind = match run.kind {
                RunKind::Task => "task",
                RunKind::Job => "job",
            };
            if !text.is_empty() {
                text.push_str(", ");
            }
            let ms = run.since.elapsed().as_millis();
            text.push_str(&format!("{} ({kind}, {ms} ms)", run.name));
        }
        text
    }
}

/// Tracks one task or blocking job while it is alive. Dropping it logs the end when the engine
/// is shutting down. On a thread outside an engine runtime it does nothing.
pub(crate) struct RunGuard {
    trace: Option<(Arc<ShutdownTrace>, u64)>,
    run: Running,
}

impl RunGuard {
    fn new(kind: RunKind, name: &'static str) -> Self {
        let run = Running {
            kind,
            name,
            since: Instant::now(),
        };
        let trace = ShutdownTrace::current().map(|trace| {
            let id = trace.next_id.fetch_add(1, Ordering::Relaxed);
            trace.running().push((id, run));
            if kind == RunKind::Job {
                match trace.since_began() {
                    Some(since) => tracing::debug!(
                        target: "tasklens::engine",
                        job = name,
                        since_shutdown_ms = since.as_millis() as u64,
                        "blocking job started during shutdown",
                    ),
                    None => {
                        tracing::trace!(target: "tasklens::engine", job = name, "blocking job started")
                    }
                }
            }
            (trace, id)
        });
        Self { trace, run }
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let Some((trace, id)) = self.trace.take() else {
            return;
        };
        trace.running().retain(|(other, _)| *other != id);
        let took_ms = self.run.since.elapsed().as_millis() as u64;
        let name = self.run.name;
        match (self.run.kind, trace.since_began()) {
            (RunKind::Task, Some(since)) => tracing::debug!(
                target: "tasklens::engine",
                worker = name,
                since_shutdown_ms = since.as_millis() as u64,
                "worker exited during shutdown",
            ),
            (RunKind::Task, None) => {}
            (RunKind::Job, Some(since)) => tracing::debug!(
                target: "tasklens::engine",
                job = name,
                took_ms,
                since_shutdown_ms = since.as_millis() as u64,
                "blocking job finished during shutdown",
            ),
            (RunKind::Job, None) => tracing::trace!(
                target: "tasklens::engine",
                job = name,
                took_ms,
                "blocking job finished",
            ),
        }
    }
}

/// Tracks session task `name` until the guard drops. Create it inside the task.
pub(crate) fn task_running(name: &'static str) -> RunGuard {
    RunGuard::new(RunKind::Task, name)
}

/// Tracks blocking job `name` until the guard drops, and logs its start and finish. Create it
/// inside the job, or on the async side and move it into the job.
pub(crate) fn job_running(name: &'static str) -> RunGuard {
    RunGuard::new(RunKind::Job, name)
}

/// Opens one journal stream. Installed by E14-A4 through `EngineHooks`.
pub type JournalSpawner = Arc<dyn Fn(JournalQuery) -> JournalHandle + Send + Sync>;

/// Builds the action gate. Called once per reducer start.
pub type ActionGateFactory = Arc<dyn Fn(HookCtx) -> Box<dyn ActionGate> + Send + Sync>;

/// Builds the record sink from the engine config. Called once per reducer start.
pub type RecordSinkFactory = Arc<dyn Fn(&EngineConfig) -> Box<dyn RecordSink> + Send + Sync>;

/// Builds the helper task's `LinkEnv`. Called once per session (E29-020).
pub type HelperEnvFactory = Arc<dyn Fn() -> LinkEnv + Send + Sync>;

/// Optional parts that later epics install: the action gate (E11-A1), the journal (E14-A4),
/// the record sink (E30-A2) and the helper (E29-020). `Default` is `crate::default_hooks()`.
#[derive(Clone)]
pub struct EngineHooks {
    pub action_gate: Option<ActionGateFactory>,
    pub record_sink: Option<RecordSinkFactory>,
    pub journal: Option<JournalSpawner>,
    /// `None` runs no helper task: the helper and setup commands are dropped.
    pub helper: Option<HelperEnvFactory>,
}

impl Default for EngineHooks {
    fn default() -> Self {
        crate::default_hooks()
    }
}

/// What the action gate factory gets. It names asupersync's `mpsc::Sender`, so it stays out of
/// the documented public API (ADR-001).
#[doc(hidden)]
pub struct HookCtx {
    pub events_tx: async_channel::Sender<ActionEvent>,
    pub msgs_tx: mpsc::Sender<EngineMsg>,
    pub boot_id: BootId,
    pub config: Arc<EngineConfig>,
    pub shared: SharedHandles,
}

/// What `EngineHandle::shutdown` reports. `tl-app` calls `std::process::exit(0)` when it is not
/// clean (ADR-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    /// The root region drained within `DRAIN_BUDGET`.
    pub drained: bool,
    /// Runtime teardown finished within `TEARDOWN_BUDGET`.
    pub torn_down: bool,
    pub elapsed: Duration,
}

impl ShutdownReport {
    pub fn clean(&self) -> bool {
        self.drained && self.torn_down
    }
}

/// The UI side of a running engine. Cheap to clone: every clone shares one `Inner`.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<Inner>,
}

struct Inner {
    latest: Arc<ArcSwap<Snapshot>>,
    wake_rx: async_channel::Receiver<()>,
    /// `None` once `shutdown` has dropped them (ADR-004 step 1).
    links: Mutex<Option<Links>>,
    /// The last demand sent, so an equal demand does not wake the workers.
    last_demand: Mutex<Demand>,
    journal: Option<JournalSpawner>,
    rt: Mutex<Option<Runtime>>,
    /// Spawns the default journal reader in `EngineClient::journal`. `shutdown` takes it out, so
    /// a live `EngineHandle` clone does not keep the runtime alive.
    rt_handle: Mutex<Option<RuntimeHandle>>,
    stopped: AtomicBool,
    /// What `shutdown` names when the engine does not stop in time.
    trace: Arc<ShutdownTrace>,
    /// The ports `open_session` handed out whose session has not ended yet.
    ports: Mutex<Vec<(ClientSessionId, Arc<PortState>)>>,
    /// The host settings every snapshot carries (ADR-018 Settings). Revision 0 until
    /// `install_host_settings`; the reducer reads them at each publish.
    host: Arc<ArcSwap<HostSettings>>,
    /// Held while a change is checked against the current revision and stored.
    host_write: Mutex<()>,
    /// The host's desktop session; ports of local windows add their pid to it.
    desktop: DesktopSession,
}

/// The UI-side channel ends that `shutdown` drops first.
struct Links {
    events_rx: async_channel::Receiver<ActionEvent>,
    requests_rx: async_channel::Receiver<RequestEvent>,
    demand_tx: watch::Sender<Demand>,
    settings_tx: watch::Sender<EngineSettings>,
    msgs_tx: mpsc::Sender<EngineMsg>,
}

impl EngineHandle {
    /// Builds a handle from the channel ends `Engine::start` creates. The runtime slots start
    /// empty; the last demand starts as the demand channel's current value.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        latest: Arc<ArcSwap<Snapshot>>,
        wake_rx: async_channel::Receiver<()>,
        events_rx: async_channel::Receiver<ActionEvent>,
        requests_rx: async_channel::Receiver<RequestEvent>,
        demand_tx: watch::Sender<Demand>,
        settings_tx: watch::Sender<EngineSettings>,
        msgs_tx: mpsc::Sender<EngineMsg>,
        journal: Option<JournalSpawner>,
        trace: Arc<ShutdownTrace>,
        host: Arc<ArcSwap<HostSettings>>,
        desktop: DesktopSession,
    ) -> Self {
        let last_demand = Mutex::new(demand_tx.borrow().clone());
        Self {
            inner: Arc::new(Inner {
                latest,
                wake_rx,
                links: Mutex::new(Some(Links {
                    events_rx,
                    requests_rx,
                    demand_tx,
                    settings_tx,
                    msgs_tx,
                })),
                last_demand,
                journal,
                rt: Mutex::new(None),
                rt_handle: Mutex::new(None),
                stopped: AtomicBool::new(false),
                trace,
                ports: Mutex::new(Vec::new()),
                host,
                host_write: Mutex::new(()),
                desktop,
            }),
        }
    }

    fn links(&self) -> std::sync::MutexGuard<'_, Option<Links>> {
        self.inner
            .links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens client session `id` for the engine service (ADR-018) and returns its port once the
    /// reducer has the session in its registry. A reducer refusal comes back as its
    /// `OpenError`, so the server never welcomes a session the engine does not have. At most
    /// `SESSIONS_MAX - 1` ports are open at once, beside the local session. A port's slot frees
    /// when its session ends: the reducer has seen the port dropped, or removed the session
    /// itself.
    ///
    /// Blocks until the reducer answers, at most `OPEN_WAIT`. Without an answer by then the
    /// open is withdrawn and the result is `Err(Busy)`.
    pub fn open_session(&self, id: ClientSessionId) -> Result<SessionPort, OpenError> {
        self.open_session_until(id, Instant::now() + OPEN_WAIT)
    }

    /// `open_session` that waits for the reducer only until `deadline`, and never longer than
    /// `OPEN_WAIT`. The engine service passes its handshake deadline, so an open cannot keep a
    /// connection in its handshake past it. A deadline already passed is `Err(Busy)` with no
    /// open sent.
    pub fn open_session_until(
        &self,
        id: ClientSessionId,
        deadline: Instant,
    ) -> Result<SessionPort, OpenError> {
        self.open_session_from(id, None, deadline)
    }

    /// `open_session_until` for a session whose `Hello` named `peer` (ADR-018). A window on
    /// this host (not a bridge) counts as "self" while its port lives, and may send the KWin
    /// commands for its own windows. A session with no peer may not send those.
    pub fn open_session_from(
        &self,
        id: ClientSessionId,
        peer: Option<SessionPeer>,
        deadline: Instant,
    ) -> Result<SessionPort, OpenError> {
        if Instant::now() >= deadline {
            return Err(OpenError::Busy);
        }
        let port = self.send_open(id, peer)?;
        let deadline = deadline.min(Instant::now() + OPEN_WAIT);
        let stopped = || self.inner.stopped.load(Ordering::Acquire);
        match port.state.wait_admission(deadline, OPEN_POLL, stopped) {
            Some(Ok(())) => Ok(port),
            Some(Err(e)) => Err(e),
            // Dropping the port withdraws the open: the reducer refuses a closed port, or closes
            // the session at its next pass if it opened it meanwhile.
            None if stopped() => Err(OpenError::Stopped),
            None => Err(OpenError::Busy),
        }
    }

    /// The checks of `open_session` and its `OpenPort` message. The port it returns is not
    /// admitted yet.
    fn send_open(
        &self,
        id: ClientSessionId,
        peer: Option<SessionPeer>,
    ) -> Result<SessionPort, OpenError> {
        if id == ClientSessionId::LOCAL {
            return Err(OpenError::Duplicate);
        }
        if self.inner.stopped.load(Ordering::Acquire) {
            return Err(OpenError::Stopped);
        }
        let links = self.links();
        let Some(links) = &*links else {
            return Err(OpenError::Stopped);
        };
        let mut ports = self
            .inner
            .ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ports.retain(|(_, port)| !port.is_ended());
        if ports.iter().any(|(open, _)| *open == id) {
            return Err(OpenError::Duplicate);
        }
        if ports.len() >= SESSIONS_MAX - 1 {
            return Err(OpenError::Full);
        }
        let now = self.inner.latest.load();
        let latest = Arc::new(ArcSwap::from_pointee(Snapshot::empty(
            now.boot_id,
            now.epoch,
        )));
        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let (events_tx, events_rx) = async_channel::bounded::<ActionEvent>(EVENTS_CAPACITY);
        // Unbounded: an end is never dropped, and admission bounds how many are outstanding.
        let (requests_tx, requests_rx) = async_channel::unbounded::<RequestEvent>();
        let state = Arc::new(PortState::default());
        let link = SessionLink {
            latest: Arc::clone(&latest),
            wake_tx,
            events_tx,
            requests_tx,
        };
        let open = ClientMsg::OpenPort {
            id,
            link,
            port: Arc::clone(&state),
        };
        links
            .msgs_tx
            .try_send(EngineMsg::Client(open))
            .map_err(|e| match e {
                mpsc::SendError::Full(_) => OpenError::Busy,
                mpsc::SendError::Disconnected(_) | mpsc::SendError::Cancelled(_) => {
                    OpenError::Stopped
                }
            })?;
        ports.push((id, Arc::clone(&state)));
        let window = peer
            .as_ref()
            .filter(|p| !p.bridge)
            .map(|p| self.inner.desktop.add_window(p.pid));
        Ok(SessionPort {
            id,
            latest,
            wake_rx,
            events_rx,
            requests_rx,
            state,
            handle: self.clone(),
            peer,
            _window: window,
        })
    }

    /// Sends `ClientMsg::Wake` when the queue has room. The reducer reads the ports' shared state
    /// on every pass anyway, so a full queue only delays it until the next tick.
    /// Makes `current` the host settings: the engine's workers use its values from now on, and
    /// the next snapshot of every session carries it. The owner of `engine.toml` calls this at
    /// start and after each change it stored (ADR-018 Settings).
    pub fn install_host_settings(&self, current: HostSettings) {
        let _write = self
            .inner
            .host_write
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.store_host_settings(current);
    }

    /// The host settings the engine runs with.
    pub fn host_settings(&self) -> Arc<HostSettings> {
        self.inner.host.load_full()
    }

    /// Stores `current` and passes its values to the workers. The caller holds `host_write`.
    fn store_host_settings(&self, current: HostSettings) {
        let settings = current.values.engine_settings();
        self.inner.host.store(Arc::new(current));
        if let Some(links) = &*self.links() {
            // An error means the engine has stopped, and then the settings no longer matter.
            let _ = links.settings_tx.send(settings);
        }
        // A publish now, so every session sees the new revision without waiting for a worker.
        self.wake_reducer();
    }

    fn wake_reducer(&self) {
        if let Some(links) = &*self.links() {
            let _ = links.msgs_tx.try_send(EngineMsg::Client(ClientMsg::Wake));
        }
    }

    /// Stops the engine in the five ADR-004 steps and consumes the handle. Call it on the main
    /// thread after GPUI has stopped (ADR-010). Other `EngineHandle` clones may stay alive: GPUI
    /// never drops its app state, so `tl-app` cannot drop the UI's clone. They see a stopped
    /// engine afterwards.
    ///
    /// `DRAIN_BUDGET` and `TEARDOWN_BUDGET` are the bounds, 1.5 s together. Blocking jobs that
    /// wait on a child process or walk many items check for cancel and stop early. A single
    /// system call that is still running is not interrupted; teardown then goes on in the
    /// background and `torn_down` is false. A missed budget logs the tasks and blocking jobs
    /// still running.
    pub fn shutdown(self) -> ShutdownReport {
        let begin = Instant::now();
        let inner = self.inner;
        let trace = Arc::clone(&inner.trace);
        trace.begin();
        inner.stopped.store(true, Ordering::Release);
        // Step 1: drop the UI-side senders.
        drop(
            inner
                .links
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        let rt = inner
            .rt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(rt) = rt else {
            tracing::warn!(target: "tasklens::engine", "shutdown: no runtime; it already ran");
            return ShutdownReport {
                drained: false,
                torn_down: false,
                elapsed: begin.elapsed(),
            };
        };
        // Step 2: cancel the root region with `CancelKind::Shutdown` and wait for it.
        let drained = rt.drain_root_region(DRAIN_BUDGET) == RootDrainOutcome::Quiescent;
        let mut still_running = String::new();
        if !drained {
            still_running = trace.still_running();
            tracing::warn!(
                target: "tasklens::engine",
                still_running = %still_running,
                "engine did not drain within {} ms",
                DRAIN_BUDGET.as_millis(),
            );
        }
        // Step 3: drop every `RuntimeHandle` clone. A live `EngineHandle` clone keeps `Inner`,
        // so take the handle out rather than wait for `Inner` to drop.
        drop(
            inner
                .rt_handle
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        drop(inner);
        // Step 4.
        let torn_down = rt.shutdown_timeout(TEARDOWN_BUDGET);
        if !torn_down {
            still_running = trace.still_running();
        }
        // Step 5.
        let report = ShutdownReport {
            drained,
            torn_down,
            elapsed: begin.elapsed(),
        };
        if !report.clean() {
            tracing::warn!(
                target: "tasklens::engine",
                drained,
                torn_down,
                elapsed_ms = report.elapsed.as_millis() as u64,
                still_running = %still_running,
                "engine shutdown did not finish cleanly",
            );
        }
        report
    }
}

/// One client session of the engine service (ADR-018), from `EngineHandle::open_session`: its
/// snapshots, events and request ends, and the way to send it demand and commands. Dropping the
/// port closes the session. The close goes through state the reducer reads on every pass, not
/// through the message queue, so it is never lost.
pub struct SessionPort {
    id: ClientSessionId,
    latest: Arc<ArcSwap<Snapshot>>,
    wake_rx: async_channel::Receiver<()>,
    events_rx: async_channel::Receiver<ActionEvent>,
    requests_rx: async_channel::Receiver<RequestEvent>,
    state: Arc<PortState>,
    handle: EngineHandle,
    /// Who opened the session, from its `Hello`.
    peer: Option<SessionPeer>,
    /// A window on this host is "self" while its port lives.
    _window: Option<WindowGuard>,
}

/// Who opened a session, from its `Hello` (ADR-018).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPeer {
    /// The window's process id, on the host the window runs on.
    pub pid: u32,
    /// The window's app id, which its windows carry as their `resourceClass`.
    pub app_id: Arc<str>,
    /// The session comes through `tasklens bridge`, so the window runs on another host.
    pub bridge: bool,
}

/// True when `peer`'s session may send `cmd` (leader decision on q4x76.12). Keep above and frame
/// geometry name a tasklens window by its pid: only a window on this host may send them, for its
/// own pid and app id. A bridge session's pid names a process on another host. Other KWin
/// commands act on any window and pass.
pub fn own_window_allowed(peer: Option<&SessionPeer>, cmd: &KWinCommand) -> bool {
    let pid = match cmd {
        KWinCommand::KeepAbove { pid, .. } | KWinCommand::SetFrameGeometry { pid, .. } => *pid,
        KWinCommand::Activate { .. }
        | KWinCommand::Close { .. }
        | KWinCommand::Pick { .. }
        | KWinCommand::CancelPick { .. } => return true,
    };
    peer.is_some_and(|p| !p.bridge && p.pid == pid && *p.app_id == *APP_ID)
}

impl std::fmt::Debug for SessionPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionPort")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl SessionPort {
    pub fn id(&self) -> ClientSessionId {
        self.id
    }

    /// The session's newest snapshot. Until the reducer first publishes for the session, an
    /// empty snapshot with sequence 0.
    pub fn latest(&self) -> Arc<Snapshot> {
        self.latest.load_full()
    }

    /// One token after each publish, as `EngineClient::wake`. Closed once the session ended.
    pub fn wake(&self) -> async_channel::Receiver<()> {
        self.wake_rx.clone()
    }

    /// The session's action events, with its own action ids.
    pub fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.events_rx.clone()
    }

    /// The end of each of the session's requests, with its own ids.
    pub fn request_events(&self) -> async_channel::Receiver<RequestEvent> {
        self.requests_rx.clone()
    }

    /// The session's demand. The reducer takes the newest one at its next pass; a newer demand
    /// replaces one it has not taken, so none is lost to a full queue.
    pub fn set_demand(&self, demand: Demand) {
        self.state.set_demand(demand);
        self.handle.wake_reducer();
    }

    /// A command from this session (`ClientMsg::Command`). Never blocks: a full queue is
    /// `Err(Busy)`, a stopped engine or an ended session `Err(EngineStopped)`.
    /// A KWin command `own_window_allowed` refuses is dropped, as the KWin worker drops a
    /// command for a window it cannot find (R-170).
    pub fn send(&self, cmd: Command) -> Result<(), CommandError> {
        if self.is_ended() {
            return Err(CommandError::EngineStopped);
        }
        if let Command::KWin(kwin) = &cmd
            && !own_window_allowed(self.peer.as_ref(), kwin)
        {
            tracing::debug!(target: "tasklens::engine", id = self.id.0, "kwin command dropped: not this session's window");
            return Ok(());
        }
        let links = self.handle.links();
        let Some(links) = &*links else {
            return Err(CommandError::EngineStopped);
        };
        let msg = ClientMsg::Command {
            id: self.id,
            cmd: Box::new(cmd),
        };
        links
            .msgs_tx
            .try_send(EngineMsg::Client(msg))
            .map_err(|e| match e {
                mpsc::SendError::Full(_) => CommandError::Busy,
                mpsc::SendError::Disconnected(_) | mpsc::SendError::Cancelled(_) => {
                    CommandError::EngineStopped
                }
            })
    }

    /// The engine removed the session (its link broke or its reducer stopped) or stopped. The
    /// server then closes the connection.
    pub fn is_ended(&self) -> bool {
        self.state.is_ended() || self.handle.inner.stopped.load(Ordering::Acquire)
    }
}

impl Drop for SessionPort {
    fn drop(&mut self) {
        self.state.close();
        self.handle.wake_reducer();
    }
}

/// Starts the engine. `tl-app` calls this once (ADR-001).
pub struct Engine;

impl Engine {
    /// Builds the runtime and the channels, publishes the empty snapshot and spawns
    /// `engine_root` in the root region, so shutdown cancellation reaches it (ADR-004).
    pub fn start(config: EngineConfig, hooks: EngineHooks) -> Result<EngineHandle, EngineError> {
        let workers = workers_for(config.mode);
        Self::start_with_workers(config, hooks, workers)
    }

    /// `start` with `workers` in place of `workers_for(config.mode)`, so tests can run fake
    /// workers on the real runtime. A `WorkerSpec` factory takes an asupersync `Cx`, so this
    /// stays out of the documented API (ADR-001).
    #[doc(hidden)]
    pub fn start_with_workers(
        config: EngineConfig,
        hooks: EngineHooks,
        workers: Vec<WorkerSpec>,
    ) -> Result<EngineHandle, EngineError> {
        let trace = Arc::new(ShutdownTrace::default());
        let rt = crate::runtime::build_traced_runtime(Arc::clone(&trace))
            .map_err(|e| EngineError::Runtime(e.to_string().into()))?;
        let boot_id = read_boot_id(&config.sys_root);
        let desktop = config.desktop.clone();

        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let (events_tx, events_rx) = async_channel::bounded::<ActionEvent>(EVENTS_CAPACITY);
        // Unbounded: an end is never dropped, and admission bounds how many are outstanding.
        let (requests_tx, requests_rx) = async_channel::unbounded::<RequestEvent>();
        let (msgs_tx, msgs_rx) = mpsc::channel::<EngineMsg>(MSGS_CAPACITY);
        let (demand_tx, demand_rx) = watch::channel(Demand::default());
        let (settings_tx, settings_rx) = watch::channel(EngineSettings::default());
        let (control_tx, _control_rx) = watch::channel(WorkerControl::default());
        let latest = Arc::new(ArcSwap::from_pointee(empty_snapshot(boot_id)));
        let host_settings = Arc::new(ArcSwap::from_pointee(HostSettings::default()));

        let journal = hooks.journal.clone();
        let ep = RootEndpoints {
            msgs: msgs_rx,
            msgs_tx: msgs_tx.clone(),
            demand: demand_rx,
            settings: settings_rx,
            host_settings: Arc::clone(&host_settings),
            control: control_tx,
            publisher: Publisher {
                latest: latest.clone(),
                wake_tx,
                stop_publishing: false,
            },
            events_tx,
            requests_tx,
            config: Arc::new(config),
            boot_id,
            hooks,
            workers,
            next_sequence: 0,
            last_engine_id: 0,
        };
        let rt_handle = rt.handle();
        rt_handle
            .try_spawn_with_cx(move |cx| engine_root(cx, ep))
            .map_err(|e| EngineError::Spawn(e.to_string().into()))?;

        let handle = EngineHandle::new(
            latest,
            wake_rx,
            events_rx,
            requests_rx,
            demand_tx,
            settings_tx,
            msgs_tx,
            journal,
            trace,
            host_settings,
            desktop,
        );
        *handle
            .inner
            .rt
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(rt);
        *handle
            .inner
            .rt_handle
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(rt_handle);
        Ok(handle)
    }
}

/// The folded boot id from `proc/sys/kernel/random/boot_id` under `root` (ADR-003). A missing
/// or malformed file gives `BootId(0)` and a warning; the engine still runs.
fn read_boot_id(root: &SysRoot) -> BootId {
    let mut buf = Vec::new();
    let parsed = read_trimmed(&root.proc_path("sys/kernel/random/boot_id"), &mut buf)
        .ok()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(BootId::from_uuid_text);
    parsed.unwrap_or_else(|| {
        tracing::warn!(target: "tasklens::engine", "boot id not readable; using 0");
        BootId(0)
    })
}

/// Sequence 0 and epoch 1, the first session's epoch. Every component is `WarmingUp` except
/// `windows`, which is `Unsupported` until the KWin worker publishes, as in `Reducer::new`.
fn empty_snapshot(boot_id: BootId) -> Snapshot {
    let mut snapshot = Snapshot::empty(boot_id, 1);
    snapshot.windows = Arc::new(WindowsSnapshot {
        header: ComponentHeader {
            validity: Validity::Unsupported,
            ..snapshot.windows.header
        },
        ..(*snapshot.windows).clone()
    });
    snapshot
}

/// A journal handle whose stream yields one `Error(err)` and then ends.
fn closed_journal(err: JournalError) -> JournalHandle {
    let (events_tx, events_rx) = async_channel::bounded(1);
    let (control_tx, _control_rx) = async_channel::bounded(1);
    // The channel is new and has one free slot, so this cannot fail.
    let _ = events_tx.try_send(JournalEvent::Error(err));
    JournalHandle::new(events_rx, control_tx)
}

impl EngineClient for EngineHandle {
    fn latest(&self) -> Arc<Snapshot> {
        self.inner.latest.load_full()
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.inner.wake_rx.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        match &*self.links() {
            Some(links) => links.events_rx.clone(),
            // After `shutdown`: a receiver whose sender is already gone.
            None => async_channel::bounded(1).1,
        }
    }

    /// The local session's channel, which `ClientSessions::set_local_requests` gives every
    /// reducer.
    fn request_events(&self) -> async_channel::Receiver<RequestEvent> {
        match &*self.links() {
            Some(links) => links.requests_rx.clone(),
            // After `shutdown`: a receiver whose sender is already gone.
            None => async_channel::unbounded().1,
        }
    }

    fn set_demand(&self, demand: Demand) {
        let mut last = self
            .inner
            .last_demand
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *last == demand {
            return;
        }
        *last = demand.clone();
        // `watch::Sender::send` needs no `Cx` and never blocks. An error means the engine has
        // stopped, and then the demand no longer matters.
        if let Some(links) = &*self.links() {
            let _ = links.demand_tx.send(demand);
        }
    }

    fn set_settings(&self, settings: EngineSettings) {
        if let Some(links) = &*self.links() {
            let _ = links.settings_tx.send(settings);
        }
    }

    /// Applies `changes` when `base_rev` is the current revision, else `Conflict`. Revision 0
    /// means no host settings were installed, so nothing can be changed yet. The engine keeps
    /// the result in memory only; the owner of `engine.toml` stores it (tasklens-q4x76.24).
    /// The in-process engine's generation is always 0, so a write with any other `generation`
    /// is not applied and gets a closed channel.
    fn set_host_settings(
        &self,
        generation: u64,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> Result<async_channel::Receiver<SetHostSettingsReply>, CommandError> {
        if self.inner.stopped.load(Ordering::Acquire) || self.links().is_none() {
            return Err(CommandError::EngineStopped);
        }
        if generation != self.generation() {
            return Ok(async_channel::bounded(1).1);
        }
        let reply = {
            let _write = self
                .inner
                .host_write
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let current = self.inner.host.load_full();
            if current.rev == 0 || base_rev != current.rev {
                SetHostSettingsReply::Conflict {
                    current: (*current).clone(),
                }
            } else {
                let rev = current.rev + 1;
                self.store_host_settings(HostSettings {
                    rev,
                    values: current.values.with_changes(&changes),
                    read_only: current.read_only,
                });
                SetHostSettingsReply::Ack { rev }
            }
        };
        let (tx, rx) = async_channel::bounded(1);
        // The channel is new and has one free slot, so this cannot fail.
        let _ = tx.try_send(reply);
        Ok(rx)
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        if self.inner.stopped.load(Ordering::Acquire) {
            return Err(CommandError::EngineStopped);
        }
        let links = self.links();
        let Some(links) = &*links else {
            return Err(CommandError::EngineStopped);
        };
        links
            .msgs_tx
            .try_send(EngineMsg::Command(cmd))
            .map_err(|e| match e {
                mpsc::SendError::Full(_) => CommandError::Busy,
                mpsc::SendError::Disconnected(_) | mpsc::SendError::Cancelled(_) => {
                    CommandError::EngineStopped
                }
            })
    }

    fn journal(&self, query: JournalQuery) -> JournalHandle {
        match &self.inner.journal {
            Some(spawn) => spawn(query),
            None => match &*self
                .inner
                .rt_handle
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
            {
                Some(rt) => crate::workers::journal::open(rt, query),
                None => closed_journal(JournalError::NotFound),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;
    use std::time::{Duration, Instant};

    use asupersync::Cx;
    use smallvec::SmallVec;
    use tl_model::actions::{ActionId, ActionKind, ActionRequest, Signal};
    use tl_model::health::SourceId;
    use tl_model::table::{Manager, UnitRef};

    use crate::config::EngineMode;
    use crate::reducer::AdmissionView;

    /// Command queue size from ADR-005.
    const QUEUE: usize = 256;

    struct Rig {
        handle: EngineHandle,
        demand_rx: watch::Receiver<Demand>,
        settings_rx: watch::Receiver<EngineSettings>,
        msgs_rx: mpsc::Receiver<EngineMsg>,
    }

    fn rig(journal: Option<JournalSpawner>) -> Rig {
        let latest = Arc::new(ArcSwap::from_pointee(Snapshot::empty(BootId(0), 0)));
        let (_wake_tx, wake_rx) = async_channel::bounded(1);
        let (_events_tx, events_rx) = async_channel::bounded(256);
        let (_requests_tx, requests_rx) = async_channel::unbounded();
        let (demand_tx, demand_rx) = watch::channel(Demand::default());
        let (settings_tx, settings_rx) = watch::channel(EngineSettings::default());
        let (msgs_tx, msgs_rx) = mpsc::channel(QUEUE);
        let handle = EngineHandle::new(
            latest,
            wake_rx,
            events_rx,
            requests_rx,
            demand_tx,
            settings_tx,
            msgs_tx,
            journal,
            Arc::default(),
            Arc::default(),
            DesktopSession::new(crate::desktop::DesktopSource::Process),
        );
        Rig {
            handle,
            demand_rx,
            settings_rx,
            msgs_rx,
        }
    }

    fn query() -> JournalQuery {
        JournalQuery {
            unit: UnitRef {
                manager: Manager::User,
                name: Arc::from("demo.service"),
            },
            follow: false,
        }
    }

    #[test]
    fn handle_is_send_sync_clone() {
        fn check<T: Send + Sync + Clone>() {}
        check::<EngineHandle>();
    }

    #[test]
    fn send_busy_when_full() {
        let r = rig(None);
        for _ in 0..QUEUE {
            assert_eq!(r.handle.send(Command::ResetBaselines), Ok(()));
        }
        assert_eq!(
            r.handle.send(Command::ResetBaselines),
            Err(CommandError::Busy)
        );
    }

    #[test]
    fn set_demand_dedups() {
        let mut r = rig(None);
        let d = Demand {
            memory: true,
            ..Demand::default()
        };
        r.handle.set_demand(d.clone());
        r.handle.set_demand(d.clone());
        assert!(r.demand_rx.has_changed());
        assert_eq!(*r.demand_rx.borrow_and_update(), d);
        // One send means one version step, so a worker's `changed()` wakes once.
        assert_eq!(r.demand_rx.seen_version(), 1);
        assert!(!r.demand_rx.has_changed());
        // A demand equal to the initial value is not sent either.
        let r2 = rig(None);
        r2.handle.set_demand(Demand::default());
        assert!(!r2.demand_rx.has_changed());
    }

    /// The one reply on a channel `set_host_settings` returned.
    fn reply(rx: &async_channel::Receiver<SetHostSettingsReply>) -> SetHostSettingsReply {
        rx.try_recv()
            .expect("the in-process engine replies at once")
    }

    #[test]
    fn host_settings_change_only_from_the_current_revision() {
        let mut r = rig(None);
        let changes = HostSettingsChanges {
            interval_ms: Some(2000),
            ..HostSettingsChanges::default()
        };
        // Nothing installed yet: revision 0, and no change applies, not even from base 0.
        assert_eq!(r.handle.host_settings().rev, 0);
        let rx = r.handle.set_host_settings(0, 0, changes.clone()).unwrap();
        assert!(matches!(
            reply(&rx),
            SetHostSettingsReply::Conflict { current } if current.rev == 0
        ));
        assert!(!r.settings_rx.has_changed());

        let mut installed = HostSettings {
            rev: 3,
            ..HostSettings::default()
        };
        installed.values.history.enabled = true;
        r.handle.install_host_settings(installed.clone());
        assert_eq!(*r.handle.host_settings(), installed);
        assert!(r.settings_rx.borrow_and_update().history.enabled);
        // Each store asks the reducer for a publish, so sessions see it at once.
        assert!(matches!(
            r.msgs_rx.try_recv(),
            Ok(EngineMsg::Client(ClientMsg::Wake))
        ));

        // A write tagged with another generation is refused and changes nothing.
        let rx = r.handle.set_host_settings(1, 3, changes.clone()).unwrap();
        assert!(rx.is_closed() && rx.try_recv().is_err());
        assert_eq!(r.handle.host_settings().rev, 3);

        let rx = r.handle.set_host_settings(0, 3, changes.clone()).unwrap();
        assert_eq!(reply(&rx), SetHostSettingsReply::Ack { rev: 4 });
        let now = r.handle.host_settings();
        assert_eq!((now.rev, now.values.interval_ms), (4, 2000));
        assert!(
            now.values.history.enabled,
            "what the change leaves out stays"
        );
        assert_eq!(r.settings_rx.borrow_and_update().interval_ms, 2000);

        // A second write from the same base is a conflict and gets the current settings.
        let rx = r
            .handle
            .set_host_settings(
                0,
                3,
                HostSettingsChanges {
                    interval_ms: Some(500),
                    ..HostSettingsChanges::default()
                },
            )
            .unwrap();
        assert_eq!(
            reply(&rx),
            SetHostSettingsReply::Conflict {
                current: (*now).clone()
            }
        );
        assert_eq!(r.handle.host_settings().values.interval_ms, 2000);

        r.handle.inner.stopped.store(true, Ordering::Release);
        assert!(matches!(
            r.handle.set_host_settings(0, 4, changes),
            Err(CommandError::EngineStopped)
        ));
    }

    #[test]
    fn send_after_stop() {
        let r = rig(None);
        r.handle.inner.stopped.store(true, Ordering::Release);
        assert_eq!(
            r.handle.send(Command::ResetBaselines),
            Err(CommandError::EngineStopped)
        );
    }

    #[test]
    fn send_after_receiver_dropped() {
        let r = rig(None);
        drop(r.msgs_rx);
        assert_eq!(
            r.handle.send(Command::ResetBaselines),
            Err(CommandError::EngineStopped)
        );
    }

    #[test]
    fn journal_without_spawner_errors_not_found() {
        let r = rig(None);
        let events = r.handle.journal(query()).events();
        match events.try_recv() {
            Ok(JournalEvent::Error(JournalError::NotFound)) => {}
            other => panic!("expected Error(NotFound), got {other:?}"),
        }
        assert!(matches!(
            events.try_recv(),
            Err(async_channel::TryRecvError::Closed)
        ));
    }

    #[test]
    fn journal_uses_spawner() {
        let spawner: JournalSpawner = Arc::new(|_q| closed_journal(JournalError::Timeout));
        let r = rig(Some(spawner));
        let events = r.handle.journal(query()).events();
        assert!(matches!(
            events.try_recv(),
            Ok(JournalEvent::Error(JournalError::Timeout))
        ));
    }

    #[test]
    fn start_returns_empty_snapshot() {
        let base =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/minimal-vm");
        let config = EngineConfig::new(EngineMode::LowMemory, base.join("state"))
            .with_sys_root(SysRoot::at(&base));
        let begin = Instant::now();
        let handle = Engine::start(config, EngineHooks::default()).expect("engine starts");
        // Start must not wait for a first sample. The bound is loose so a busy machine does not
        // fail it (tasklens-cr0.32). No bench scene measures engine start.
        assert!(
            begin.elapsed() < Duration::from_secs(2),
            "start took {:?}",
            begin.elapsed()
        );
        let snap = handle.latest();
        assert_eq!(snap.sequence, 0);
        // The fixture's boot id is 00000000-0000-4000-8000-000000000001 (R-267).
        assert_eq!(
            snap.boot_id,
            BootId::from_uuid_text("00000000-0000-4000-8000-000000000001").unwrap()
        );
        assert_eq!(snap.windows.header.validity, Validity::Unsupported);
        assert_eq!(snap.system.header.validity, Validity::WarmingUp);
    }

    #[test]
    fn wake_after_first_publish() {
        let base =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/minimal-vm");
        let config = EngineConfig::new(EngineMode::LowMemory, base.join("state"))
            .with_sys_root(SysRoot::at(&base));
        let handle = Engine::start(config, EngineHooks::default()).expect("engine starts");
        let wake = handle.wake();
        // The first `Tick` after 1 s makes the reducer publish.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(futures_lite::future::block_on(wake.recv()));
        });
        let woke = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("wake within 5 s");
        assert_eq!(woke, Ok(()));
        assert!(handle.latest().sequence >= 1);
    }

    /// Parks the reducer inside `submit` until the test releases it, so a test can hold a running
    /// reducer at a chosen point.
    struct ParkGate {
        parked: std::sync::mpsc::Sender<()>,
        release: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
    }

    impl ActionGate for ParkGate {
        fn submit(&mut self, _cx: &Cx, _req: ActionRequest, _view: &AdmissionView<'_>) {
            let _ = self.parked.send(());
            let _ = self
                .release
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recv_timeout(Duration::from_secs(10));
        }

        fn finished(&mut self, _id: ActionId) -> Option<SourceId> {
            None
        }
    }

    /// An engine on the fixture with a running reducer that `park` holds and `release` lets go.
    /// Low-memory mode has no workers, so only the test and the tick fill the queue.
    struct Live {
        handle: EngineHandle,
        parked_rx: std::sync::mpsc::Receiver<()>,
        release_tx: std::sync::mpsc::Sender<()>,
    }

    impl Live {
        fn start() -> Self {
            let (parked_tx, parked_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let release = Arc::new(Mutex::new(release_rx));
            let hooks = EngineHooks {
                action_gate: Some(Arc::new(move |_ctx: HookCtx| {
                    Box::new(ParkGate {
                        parked: parked_tx.clone(),
                        release: Arc::clone(&release),
                    }) as Box<dyn ActionGate>
                })),
                ..EngineHooks::default()
            };
            let base = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tl-collect/tests/fixtures/minimal-vm");
            let config = EngineConfig::new(EngineMode::LowMemory, base.join("state"))
                .with_sys_root(SysRoot::at(&base));
            let handle = Engine::start(config, hooks).expect("engine starts");
            Self {
                handle,
                parked_rx,
                release_tx,
            }
        }

        /// Sends an action and waits until the reducer is parked in its `submit`.
        fn park(&self) {
            let park = Command::Action(ActionRequest {
                id: ActionId(1),
                kind: ActionKind::Signal(Signal::Term),
                targets: SmallVec::new(),
                privileged: false,
            });
            assert_eq!(self.handle.send(park), Ok(()));
            self.parked_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("the reducer parks");
        }

        fn release(&self) {
            let _ = self.release_tx.send(());
        }

        /// Fills the command queue until it refuses a command.
        fn fill_queue(&self) {
            let mut sent = 0;
            while self.handle.send(Command::ResetBaselines).is_ok() {
                sent += 1;
                assert!(sent <= QUEUE, "the queue never filled");
            }
        }

        fn shutdown(self) {
            self.release();
            let report = self.handle.shutdown();
            assert!(report.clean(), "{report:?}");
        }
    }

    /// Waits up to 5 s for `done`.
    fn within_5s(done: impl Fn() -> bool) -> bool {
        let begin = Instant::now();
        while !done() {
            if begin.elapsed() > Duration::from_secs(5) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    #[test]
    fn open_session_serves_eight_ports_beside_the_local_session() {
        let live = Live::start();
        let mut ports: Vec<SessionPort> = (1..SESSIONS_MAX as u32)
            .map(|i| live.handle.open_session(ClientSessionId(i)).expect("room"))
            .collect();
        assert_eq!(ports.len(), 8);
        assert_eq!(
            live.handle.open_session(ClientSessionId(99)).err(),
            Some(OpenError::Full)
        );
        assert_eq!(
            live.handle.open_session(ClientSessionId(1)).err(),
            Some(OpenError::Duplicate)
        );
        assert_eq!(
            live.handle.open_session(ClientSessionId::LOCAL).err(),
            Some(OpenError::Duplicate)
        );

        // A dropped port keeps its slot until the reducer has ended its session.
        live.park();
        let first = Arc::clone(&ports[0].state);
        drop(ports.remove(0));
        assert!(first.is_closed());
        assert!(!first.is_ended());
        assert_eq!(
            live.handle.open_session(ClientSessionId(99)).err(),
            Some(OpenError::Full)
        );
        live.release();
        assert!(
            within_5s(|| first.is_ended()),
            "the reducer ends the session"
        );
        let reopened = live
            .handle
            .open_session(ClientSessionId(99))
            .expect("the ended session's slot is free");
        drop(reopened);
        drop(ports);
        live.shutdown();
    }

    #[test]
    fn a_port_dropped_on_a_full_queue_still_closes() {
        let live = Live::start();
        let port = live.handle.open_session(ClientSessionId(1)).expect("room");
        let state = Arc::clone(&port.state);
        live.park();
        live.fill_queue();
        assert_eq!(
            live.handle.open_session(ClientSessionId(2)).err(),
            Some(OpenError::Busy),
            "a full queue refuses an open and keeps no slot"
        );
        assert_eq!(port.send(Command::ResetBaselines), Err(CommandError::Busy));
        // The demand is not queued, so a full queue does not lose it.
        port.set_demand(Demand {
            memory: true,
            ..Demand::default()
        });
        // The drop's wake does not fit in the queue; the close is in the shared state.
        drop(port);
        assert!(state.is_closed(), "the close is in the shared state");
        live.release();
        assert!(
            within_5s(|| state.is_ended()),
            "the reducer reads the close with no message for it"
        );
        let again = live
            .handle
            .open_session(ClientSessionId(2))
            .expect("the refused open kept no slot");
        drop(again);
        live.shutdown();
    }

    #[test]
    fn open_session_until_waits_no_longer_than_its_deadline() {
        let live = Live::start();
        live.park();
        let begin = Instant::now();
        assert_eq!(
            live.handle
                .open_session_until(ClientSessionId(1), begin + Duration::from_millis(300))
                .err(),
            Some(OpenError::Busy),
            "a parked reducer does not answer by the deadline"
        );
        let waited = begin.elapsed();
        assert!(
            waited >= Duration::from_millis(300) && waited < OPEN_WAIT,
            "waited {waited:?}"
        );
        assert_eq!(
            live.handle
                .open_session_until(ClientSessionId(2), Instant::now())
                .err(),
            Some(OpenError::Busy),
            "a passed deadline sends no open"
        );
        live.release();
        // The withdrawn open holds its id until the reducer has read the drop.
        assert!(
            within_5s(|| live.handle.open_session(ClientSessionId(1)).is_ok()),
            "the withdrawn open frees its id"
        );
        live.shutdown();
    }

    #[test]
    fn open_session_after_stop() {
        let r = rig(None);
        r.handle.inner.stopped.store(true, Ordering::Release);
        assert_eq!(
            r.handle.open_session(ClientSessionId(1)).err(),
            Some(OpenError::Stopped)
        );
    }

    #[test]
    fn port_gets_snapshots_and_its_drop_frees_the_slot() {
        let base =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/minimal-vm");
        let config = EngineConfig::new(EngineMode::LowMemory, base.join("state"))
            .with_sys_root(SysRoot::at(&base));
        let handle = Engine::start(config, EngineHooks::default()).expect("engine starts");
        let port = handle.open_session(ClientSessionId(1)).expect("room");
        let wake = port.wake();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(futures_lite::future::block_on(wake.recv()));
        });
        let woke = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("wake within 5 s");
        assert_eq!(woke, Ok(()));
        assert!(port.latest().sequence >= 1);

        let others: Vec<SessionPort> = (2..SESSIONS_MAX as u32)
            .map(|i| handle.open_session(ClientSessionId(i)).expect("room"))
            .collect();
        assert_eq!(
            handle.open_session(ClientSessionId(99)).err(),
            Some(OpenError::Full)
        );
        drop(port);
        let begin = Instant::now();
        let reopened = loop {
            match handle.open_session(ClientSessionId(99)) {
                Ok(port) => break port,
                Err(OpenError::Full) if begin.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("the dropped port's slot did not free: {e}"),
            }
        };
        assert!(!reopened.is_ended());
        drop(others);
        drop(reopened);
        let _ = handle.shutdown();
    }

    #[test]
    fn missing_boot_id_reads_zero() {
        assert_eq!(
            read_boot_id(&SysRoot::at(Path::new("/nonexistent"))),
            BootId(0)
        );
    }

    fn peer(pid: u32, bridge: bool) -> SessionPeer {
        SessionPeer {
            pid,
            app_id: Arc::from(APP_ID),
            bridge,
        }
    }

    /// q4x76.12: keep above and frame geometry only from a window on this host, for its own
    /// pid and app id. Other KWin commands pass from any session.
    #[test]
    fn own_window_commands_only_from_the_window_itself() {
        use tl_model::snapshot::FrameGeometry;

        let keep = |pid| KWinCommand::KeepAbove {
            pid,
            caption: None,
            on: true,
        };
        let place = |pid| KWinCommand::SetFrameGeometry {
            pid,
            caption: Arc::from("tasklens Graph 1"),
            geometry: FrameGeometry::default(),
        };
        let local = peer(4000, false);
        assert!(own_window_allowed(Some(&local), &keep(4000)));
        assert!(own_window_allowed(Some(&local), &place(4000)));
        assert!(
            !own_window_allowed(Some(&local), &keep(4001)),
            "another pid"
        );
        assert!(
            !own_window_allowed(Some(&peer(4000, true)), &keep(4000)),
            "a bridge session"
        );
        assert!(!own_window_allowed(None, &keep(4000)), "no Hello peer");
        let other_app = SessionPeer {
            app_id: Arc::from("org.example.Other"),
            ..local.clone()
        };
        assert!(
            !own_window_allowed(Some(&other_app), &keep(4000)),
            "another app id"
        );
        let activate = KWinCommand::Activate {
            internal_id: Arc::from("{0a1b2c3d-0000-4000-8000-00000000000a}"),
        };
        assert!(own_window_allowed(Some(&peer(4000, true)), &activate));
        assert!(own_window_allowed(None, &KWinCommand::Pick { seq: 1 }));
    }

    /// q4x76.12: a refused own-window command is dropped and never reaches the reducer.
    #[test]
    fn refused_own_window_command_is_not_queued() {
        let mut rig = rig(None);
        let port = rig
            .handle
            .send_open(ClientSessionId(1), Some(peer(4000, true)))
            .expect("open sent");
        assert!(matches!(
            rig.msgs_rx.try_recv(),
            Ok(EngineMsg::Client(ClientMsg::OpenPort { .. }))
        ));
        let keep = Command::KWin(KWinCommand::KeepAbove {
            pid: 4000,
            caption: None,
            on: true,
        });
        assert_eq!(port.send(keep), Ok(()));
        assert!(rig.msgs_rx.try_recv().is_err(), "nothing queued");
    }

    /// q4x76.12: a window on this host is "self" while its port lives; a bridge session's pid
    /// names a process on another host and is never added.
    #[test]
    fn local_window_pids_follow_their_ports() {
        let rig = rig(None);
        let desktop = rig.handle.inner.desktop.clone();
        let local = rig
            .handle
            .send_open(ClientSessionId(1), Some(peer(4000, false)))
            .expect("open sent");
        let remote = rig
            .handle
            .send_open(ClientSessionId(2), Some(peer(5000, true)))
            .expect("open sent");
        let unknown = rig
            .handle
            .send_open(ClientSessionId(3), None)
            .expect("open sent");
        assert_eq!(desktop.window_pids(), [4000]);
        drop(local);
        assert!(desktop.window_pids().is_empty());
        drop((remote, unknown));
    }
}
