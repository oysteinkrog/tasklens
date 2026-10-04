//! One `ManagedSupervisor` per worker (ADR-004).
//!
//! The restart budget is shared by the whole supervisor, so each worker gets
//! its own one-child supervisor. A crash-looping source then cannot spend
//! another source's budget. The policy is fixed: one-for-one, `Transient`,
//! backoff 500 ms doubling to 30 s, 5 restarts per 60 s, escalation `Stop`.
//!
//! `ChildSpec::depends_on` is never used.

use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::cx::Scope;
use asupersync::runtime::{JoinError, RuntimeState, SpawnError};
use asupersync::supervision::{
    BackoffStrategy, ChildSpec, EscalationPolicy, ManagedChildBinding, ManagedChildFactory,
    ManagedGeneration, ManagedRestartMode, ManagedSupervisor, ManagedSupervisorReport,
    RestartPolicy, SupervisionConfig, SupervisorBuilder,
};
use asupersync::types::policy::FailFast;
use asupersync::types::{Outcome, TaskId};

use crate::update::WorkerError;

pub const MAX_RESTARTS: u32 = 5;
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);
pub const BACKOFF_INITIAL: Duration = Duration::from_millis(500);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);
pub const BACKOFF_MULTIPLIER: f64 = 2.0;

/// Any `Fn(Cx, ManagedGeneration) -> Fut` with `Fut: Future<Output = Outcome<(), WorkerError>> + Send`.
pub type WorkerFactory = Arc<dyn ManagedChildFactory<WorkerError>>;

#[derive(Debug, thiserror::Error)]
pub enum SuperviseError {
    #[error("compile: {0}")]
    Compile(Arc<str>),
    #[error("bind: {0}")]
    Bind(Arc<str>),
    #[error("spawn: {0}")]
    Spawn(Arc<str>),
}

/// The legacy start that `ChildSpec::new` requires. Managed mode never calls it.
fn legacy_unreachable(
    _: &Scope<'static, FailFast>,
    _: &mut RuntimeState,
    _: &Cx,
) -> Result<TaskId, SpawnError> {
    unreachable!("managed mode never calls the legacy start")
}

/// Builds the one-child supervisor for the worker `name`.
pub fn build_supervisor(
    name: &str,
    factory: WorkerFactory,
) -> Result<ManagedSupervisor<WorkerError>, SuperviseError> {
    let compiled = SupervisorBuilder::new(name)
        .with_restart_policy(RestartPolicy::OneForOne)
        .child(ChildSpec::new(name, legacy_unreachable))
        .compile()
        .map_err(|error| SuperviseError::Compile(error.to_string().into()))?;
    let config = SupervisionConfig::new(MAX_RESTARTS, RESTART_WINDOW)
        .with_restart_policy(RestartPolicy::OneForOne)
        .with_backoff(BackoffStrategy::Exponential {
            initial: BACKOFF_INITIAL,
            max: BACKOFF_MAX,
            multiplier: BACKOFF_MULTIPLIER,
        })
        .with_escalation(EscalationPolicy::Stop);
    // `ManagedChildBinding::new` takes the factory by value, and an
    // `Arc<dyn ManagedChildFactory>` is not one itself, so a closure forwards to it.
    let start = move |cx: Cx, generation: ManagedGeneration| factory.start(cx, generation);
    compiled
        .bind_managed(
            vec![ManagedChildBinding::new(
                name,
                ManagedRestartMode::Transient,
                start,
            )],
            config,
        )
        .map_err(|error| SuperviseError::Bind(format!("{error:?}").into()))
}

/// Builds the supervisor, spawns it in the caller's region and waits for its report.
///
/// A controller task that ends without a report maps to `SuperviseError::Spawn`.
pub async fn run_supervised(
    cx: &Cx,
    name: &str,
    factory: WorkerFactory,
) -> Result<ManagedSupervisorReport<WorkerError>, SuperviseError> {
    let supervisor = build_supervisor(name, factory)?;
    let mut handle = supervisor
        .spawn(cx)
        .map_err(|error| SuperviseError::Spawn(error.to_string().into()))?;
    handle
        .join()
        .await
        .map_err(|error: JoinError| SuperviseError::Spawn(error.to_string().into()))
}

/// True when the child is down because the 5-in-60 s restart budget was spent.
///
/// With `EscalationPolicy::Stop` the controller leaves a refused child down and
/// still ends with `Outcome::Ok`. The child's last generation then ended in a
/// way `Transient` would restart: an error or a panic, not a cancel.
pub fn budget_exhausted(report: &ManagedSupervisorReport<WorkerError>) -> bool {
    if !matches!(report.outcome, Outcome::Ok(())) {
        return false;
    }
    if report.restart_batches < u64::from(MAX_RESTARTS) {
        return false;
    }
    report.children.iter().any(|child| {
        !child.shutdown_requested_before_completion
            && (matches!(child.outcome, Outcome::Panicked(_))
                || matches!(child.task_outcome, Err(JoinError::Panicked(_)))
                || (child.task_outcome.is_ok() && matches!(child.outcome, Outcome::Err(_))))
    })
}
