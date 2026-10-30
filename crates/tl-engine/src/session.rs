//! `engine_root` and `engine_session`: the engine's task tree (ADR-004).
//!
//! `engine_root` owns the UI-facing channel ends for the life of the process and runs sessions
//! in a loop. A session spawns one `SourceController` per worker and the 1 s `Tick` task, then
//! runs the reducer inline under `catch_unwind`. A reducer panic or `InvariantViolation` ends the
//! session; every child task is aborted and a new session starts 1 s later with the next epoch.
//! More than 3 restarts in 60 s stop the restarts: the root publishes an all-`Failed` health
//! snapshot and stays alive until it is cancelled.
//!
//! The control watch and the publish sequence live in `RootEndpoints`, so they carry over from
//! one session to the next. `WorkerControl.progress` therefore keeps the last started command of
//! every source, and a new session does not run a forwarded command twice (R-276).

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;
use asupersync::Cx;
use asupersync::channel::{mpsc, watch};
use asupersync::runtime::TaskHandle;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::sleep;
use futures_lite::FutureExt;
use futures_lite::future::poll_fn;
use tl_model::actions::ActionEvent;
use tl_model::demand::Demand;
use tl_model::health::SourceId;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::time::{BootId, now_boot_ns};

use crate::bridge::{EngineHooks, HookCtx};
use crate::client_session::RequestEvent;
use crate::config::{EngineConfig, EngineMode};
use crate::controller::SourceController;
use crate::dbus::SharedHandles;
use crate::helper::{HelperHandle, HelperMsg, helper_queue, helper_task_with};
use crate::reducer::health::HealthState;
use crate::reducer::{HelperCommand, Publisher, Reducer};
use crate::source::{WorkerControl, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::EngineMsg;
use crate::workers::WorkerSpec;
use crate::workers::history::{
    HistoryMsg, HistorySink, HistorySlot, HistoryWorkerConfig, run_history,
};
use crate::workers::metric_history::{
    MetricHistorySlot, MetricSink, MetricWorkerConfig, run_metric_history,
};

/// Wait between a failed session and the next one.
pub const RESTART_DELAY: Duration = Duration::from_secs(1);
/// Restarts allowed within `RESTART_WINDOW`. One more failure stops the engine.
pub const MAX_RESTARTS: u32 = 3;
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);
/// Period of the housekeeping `Tick` message.
pub const TICK: Duration = Duration::from_secs(1);

/// Capacity of the reducer's helper command queue (R-148 item 1).
const HELPER_COMMANDS: usize = 64;

/// The `last_error` of every source after the restarts stop.
pub const RESTARTED_TOO_OFTEN: &str = "engine restarted too often";

/// Everything the root owns for the life of the process. `Engine::start` builds it.
pub struct RootEndpoints {
    /// UI commands and worker updates share this queue (capacity 256).
    pub msgs: mpsc::Receiver<EngineMsg>,
    /// Cloned into every worker's `WorkerIo` and into the tick task.
    pub msgs_tx: mpsc::Sender<EngineMsg>,
    pub demand: watch::Receiver<Demand>,
    pub settings: watch::Receiver<EngineSettings>,
    /// Lent to the reducer for each session and taken back when the session ends.
    pub control: watch::Sender<WorkerControl>,
    pub publisher: Publisher,
    pub events_tx: async_channel::Sender<ActionEvent>,
    /// The local session's request ends (ADR-018). Every reducer sends on it.
    pub requests_tx: async_channel::Sender<RequestEvent>,
    pub config: Arc<EngineConfig>,
    pub boot_id: BootId,
    pub hooks: EngineHooks,
    /// The workers every session starts. `Engine::start` passes `workers_for(config.mode)`;
    /// tests pass a fake set.
    pub workers: Vec<WorkerSpec>,
    /// The last published sequence. Each session starts its reducer here, so the sequence
    /// grows across epochs.
    pub next_sequence: u64,
    /// The last engine-wide request id. Each reducer continues after it, so an id from a
    /// stopped reducer that is still in the worker queue never names a newer request.
    pub last_engine_id: u64,
}

