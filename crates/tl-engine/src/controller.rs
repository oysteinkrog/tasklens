//! One `SourceController` per worker for the life of a session (ADR-004).
//!
//! The controller runs the worker's one-child supervisor in its own task. When the restart
//! budget is spent it publishes `Disabled`, sleeps `DISABLE_FOR`, binds a fresh supervisor and
//! tries again: five failures in a minute, then off for a minute.
//!
//! The supervisor runs inline with `ManagedSupervisor::run`, not through a spawned handle. Its
//! controller then watches this task's `Cx`, so cancelling the controller task drains the
//! running generation. Joining a spawned handle ignores cancellation.
//!
//! A restart reaches the reducer as `Failed` then `Running` (R-289): a generation that ends
//! with an error or a panic sends `Failed`, and the next generation sends `Running` first.
//!
//! Every restart, inside one supervisor or after `Disabled`, is counted in `RestartCounts`
//! before its `Running` is sent, so the health snapshot that shows it carries the new count.

use std::panic::{AssertUnwindSafe, resume_unwind};
use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::{ManagedGeneration, ManagedSupervisorReport};
use asupersync::time::sleep;
use asupersync::types::Outcome;
use futures_lite::FutureExt;
use tl_model::health::{SourceId, SourceState};
use tl_model::time::{TimeNs, now_boot_ns};

use crate::supervise::{RestartCounts, WorkerFactory, budget_exhausted, build_supervisor};
use crate::update::{EngineMsg, SourceUpdate, WorkerError};

/// How long a source stays off after it spent its restart budget.
pub const DISABLE_FOR: Duration = Duration::from_secs(60);

pub struct SourceController {
    id: SourceId,
    name: &'static str,
    factory: WorkerFactory,
    updates: mpsc::Sender<EngineMsg>,
    restarts: RestartCounts,
}

impl SourceController {
    pub fn new(id: SourceId, factory: WorkerFactory, updates: mpsc::Sender<EngineMsg>) -> Self {
        Self {
            id,
            name: source_name(id),
            factory,
            updates,
            restarts: RestartCounts::default(),
        }
    }

    /// Counts this source's restarts in `restarts`, the reducer's
    /// `HealthState::restart_counts`. Without it the counts go nowhere.
    pub fn with_restart_counts(mut self, restarts: RestartCounts) -> Self {
        self.restarts = restarts;
        self
    }

    /// Runs until `cx` is cancelled. Spawn with `cx.spawn(|cx| ctl.run(cx))` from the session.
    pub async fn run(self, cx: Cx) {
        let name = self.name;
        self.run_inner(cx).await;
        // Diagnostic for slow shutdowns: which worker ended, and how long after `shutdown` began.
        if let Some(since) = crate::bridge::since_shutdown_began() {
            tracing::debug!(
                target: "tasklens::engine",
                worker = name,
                since_shutdown_ms = since.as_millis() as u64,
                "worker exited during shutdown",
            );
        }
    }

    async fn run_inner(self, cx: Cx) {
        // True once a supervisor ran: the next pass restarts a worker that stopped.
        let mut restart = false;
        loop {
            if restart {
                self.restarts.bump(self.id);
            }
            if !self
                .health(&cx, SourceState::Running, None, now_boot_ns())
                .await
            {
                return;
            }
            let factory = report_restarts(
                self.id,
                self.factory.clone(),
                self.updates.clone(),
                self.restarts.clone(),
            );
            let report = match build_supervisor(self.name, factory) {
                Ok(supervisor) => {
                    restart = true;
                    supervisor.run(&cx).await
                }
                Err(error) => {
                    // A build error repeats on every try, so wait before the next one.
                    warn_source(self.id, &error.to_string());
                    sleep(cx.now(), DISABLE_FOR).await;
                    if cx.checkpoint().is_err() {
                        return;
                    }
                    continue;
                }
            };
            if cx.is_cancel_requested() {
                return;
            }
            if budget_exhausted(&report) {
                let t_ns = now_boot_ns();
                let until = t_ns.saturating_add(DISABLE_FOR.as_nanos() as u64);
                let state = SourceState::Disabled { until };
                if !self
                    .health(&cx, state, Some(last_error(&report)), t_ns)
                    .await
                {
                    return;
                }
                sleep(cx.now(), DISABLE_FOR).await;
                if cx.checkpoint().is_err() {
                    return;
                }
                continue;
            }
            if finished_ok(&report) {
                // The worker reported `Unsupported` before it returned. Never restart it
                // (R-155 item 4).
                loop {
                    sleep(cx.now(), DISABLE_FOR).await;
                    if cx.checkpoint().is_err() {
                        return;
                    }
                }
            }
            warn_source(self.id, outcome_kind(&report));
        }
    }

    /// Sends one health message. Returns false when the loop must stop.
    async fn health(
        &self,
        cx: &Cx,
        state: SourceState,
        last_error: Option<Arc<str>>,
        t_ns: TimeNs,
    ) -> bool {
        let update = SourceUpdate::Health {
            id: self.id,
            state,
            last_error,
            t_ns,
        };
        self.updates
            .send(cx, EngineMsg::Source(update))
            .await
            .is_ok()
    }
}

