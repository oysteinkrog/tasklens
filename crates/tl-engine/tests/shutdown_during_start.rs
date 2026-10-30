//! Shutdown soon after `Engine::start`, while the sources are still starting (tasklens-8hz.13).
//!
//! These run on the real runtime with the real workers, because the bug was in how the runtime
//! tears down, which the lab runtime does not model. `tl-app` keeps one `EngineHandle` clone
//! alive inside GPUI's app state, which is never dropped, so the `live clone` cases copy that.

use std::time::{Duration, Instant};

use tl_collect::sysroot::SysRoot;
use tl_engine::bridge::{DRAIN_BUDGET, Engine, EngineHooks, TEARDOWN_BUDGET};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};

const DELAYS_MS: [u64; 3] = [50, 200, 1000];
/// Test limit for a shutdown: the ADR-004 drain and teardown budgets plus 2 s of slack for a busy
/// machine (tasklens-cr0.32). A hung shutdown still fails it.
const SHUTDOWN_LIMIT: Duration = DRAIN_BUDGET
    .saturating_add(TEARDOWN_BUDGET)
    .saturating_add(Duration::from_secs(2));

fn fixture_config() -> EngineConfig {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tl-collect/tests/fixtures/minimal-vm");
    EngineConfig::new(EngineMode::Full, base.join("state")).with_sys_root(SysRoot::at(&base))
}

/// Starts the engine, waits `delay`, shuts it down and checks the report. With `keep_clone`, a
/// second handle stays alive across the shutdown, as the GPUI global does in `tl-app`.
fn shutdown_after(delay: Duration, keep_clone: bool) {
    let handle = Engine::start(fixture_config(), EngineHooks::default()).expect("engine starts");
    let clone = keep_clone.then(|| handle.clone());
    std::thread::sleep(delay);
    let begin = Instant::now();
    let report = handle.shutdown();
    let waited = begin.elapsed();
    assert!(report.drained, "delay {delay:?}: {report:?}");
    assert!(report.torn_down, "delay {delay:?}: {report:?}");
    assert!(
        waited < SHUTDOWN_LIMIT,
        "delay {delay:?}: shutdown took {waited:?}"
    );
    if let Some(clone) = clone {
        assert_eq!(
            clone.send(Command::ResetBaselines),
            Err(CommandError::EngineStopped)
        );
    }
}

#[test]
fn shutdown_during_start_tears_down() {
    for ms in DELAYS_MS {
        shutdown_after(Duration::from_millis(ms), false);
    }
}

#[test]
fn shutdown_during_start_tears_down_with_live_clone() {
    for ms in DELAYS_MS {
        shutdown_after(Duration::from_millis(ms), true);
    }
}
