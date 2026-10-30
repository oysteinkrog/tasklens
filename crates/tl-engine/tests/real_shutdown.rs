//! Shutdown on the real system under load (ADR-013, E09-A2).
//!
//! Run on the owner machine with
//! `heavy-build cargo test -p tl-engine --test real_shutdown -- --ignored`. Each round starts
//! its own engine on the live `/proc` and `/sys` and stops only that engine.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tl_engine::{Engine, EngineConfig, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::demand::{ColumnSet, CpuDemand, Demand};

const ROUNDS: usize = 5;
/// `DRAIN_BUDGET` plus `TEARDOWN_BUDGET` (ADR-004).
const LIMIT: Duration =
    tl_engine::bridge::DRAIN_BUDGET.saturating_add(tl_engine::bridge::TEARDOWN_BUDGET);

/// Every surface visible: all workers registered so far sample at their fastest rate.
fn every_surface() -> Demand {
    Demand {
        any_surface_visible: true,
        cpu: CpuDemand::PerCore,
        cpu_fast: true,
        memory: true,
        disks: true,
        nets: true,
        gpus: true,
        sensors: true,
        process_table: true,
        process_columns: ColumnSet([u64::MAX; 4]),
        gpu_process: true,
        connections: true,
        services: true,
        startup: true,
        inventory: true,
        ..Demand::default()
    }
}

#[test]
#[ignore = "owner machine: samples the live system"]
fn shutdown_under_load_within_1500ms() {
    let state_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("real_shutdown-state");
    for round in 1..=ROUNDS {
        let config = EngineConfig::new(EngineMode::Full, state_dir.clone());
        let handle = Engine::start(config, EngineHooks::default()).expect("engine starts");
        handle.set_demand(every_surface());
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            handle.latest().sequence >= 2,
            "round {round}: no publishes in 3 s"
        );

        let begin = Instant::now();
        let report = handle.shutdown();
        let wall = begin.elapsed();
        eprintln!("round {round}: {report:?}, wall {wall:?}");
        assert!(report.clean(), "round {round}: {report:?}");
        assert!(report.elapsed < LIMIT, "round {round}: {report:?}");
    }
}
