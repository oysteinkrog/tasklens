//! Builds the asupersync runtime with the ADR-004 settings.
//!
//! The blocking pool is off by default in asupersync 0.5.0. Without it,
//! `Cx::spawn_blocking` runs the closure inline on the async worker, so
//! `build_runtime` always sets it.
//!
//! Never call `RuntimeBuilder::current_thread()`. Never call
//! `RuntimeHandle::spawn_blocking`. Actions, the launcher, the opener and the
//! journal use `Cx::spawn_blocking` and join with `TaskHandle::join`. Periodic
//! samples do not use the pool: each source has its own sampling thread, so a
//! hung sample stalls only its own source (ADR-004, tasklens-wlf.30).
//!
//! Banned asupersync APIs in tl-engine production code (ADR-004): `AppSpec`,
//! `GenServer`, `actor`, `race!`, `select!`, `race_drained`,
//! `Cx::race_timeout`, `asupersync::signal`, `ctrl_c`, `asupersync::process`,
//! `asupersync::fs`, `Cx::for_request`, `Cx::for_testing`,
//! `ChildSpec::depends_on`, `JoinHandle` from `RuntimeHandle::spawn` (use
//! `spawn_checked`), and `Budget::with_poll_quota` on long-lived tasks.

use std::sync::Arc;

use asupersync::runtime::{Runtime, RuntimeBuilder};

use crate::bridge::ShutdownTrace;

/// One async worker thread.
pub const WORKER_THREADS: usize = 1;
/// Blocking threads kept alive. Idle threads above this retire after 10 s.
pub const BLOCKING_THREADS_MIN: usize = 1;
/// At most 4 blocking jobs run at once. Periodic samples run on their own threads, outside
/// this limit.
pub const BLOCKING_THREADS_MAX: usize = 4;
pub const GLOBAL_QUEUE_LIMIT: usize = 256;
pub const THREAD_NAME_PREFIX: &str = "tl-engine";

/// Builds the engine runtime. `Engine::start` maps the error to
/// `EngineError::Runtime`.
// `asupersync::Error` is large, and `RuntimeBuilder::build` returns it as is.
#[allow(clippy::result_large_err)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "Engine::start (E09) is the first caller")
)]
pub(crate) fn build_runtime() -> Result<Runtime, asupersync::Error> {
    builder().build()
}

/// `build_runtime` whose threads, the async worker and every blocking pool thread, all carry
/// `trace`, so their tasks and jobs report to this engine's shutdown (`Engine::start`).
#[allow(clippy::result_large_err)]
pub(crate) fn build_traced_runtime(
    trace: Arc<ShutdownTrace>,
) -> Result<Runtime, asupersync::Error> {
    builder()
        .on_thread_start(move || ShutdownTrace::enter(&trace))
        .build()
}

fn builder() -> RuntimeBuilder {
    RuntimeBuilder::new()
        .worker_threads(WORKER_THREADS)
        .blocking_threads(BLOCKING_THREADS_MIN, BLOCKING_THREADS_MAX)
        .thread_name_prefix(THREAD_NAME_PREFIX)
        .global_queue_limit(GLOBAL_QUEUE_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use asupersync::Cx;

    #[test]
    fn builds_and_runs_a_task() {
        let rt = build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async { 40 + 2 });
        let result = rt.block_on(join);
        assert_eq!(result.expect("task completes"), 42);
    }

    #[test]
    fn blocking_pool_is_configured() {
        let rt = build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async {
            let cx = Cx::current().expect("task has a Cx");
            let worker_id = std::thread::current().id();
            let mut handle = cx
                .spawn_blocking(|_| {
                    let thread = std::thread::current();
                    (thread.id(), thread.name().map(str::to_owned))
                })
                .expect("blocking job is admitted");
            let joined = handle.join(&cx).await;
            (worker_id, joined)
        });
        let (worker_id, joined) = rt.block_on(join).expect("task completes");
        let (blocking_id, blocking_name) = joined.expect("blocking job completes");
        let name = blocking_name.expect("blocking thread has a name");
        assert!(name.starts_with(THREAD_NAME_PREFIX), "thread name {name:?}");
        assert_ne!(
            blocking_id, worker_id,
            "blocking job ran on the async worker"
        );
    }
}
