//! The runtime's obligation table stays bounded while a source sends updates.
//!
//! Every `mpsc::Sender::send(cx, ..)` reserves a send permit, which the runtime records as an
//! obligation. asupersync 0.5.0 kept each resolved record forever (120 bytes each), so the
//! installed 1.3.0 engine held about 382 million of them after 32 hours (45 GB of heap).

use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::dbus::SharedHandles;
use tl_engine::source::{self, Source, WorkerControl, WorkerIo};
use tl_engine::update::{EngineMsg, SourceUpdate};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::settings::EngineSettings;

/// Samples every 100 ms and sends one health update per sample.
struct TickSource;

impl Source for TickSource {
    fn id(&self) -> SourceId {
        SourceId::Disk
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(Duration::from_millis(100))
    }

    fn sample(self) -> (Self, SourceUpdate) {
        let update = SourceUpdate::Health {
            id: SourceId::Disk,
            state: SourceState::Running,
            last_error: None,
            t_ns: 0,
        };
        (self, update)
    }
}

/// Samples sent in the test.
const SAMPLES: usize = 12_000;
/// Obligation records the runtime may keep. asupersync 0.6 keeps the last 4096 resolved
/// records for queries; anything near `SAMPLES` is the leak.
const MAX_RECORDS: usize = 5_000;

#[test]
fn obligation_table_stays_bounded_over_many_sends() {
    let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(10_000_000));
    let root = lab.state.create_root_region(Budget::INFINITE);
    let (updates, mut rx) = mpsc::channel::<EngineMsg>(256);
    let (_demand_tx, demand) = watch::channel(Demand::default());
    let (_settings_tx, settings) = watch::channel(EngineSettings::default());
    let (_control_tx, control) = watch::channel(WorkerControl::default());
    let io = WorkerIo::new(updates, demand, settings, control, SharedHandles::default());

    let (task, handle) = lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("task has a Cx");
            let _ = source::run(cx, io, TickSource).await;
        })
        .expect("task is created");
    lab.scheduler.lock().schedule(task, 0);
    lab.run_until_idle();

    let mut received = 0usize;
    while received < SAMPLES {
        lab.advance_time(Duration::from_millis(100).as_nanos() as u64);
        lab.step_for_test();
        lab.run_until_idle();
        while rx.try_recv().is_ok() {
            received += 1;
        }
    }

    let records = lab.state.obligations_len();
    assert!(
        records <= MAX_RECORDS,
        "{records} obligation records after {received} updates; resolved records must be freed"
    );

    handle.abort();
    let _ = lab.run_until_quiescent_with_report();
}
