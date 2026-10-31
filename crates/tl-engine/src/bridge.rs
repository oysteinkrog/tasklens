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
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::history::RecordSink;
use tl_model::journal::{JournalError, JournalEvent, JournalHandle, JournalQuery};
use tl_model::request::RequestEvent;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{ComponentHeader, Snapshot, WindowsSnapshot};
use tl_model::time::BootId;
use tl_model::validity::Validity;

use crate::EngineError;
use crate::client_session::{ClientMsg, ClientSessionId};
use crate::config::EngineConfig;
use crate::dbus::SharedHandles;
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
            }),
        }
    }

    fn links(&self) -> std::sync::MutexGuard<'_, Option<Links>> {
        self.inner
            .links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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

        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let (events_tx, events_rx) = async_channel::bounded::<ActionEvent>(EVENTS_CAPACITY);
        // Unbounded: an end is never dropped, and admission bounds how many are outstanding.
        let (requests_tx, requests_rx) = async_channel::unbounded::<RequestEvent>();
        let (msgs_tx, msgs_rx) = mpsc::channel::<EngineMsg>(MSGS_CAPACITY);
        let (demand_tx, demand_rx) = watch::channel(Demand::default());
        let (settings_tx, settings_rx) = watch::channel(EngineSettings::default());
        let (control_tx, _control_rx) = watch::channel(WorkerControl::default());
        let latest = Arc::new(ArcSwap::from_pointee(empty_snapshot(boot_id)));

        let journal = hooks.journal.clone();
        let ep = RootEndpoints {
            msgs: msgs_rx,
            msgs_tx: msgs_tx.clone(),
            demand: demand_rx,
            settings: settings_rx,
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
            let _ = links.demand_tx.send(demand.clone());
            // Wakes the reducer, so the workers get the new combined demand now and not at its
            // next message (ADR-018). A full queue means the reducer runs soon anyway.
            let _ = links.msgs_tx.try_send(EngineMsg::Client(ClientMsg::Demand {
                id: ClientSessionId::LOCAL,
                demand: Box::new(demand),
            }));
        }
    }

    fn set_settings(&self, settings: EngineSettings) {
        if let Some(links) = &*self.links() {
            let _ = links.settings_tx.send(settings);
        }
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

    use tl_model::table::{Manager, UnitRef};

    use crate::config::EngineMode;

    /// Command queue size from ADR-005.
    const QUEUE: usize = 256;

    struct Rig {
        handle: EngineHandle,
        demand_rx: watch::Receiver<Demand>,
        msgs_rx: mpsc::Receiver<EngineMsg>,
    }

    fn rig(journal: Option<JournalSpawner>) -> Rig {
        let latest = Arc::new(ArcSwap::from_pointee(Snapshot::empty(BootId(0), 0)));
        let (_wake_tx, wake_rx) = async_channel::bounded(1);
        let (_events_tx, events_rx) = async_channel::bounded(256);
        let (_requests_tx, requests_rx) = async_channel::unbounded();
        let (demand_tx, demand_rx) = watch::channel(Demand::default());
        let (settings_tx, _settings_rx) = watch::channel(EngineSettings::default());
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
        );
        Rig {
            handle,
            demand_rx,
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
        // One wake for the reducer, with the local session's demand.
        match r.msgs_rx.try_recv() {
            Ok(EngineMsg::Client(ClientMsg::Demand { id, demand })) => {
                assert_eq!(id, ClientSessionId::LOCAL);
                assert_eq!(*demand, d);
            }
            other => panic!("expected a local Demand, got {other:?}"),
        }
        assert!(r.msgs_rx.try_recv().is_err());
        // A demand equal to the initial value is not sent either.
        let r2 = rig(None);
        r2.handle.set_demand(Demand::default());
        assert!(!r2.demand_rx.has_changed());
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

    #[test]
    fn missing_boot_id_reads_zero() {
        assert_eq!(
            read_boot_id(&SysRoot::at(Path::new("/nonexistent"))),
            BootId(0)
        );
    }
}