/// Wraps `factory` so each restart is `Failed` then `Running` in the health stream (R-289).
/// Without this a restarted worker that sends no health of its own, or sends `Running` again,
/// shows no state change, and the reducer cannot gap its series.
fn report_restarts(
    id: SourceId,
    factory: WorkerFactory,
    updates: mpsc::Sender<EngineMsg>,
    restarts: RestartCounts,
) -> WorkerFactory {
    Arc::new(move |cx: Cx, generation: ManagedGeneration| {
        let run = factory.start(cx.clone(), generation);
        let updates = updates.clone();
        let restarts = restarts.clone();
        async move {
            if generation.number > 1 {
                restarts.bump(id);
                send_health(&cx, &updates, id, SourceState::Running, None).await;
            }
            match AssertUnwindSafe(run).catch_unwind().await {
                Ok(Outcome::Err(WorkerError::Closed)) => Outcome::Err(WorkerError::Closed),
                Ok(Outcome::Err(error)) => {
                    let text: Arc<str> = error.to_string().into();
                    send_health(&cx, &updates, id, SourceState::Failed, Some(text)).await;
                    Outcome::Err(error)
                }
                Ok(Outcome::Panicked(payload)) => {
                    send_health(&cx, &updates, id, SourceState::Failed, Some("panic".into())).await;
                    Outcome::Panicked(payload)
                }
                Ok(outcome) => outcome,
                Err(payload) => {
                    send_health(&cx, &updates, id, SourceState::Failed, Some("panic".into())).await;
                    resume_unwind(payload)
                }
            }
        }
    })
}

/// Sends one health message from a worker generation. A closed queue is ignored: the worker
/// is ending anyway.
async fn send_health(
    cx: &Cx,
    updates: &mpsc::Sender<EngineMsg>,
    id: SourceId,
    state: SourceState,
    last_error: Option<Arc<str>>,
) {
    let update = SourceUpdate::Health {
        id,
        state,
        last_error,
        t_ns: now_boot_ns(),
    };
    let _ = updates.send(cx, EngineMsg::Source(update)).await;
}

/// The last generation's error text, or "panic".
fn last_error(report: &ManagedSupervisorReport<WorkerError>) -> Arc<str> {
    match report.children.last() {
        Some(child) => match (&child.task_outcome, &child.outcome) {
            (Ok(()), Outcome::Err(error)) => error.to_string().into(),
            _ => "panic".into(),
        },
        None => "panic".into(),
    }
}

/// True when the last generation returned `Ok(())` on its own, not because of a cancel.
fn finished_ok(report: &ManagedSupervisorReport<WorkerError>) -> bool {
    matches!(report.outcome, Outcome::Ok(()))
        && report.children.last().is_some_and(|child| {
            !child.shutdown_requested_before_completion
                && child.task_outcome.is_ok()
                && matches!(child.outcome, Outcome::Ok(()))
        })
}

/// The supervisor's stop reason, for the log. Never includes error text.
fn outcome_kind(report: &ManagedSupervisorReport<WorkerError>) -> &'static str {
    let joined = report.children.last().map(|child| &child.task_outcome);
    match (&report.outcome, joined) {
        (Outcome::Err(_), _) => "supervisor ended with an error",
        (Outcome::Cancelled(_), _) => "supervisor was cancelled",
        (Outcome::Panicked(_), _) => "supervisor panicked",
        (Outcome::Ok(()), Some(Err(JoinError::Cancelled(_)))) => "worker was cancelled",
        (Outcome::Ok(()), _) => "supervisor ended",
    }
}

/// Maps every `SourceId` to its worker name and its log target `tasklens::source::<name>`.
/// A `tracing` target must be a constant, so each source gets its own `warn!` arm.
macro_rules! source_names {
    ($($id:ident => $name:literal,)*) => {
        fn source_name(id: SourceId) -> &'static str {
            match id {
                $(SourceId::$id => $name,)*
            }
        }

        fn warn_source(id: SourceId, why: &str) {
            match id {
                $(SourceId::$id => tracing::warn!(
                    target: concat!("tasklens::source::", $name),
                    "{why}; restarting the worker"
                ),)*
            }
        }
    };
}

source_names! {
    Cpu => "cpu",
    Memory => "memory",
    Disk => "disk",
    Net => "net",
    GpuNvidia => "gpu_nvidia",
    GpuDrm => "gpu_drm",
    Sensors => "sensors",
    Processes => "processes",
    ProcessDetail => "process_detail",
    Connections => "connections",
    SystemdSystem => "systemd_system",
    SystemdUser => "systemd_user",
    Logind => "logind",
    Startup => "startup",
    Inventory => "inventory",
    KWin => "kwin",
    Udisks => "udisks",
    UPower => "upower",
    NetworkManager => "network_manager",
    HistoryRecorder => "history_recorder",
    Journal => "journal",
    MetricHistory => "metric_history",
}
