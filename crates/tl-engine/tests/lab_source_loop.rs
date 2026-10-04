//! LabRuntime tests for the sampling loop in `source.rs` (ADR-004, ADR-013).

use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::source::{self, Source, WorkerControl, WorkerIo};
use tl_engine::update::{DeviceKinds, DeviceList, EngineMsg, SourceUpdate};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::settings::EngineSettings;

/// Always on at 100 ms. Its first sample leaves one `Devices` extra behind.
struct ExtraSource {
    extra: Option<SourceUpdate>,
    sampled: bool,
}

impl Source for ExtraSource {
    fn id(&self) -> SourceId {
        SourceId::Disk
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(Duration::from_millis(100))
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        if !self.sampled {
            self.sampled = true;
            self.extra = Some(SourceUpdate::Devices(DeviceList {
                t_ns: 0,
                kinds: DeviceKinds::DISKS,
                entries: Vec::new(),
            }));
        }
        let update = SourceUpdate::Health {
            id: SourceId::Disk,
            state: SourceState::Running,
            last_error: None,
            t_ns: 0,
        };
        (self, update)
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.extra.take()
    }
}

#[test]
fn extras_sent_before_sample() {
    let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(1_000_000));
    let (updates, mut rx) = mpsc::channel::<EngineMsg>(256);
    let (_demand_tx, demand) = watch::channel(Demand::default());
    let (_settings_tx, settings) = watch::channel(EngineSettings::default());
    let (_control_tx, control) = watch::channel(WorkerControl::default());
    let io = WorkerIo::new(updates, demand, settings, control);
    let src = ExtraSource {
        extra: None,
        sampled: false,
    };

    let root = lab.state.create_root_region(Budget::INFINITE);
    let (task, handle) = lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("task has a Cx");
            source::run(cx, io, src).await
        })
        .expect("task is created");
    lab.scheduler.lock().schedule(task, 0);
    lab.run_until_idle();

    let first = rx.try_recv().expect("the extra arrived");
    assert!(
        matches!(first, EngineMsg::Source(SourceUpdate::Devices(_))),
        "first message {first:?}"
    );
    let second = rx.try_recv().expect("the sample arrived");
    assert!(
        matches!(
            second,
            EngineMsg::Source(SourceUpdate::Health {
                state: SourceState::Running,
                ..
            })
        ),
        "second message {second:?}"
    );
    assert!(rx.try_recv().is_err(), "one sample, one extra");

    handle.abort();
    let report = lab.run_until_quiescent_with_report();
    assert!(report.lab_test_passed(), "{report:?}");
}