/// Why a session ended other than by cancel.
#[derive(Debug)]
enum SessionEnd {
    Panic,
    Invariant(&'static str),
    /// A child task could not be spawned.
    Spawn,
}

impl SessionEnd {
    /// Never includes the panic payload.
    fn reason(&self) -> &'static str {
        match self {
            SessionEnd::Panic => "reducer panicked",
            SessionEnd::Invariant(what) => what,
            SessionEnd::Spawn => "spawn failed",
        }
    }
}

/// The session's child tasks. `TaskHandle` does not cancel on drop, so this aborts every task
/// when it is dropped, on return and on unwind alike.
struct Children(Vec<TaskHandle<()>>);

impl Drop for Children {
    fn drop(&mut self) {
        for handle in &self.0 {
            handle.abort();
        }
    }
}

/// Runs sessions until one ends by cancel, or until the restarts stop and the root is
/// cancelled. Spawn it with `try_spawn_with_cx` so shutdown cancellation reaches it.
pub async fn engine_root(cx: Cx, mut ep: RootEndpoints) {
    let window = RESTART_WINDOW.as_nanos() as u64;
    let mut epoch = 1;
    let mut failures: Vec<u64> = Vec::new();
    loop {
        let Err(end) = engine_session(&cx, &mut ep, epoch).await else {
            return;
        };
        tracing::error!(target: "tasklens::engine", reason = end.reason(), epoch, "session ended");
        let now = cx.now().as_nanos();
        failures.retain(|&t| now.saturating_sub(t) < window);
        failures.push(now);
        if failures.len() > MAX_RESTARTS as usize {
            tracing::error!(target: "tasklens::engine", epoch, "{RESTARTED_TOO_OFTEN}; not restarting");
            publish_all_failed(&mut ep, epoch);
            // Stay alive so the UI keeps its channels until shutdown.
            loop {
                sleep(cx.now(), Duration::from_secs(60)).await;
                if cx.checkpoint().is_err() {
                    return;
                }
            }
        }
        sleep(cx.now(), RESTART_DELAY).await;
        if cx.checkpoint().is_err() {
            return;
        }
        epoch += 1;
    }
}

/// One session. `Ok` when the reducer stops on cancel, a closed queue or a gone UI; `Err` on a
/// panic or an invariant violation. Every child task is aborted before it returns.
async fn engine_session(cx: &Cx, ep: &mut RootEndpoints, epoch: u32) -> Result<(), SessionEnd> {
    // `watch::Sender` is not `Clone`, so the reducer borrows the root's sender for the session.
    let control = std::mem::replace(&mut ep.control, placeholder_control());
    let mut reducer = Reducer::new(epoch, ep.boot_id, control, ep.events_tx.clone());
    reducer.clients.set_local_requests(ep.requests_tx.clone());
    reducer.clients.resume_engine_ids(ep.last_engine_id);
    let result = run_session(cx, ep, &mut reducer, epoch).await;
    // Every request still active ends here as `Lost`: the local session outlives this reducer.
    reducer.stopped();
    ep.control = std::mem::replace(&mut reducer.control, placeholder_control());
    ep.next_sequence = reducer.sequence;
    ep.last_engine_id = reducer.clients.last_engine_id();
    result
}

/// A sender with no receivers, held in place of a lent sender.
fn placeholder_control() -> watch::Sender<WorkerControl> {
    watch::channel(WorkerControl::default()).0
}

