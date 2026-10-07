//! A source whose `sample()` never returns must not stop the CPU source (tasklens-wlf.30,
//! ADR-004, ADR-013; TMOG feedback #1004: one drive query on an empty card reader froze every
//! graph for about 20 s).
//!
//! These run on the real runtime with the production pool sizes from `tl_engine::runtime`, not
//! on `LabRuntime`: the lab runtime has no blocking pool and runs `spawn_blocking` closures
//! inline, so a closure that blocks forever would hang the lab itself and could not show pool
//! exhaustion.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::runtime::{Runtime, RuntimeBuilder};
use tl_engine::dbus::SharedHandles;
use tl_engine::runtime::{BLOCKING_THREADS_MAX, BLOCKING_THREADS_MIN, WORKER_THREADS};
use tl_engine::source::{self, Source, WorkerControl, WorkerIo};
use tl_engine::update::{EngineMsg, SourceUpdate};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::settings::EngineSettings;

const RUN: Duration = Duration::from_secs(3);
/// 10 Hz for 3 s is 30 samples; allow a slow CI machine some slack.
const MIN_CPU_SAMPLES: u64 = 25;

/// Opened at the end of a test so the blocked pool threads can finish.
type Gate = Arc<(Mutex<bool>, Condvar)>;

fn health(id: SourceId) -> SourceUpdate {
    SourceUpdate::Health {
        id,
        state: SourceState::Running,
        last_error: None,
        t_ns: 0,
    }
}

/// Stands in for the CPU worker: 10 Hz, counts its samples.
struct CountingSource(Arc<AtomicU64>);

impl Source for CountingSource {
    fn id(&self) -> SourceId {
        SourceId::Cpu
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(Duration::from_millis(100))
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.0.fetch_add(1, Ordering::AcqRel);
        (self, health(SourceId::Cpu))
    }
}

/// A source whose first `sample()` blocks until the gate opens (the empty card reader).
struct BlockedSource {
    id: SourceId,
    gate: Gate,
}

impl Source for BlockedSource {
    fn id(&self) -> SourceId {
        self.id
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(Duration::from_millis(100))
    }

    fn sample(self) -> (Self, SourceUpdate) {
        {
            let (open, cv) = &*self.gate;
            let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
            while !*open {
                open = cv.wait(open).unwrap_or_else(PoisonError::into_inner);
            }
        }
        let id = self.id;
        (self, health(id))
    }
}

fn production_runtime() -> Runtime {
    RuntimeBuilder::new()
        .worker_threads(WORKER_THREADS)
        .blocking_threads(BLOCKING_THREADS_MIN, BLOCKING_THREADS_MAX)
        .build()
        .expect("runtime builds")
}

/// Runs the CPU source next to `blocked` blocked sources for [`RUN`] and returns its sample
/// count.
fn cpu_samples_beside(blocked: usize) -> u64 {
    let rt = production_runtime();
    let (updates, rx) = mpsc::channel::<EngineMsg>(4096);
    let (_demand_tx, demand) = watch::channel(Demand::default());
    let (_settings_tx, settings) = watch::channel(EngineSettings::default());
    let (_control_tx, control) = watch::channel(WorkerControl::default());
    let io = || {
        WorkerIo::new(
            updates.clone(),
            demand.clone(),
            settings.clone(),
            control.clone(),
            SharedHandles::default(),
        )
    };
    let gate: Gate = Arc::default();
    let ids = [
        SourceId::Disk,
        SourceId::Net,
        SourceId::GpuDrm,
        SourceId::Sensors,
        SourceId::Memory,
    ];
    for &id in ids.iter().take(blocked) {
        let (io, gate) = (io(), Arc::clone(&gate));
        rt.handle()
            .try_spawn_with_cx(move |cx| async move {
                let _ = source::run(cx, io, BlockedSource { id, gate }).await;
            })
            .expect("blocked source spawns");
    }
    let count = Arc::new(AtomicU64::new(0));
    let (cpu_io, cpu_count) = (io(), Arc::clone(&count));
    rt.handle()
        .try_spawn_with_cx(move |cx| async move {
            let _ = source::run(cx, cpu_io, CountingSource(cpu_count)).await;
        })
        .expect("cpu source spawns");

    std::thread::sleep(RUN);
    let samples = count.load(Ordering::Acquire);

    // Let the blocked threads finish, then leave the runtime and its looping tasks behind:
    // the sources never end on their own, and the process exits after the test.
    let (open, cv) = &*gate;
    *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
    cv.notify_all();
    std::mem::forget(rx);
    std::mem::forget(rt);
    samples
}

/// The pool has room: 3 blocked sources hold 3 of the 4 blocking threads.
#[test]
fn cpu_keeps_10hz_beside_three_blocked_sources() {
    let samples = cpu_samples_beside(BLOCKING_THREADS_MAX - 1);
    assert!(samples >= MIN_CPU_SAMPLES, "{samples} CPU samples in 3 s");
}

/// The tasklens-wlf.30 case: four sources blocked forever.
#[test]
fn cpu_keeps_10hz_beside_four_blocked_sources() {
    let samples = cpu_samples_beside(4);
    assert!(
        samples >= MIN_CPU_SAMPLES,
        "{samples} CPU samples in 3 s with 4 of {BLOCKING_THREADS_MAX} blocking threads held"
    );
}
