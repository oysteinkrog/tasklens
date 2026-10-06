//! `EngineHandle`, the UI's only view of the engine, and the `EngineHooks` later epics fill in
//! (ADR-005, ADR-001 rule 2).
//!
//! No public signature here names an asupersync type (D-001). The one exception is `HookCtx`,
//! which is `#[doc(hidden)]` and used only inside the crate and by E11's gate factory, and
//! `Engine::start_with_workers`, which is `#[doc(hidden)]` and used only by tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
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
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{ComponentHeader, Snapshot, WindowsSnapshot};
use tl_model::time::BootId;
use tl_model::validity::Validity;

use crate::EngineError;
use crate::config::EngineConfig;
use crate::dbus::SharedHandles;
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

/// Opens one journal stream. Installed by E14-A4 through `EngineHooks`.
pub type JournalSpawner = Arc<dyn Fn(JournalQuery) -> JournalHandle + Send + Sync>;

/// Builds the action gate. Called once per reducer start.
pub type ActionGateFactory = Arc<dyn Fn(HookCtx) -> Box<dyn ActionGate> + Send + Sync>;

/// Builds the record sink from the engine config. Called once per reducer start.
pub type RecordSinkFactory = Arc<dyn Fn(&EngineConfig) -> Box<dyn RecordSink> + Send + Sync>;

/// Optional parts that later epics install: the action gate (E11-A1), the journal (E14-A4) and
/// the record sink (E30-A2). `Default` is `crate::default_hooks()`.
#[derive(Clone)]
pub struct EngineHooks {
    pub action_gate: Option<ActionGateFactory>,
    pub record_sink: Option<RecordSinkFactory>,
    pub journal: Option<JournalSpawner>,
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
    /// Spawns the default journal reader in `EngineClient::journal`. Dropped with `Inner`, so a
    /// live `EngineHandle` clone keeps the runtime alive.
    rt_handle: Mutex<Option<RuntimeHandle>>,
    stopped: AtomicBool,
}

/// The UI-side channel ends that `shutdown` drops first.
struct Links {
    events_rx: async_channel::Receiver<ActionEvent>,
    demand_tx: watch::Sender<Demand>,
    settings_tx: watch::Sender<EngineSettings>,
    msgs_tx: mpsc::Sender<EngineMsg>,
}

impl EngineHandle {
    /// Builds a handle from the channel ends `Engine::start` creates. The runtime slots start
    /// empty; the last demand starts as the demand channel's current value.
    pub(crate) fn new(
        latest: Arc<ArcSwap<Snapshot>>,
        wake_rx: async_channel::Receiver<()>,
        events_rx: async_channel::Receiver<ActionEvent>,
        demand_tx: watch::Sender<Demand>,
        settings_tx: watch::Sender<EngineSettings>,
        msgs_tx: mpsc::Sender<EngineMsg>,
        journal: Option<JournalSpawner>,
    ) -> Self {
        let last_demand = Mutex::new(demand_tx.borrow().clone());
        Self {
            inner: Arc::new(Inner {
                latest,
                wake_rx,
                links: Mutex::new(Some(Links {
                    events_rx,
                    demand_tx,
                    settings_tx,
                    msgs_tx,
                })),
                last_demand,
                journal,
                rt: Mutex::new(None),
                rt_handle: Mutex::new(None),
                stopped: AtomicBool::new(false),
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
    /// thread after GPUI has stopped (ADR-010), and drop every other `EngineHandle` clone first:
    /// a live clone keeps the runtime alive, so teardown cannot finish and `torn_down` is false.
    ///
    /// `total` is advisory. `DRAIN_BUDGET` and `TEARDOWN_BUDGET` are the real bounds, 1.5 s
    /// together. A blocking job that is still running is not interrupted; teardown then goes on
    /// in the background and `torn_down` is false.
    pub fn shutdown(self, total: Duration) -> ShutdownReport {
        let begin = Instant::now();
        let inner = self.inner;
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
        // Step 3: drop `rt_handle`. This is the last owner of `Inner` unless the caller kept a
        // clone.
        drop(inner);
        // Step 4.
        let torn_down = rt.shutdown_timeout(TEARDOWN_BUDGET);
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
                total_ms = total.as_millis() as u64,
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
        let rt = crate::runtime::build_runtime()
            .map_err(|e| EngineError::Runtime(e.to_string().into()))?;
        let boot_id = read_boot_id(&config.sys_root);

        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let (events_tx, events_rx) = async_channel::bounded::<ActionEvent>(EVENTS_CAPACITY);
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
            config: Arc::new(config),
            boot_id,
            hooks,
            workers,
            next_sequence: 0,
        };
        let rt_handle = rt.handle();
        rt_handle
            .try_spawn_with_cx(move |cx| engine_root(cx, ep))
            .map_err(|e| EngineError::Spawn(e.to_string().into()))?;

        let handle = EngineHandle::new(
            latest,
            wake_rx,
            events_rx,
            demand_tx,
            settings_tx,
            msgs_tx,
            journal,
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
        let (demand_tx, demand_rx) = watch::channel(Demand::default());
        let (settings_tx, _settings_rx) = watch::channel(EngineSettings::default());
        let (msgs_tx, msgs_rx) = mpsc::channel(QUEUE);
        let handle = EngineHandle::new(
            latest,
            wake_rx,
            events_rx,
            demand_tx,
            settings_tx,
            msgs_tx,
            journal,
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
        assert!(
            begin.elapsed() < Duration::from_millis(100),
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