async fn run_session(
    cx: &Cx,
    ep: &mut RootEndpoints,
    reducer: &mut Reducer,
    epoch: u32,
) -> Result<(), SessionEnd> {
    reducer.sequence = ep.next_sequence;
    reducer.settings_rx = Some(ep.settings.clone());
    reducer.demand_rx = Some(ep.demand.clone());
    if epoch > 1 {
        // The rings are new, but the charts must not join the old epoch's line to this one.
        reducer.series.mark_gap_all();
    }
    // E29-020: helper spawn
    let mut children = Children(Vec::with_capacity(ep.workers.len() + 4));
    let helper = match ep.hooks.helper.as_ref() {
        Some(make) => {
            let (handle, queue) = helper_queue();
            let (env, updates) = (make(), ep.msgs_tx.clone());
            children.0.push(spawn(cx, move |cx| async move {
                let _running = crate::bridge::task_running("helper");
                helper_task_with(cx, queue, updates, env).await;
            })?);
            let (tx, rx) = mpsc::channel::<HelperCommand>(HELPER_COMMANDS);
            reducer.helper_tx = Some(tx);
            let forward = handle.clone();
            children.0.push(spawn(cx, move |cx| async move {
                let _running = crate::bridge::task_running("forward_helper");
                forward_helper(cx, rx, forward).await;
            })?);
            Some(handle)
        }
        None => None,
    };
    let shared = SharedHandles {
        helper,
        // The low-memory engine has no detail panels and no P2 reads (R-152).
        pkg: (ep.config.mode == EngineMode::Full)
            .then(|| ep.config.pkg_backend.index())
            .flatten(),
        ..SharedHandles::default()
    };
    reducer.action_gate = ep.hooks.action_gate.as_ref().map(|make| {
        make(HookCtx {
            events_tx: ep.events_tx.clone(),
            msgs_tx: ep.msgs_tx.clone(),
            boot_id: ep.boot_id,
            config: ep.config.clone(),
            shared: shared.clone(),
        })
    });
    if let Some(gate) = reducer.action_gate.as_ref() {
        reducer.health.set_caps(gate.caps());
    }
    reducer.record_sink = ep.hooks.record_sink.as_ref().map(|make| make(&ep.config));
    // E30-A2: history sink and worker. A test sink from the hooks replaces both. The low-memory
    // engine runs no history (R-152).
    let history_slot: Option<HistorySlot> = (reducer.record_sink.is_none()
        && ep.config.mode == EngineMode::Full)
        .then(|| Arc::new(ArcSwapOption::empty()));

    for spec in &ep.workers {
        let io = WorkerIo::new(
            ep.msgs_tx.clone(),
            ep.demand.clone(),
            ep.settings.clone(),
            reducer.control.subscribe(),
            shared.clone(),
        );
        let ctl = SourceController::new(
            spec.id,
            (spec.factory)(ep.config.clone(), io),
            ep.msgs_tx.clone(),
        )
        .with_restart_counts(reducer.health.restart_counts());
        children.0.push(spawn(cx, move |cx| ctl.run(cx))?);
    }
    if let Some(slot) = history_slot {
        let io = WorkerIo::new(
            ep.msgs_tx.clone(),
            ep.demand.clone(),
            ep.settings.clone(),
            reducer.control.subscribe(),
            shared.clone(),
        );
        let ctl = SourceController::new(
            SourceId::HistoryRecorder,
            history_factory(&ep.config, ep.boot_id, epoch, io, slot.clone()),
            ep.msgs_tx.clone(),
        )
        .with_restart_counts(reducer.health.restart_counts());
        children.0.push(spawn(cx, move |cx| ctl.run(cx))?);
        reducer.record_sink = Some(Box::new(HistorySink::new(slot.clone())));
        reducer.history_cmd = Some(HistorySink::new(slot));
    }
    // ADR-017: device metric history. A test sink from the hooks turns it off, as it does the
    // history worker. The low-memory engine runs none (R-152).
    if ep.hooks.record_sink.is_none() && ep.config.mode == EngineMode::Full {
        let slot: MetricHistorySlot = Arc::new(ArcSwapOption::empty());
        let io = WorkerIo::new(
            ep.msgs_tx.clone(),
            ep.demand.clone(),
            ep.settings.clone(),
            reducer.control.subscribe(),
            shared.clone(),
        );
        let ctl = SourceController::new(
            SourceId::MetricHistory,
            metric_history_factory(&ep.config, ep.boot_id, io, slot.clone()),
            ep.msgs_tx.clone(),
        )
        .with_restart_counts(reducer.health.restart_counts());
        children.0.push(spawn(cx, move |cx| ctl.run(cx))?);
        reducer.metric_sink = Some(MetricSink::new(slot));
    }
    let tick_tx = ep.msgs_tx.clone();
    children.0.push(spawn(cx, move |cx| async move {
        let _running = crate::bridge::task_running("tick");
        tick(cx, tick_tx).await;
    })?);

    let running = crate::bridge::task_running("reducer");
    let run = AssertUnwindSafe(reducer.run(cx, &mut ep.msgs, &mut ep.publisher))
        .catch_unwind()
        .await;
    drop(running);
    drop(children);
    match run {
        Ok(Ok(())) => Ok(()),
        Ok(Err(violation)) => Err(SessionEnd::Invariant(violation.0)),
        Err(_payload) => Err(SessionEnd::Panic),
    }
}

/// The `HistoryRecorder` worker. Each generation makes its own channel and stores the sender
/// in `slot` on its first poll (ADR-004 factory contract). `SessionStart` goes into that
/// channel right after the first poll, so it is never dropped for want of a sender, and every
/// generation starts its data with a gap.
fn history_factory(
    config: &EngineConfig,
    boot: BootId,
    epoch: u32,
    io: WorkerIo,
    slot: HistorySlot,
) -> WorkerFactory {
    let history_dir = config.history_dir.clone();
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let cfg = HistoryWorkerConfig {
            boot,
            history_dir: history_dir.clone(),
            load_delay: None,
        };
        let mut run = Box::pin(run_history(cx, io.clone(), slot.clone(), cfg));
        let slot = slot.clone();
        let mut started = false;
        poll_fn(move |task| {
            let poll = run.as_mut().poll(task);
            if !started {
                started = true;
                let start = HistoryMsg::SessionStart {
                    epoch,
                    t_ns: now_boot_ns(),
                };
                if let Some(tx) = slot.load().as_deref()
                    && tx.try_send(start).is_ok()
                {
                    task.waker().wake_by_ref();
                }
            }
            poll
        })
    })
}

/// The `MetricHistory` worker. Each generation makes its own channel and stores its sender in
/// `slot` while its store is open (ADR-017).
fn metric_history_factory(
    config: &EngineConfig,
    boot: BootId,
    io: WorkerIo,
    slot: MetricHistorySlot,
) -> WorkerFactory {
    let root = config.state_dir.join("metrics");
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let cfg = MetricWorkerConfig {
            boot,
            root: root.clone(),
        };
        run_metric_history(cx, io.clone(), slot.clone(), cfg)
    })
}

/// Spawns a child in the root's region. A failed spawn ends the session.
fn spawn<F, Fut>(cx: &Cx, f: F) -> Result<TaskHandle<()>, SessionEnd>
where
    F: FnOnce(Cx) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    cx.spawn(f).map_err(|error| {
        tracing::error!(target: "tasklens::engine", %error, "session child did not spawn");
        SessionEnd::Spawn
    })
}

/// Turns each reducer `HelperCommand` into the same-named `HelperMsg` on the helper queue
/// (R-247). Ends when either queue closes or on cancel.
async fn forward_helper(cx: Cx, mut rx: mpsc::Receiver<HelperCommand>, helper: HelperHandle) {
    while let Ok(cmd) = rx.recv(&cx).await {
        let msg = match cmd {
            HelperCommand::Start => HelperMsg::Start,
            HelperCommand::End => HelperMsg::End,
            HelperCommand::SetupStatus => HelperMsg::SetupStatus,
            HelperCommand::RunSetup(step) => HelperMsg::RunSetup(step),
        };
        if helper.send(&cx, msg).await.is_err() {
            return;
        }
    }
}

/// Sends `EngineMsg::Tick` every `TICK` until cancelled or the queue closes.
async fn tick(cx: Cx, msgs_tx: mpsc::Sender<EngineMsg>) {
    loop {
        sleep(cx.now(), TICK).await;
        if cx.checkpoint().is_err() {
            return;
        }
        if msgs_tx.send(&cx, EngineMsg::Tick).await.is_err() {
            return;
        }
    }
}

/// Publishes the last snapshot again with every source `Failed` and the next sequence.
fn publish_all_failed(ep: &mut RootEndpoints, epoch: u32) {
    let mut next = Snapshot::clone(&ep.publisher.latest.load());
    ep.next_sequence += 1;
    next.sequence = ep.next_sequence;
    next.epoch = epoch;
    next.published_ns = now_boot_ns();
    next.health = HealthState::all_failed(epoch, RESTARTED_TOO_OFTEN);
    ep.publisher.publish(next);
}
