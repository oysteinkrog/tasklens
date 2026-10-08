//! A source whose `sample()` never returns must not stop the CPU source (tasklens-wlf.30,
//! ADR-004, ADR-013; TMOG feedback #1004: one drive query on an empty card reader froze every
//! graph for about 20 s). Each source samples on its own thread, so a hung sample stalls only
//! its own source, which reports `Late`.
//!
//! These run on the real runtime with the production pool sizes from `tl_engine::runtime`, not
//! on `LabRuntime`: the lab runtime has no blocking pool and runs samples inline, so a sample
//! that blocks forever would hang the lab itself.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use asupersync::channel::{mpsc, watch};
use asupersync::runtime::{Runtime, RuntimeBuilder};
use asupersync::{CancelKind, Cx};
use tl_engine::dbus::SharedHandles;
use tl_engine::runtime::{BLOCKING_THREADS_MAX, BLOCKING_THREADS_MIN, WORKER_THREADS};
use tl_engine::source::{self, LATE_COUNT, Source, WorkerControl, WorkerIo};
use tl_engine::update::{EngineMsg, SourceUpdate};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::settings::EngineSettings;

const RUN: Duration = Duration::from_secs(3);
/// 10 Hz for 3 s is 30 samples; allow a slow CI machine some slack.
const MIN_CPU_SAMPLES: u64 = 25;
const PERIOD: Duration = Duration::from_millis(100);

/// Opened at the end of a test so the blocked sampling threads can finish.
type Gate = Arc<(Mutex<bool>, Condvar)>;

fn open(gate: &Gate) {
    let (open, cv) = &**gate;
    *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
    cv.notify_all();
}

fn health(id: SourceId) -> SourceUpdate {
    SourceUpdate::Health {
        id,
        state: SourceState::Running,
        last_error: None,
        t_ns: 0,
    }
}

/// Stands in for the CPU worker: 10 Hz, counts its samples.
struct CountingSource {
    id: SourceId,
    count: Arc<AtomicU64>,
}

impl Source for CountingSource {
    fn id(&self) -> SourceId {
        self.id
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(PERIOD)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.count.fetch_add(1, Ordering::AcqRel);
        let id = self.id;
        (self, health(id))
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
        Some(PERIOD)
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

/// The production runtime, the reducer's queue and the watches the sources read.
struct Rig {
    rt: Runtime,
    updates: mpsc::Sender<EngineMsg>,
    rx: mpsc::Receiver<EngineMsg>,
    demand: watch::Receiver<Demand>,
    settings: watch::Receiver<EngineSettings>,
    control: watch::Receiver<WorkerControl>,
    _tx: (
        watch::Sender<Demand>,
        watch::Sender<EngineSettings>,
        watch::Sender<WorkerControl>,
    ),
}

impl Rig {
    fn new() -> Self {
        let rt = RuntimeBuilder::new()
            .worker_threads(WORKER_THREADS)
            .blocking_threads(BLOCKING_THREADS_MIN, BLOCKING_THREADS_MAX)
            .build()
            .expect("runtime builds");
        let (updates, rx) = mpsc::channel::<EngineMsg>(4096);
        let (demand_tx, demand) = watch::channel(Demand::default());
        let (settings_tx, settings) = watch::channel(EngineSettings::default());
        let (control_tx, control) = watch::channel(WorkerControl::default());
        Self {
            rt,
            updates,
            rx,
            demand,
            settings,
            control,
            _tx: (demand_tx, settings_tx, control_tx),
        }
    }

    /// Starts `src`'s sampling loop. Returns its `Cx`, for cancelling, and a channel that gets
    /// one message when the loop returns.
    fn start<S: Source>(&self, src: S) -> (Arc<Mutex<Option<Cx>>>, std::sync::mpsc::Receiver<()>) {
        let io = WorkerIo::new(
            self.updates.clone(),
            self.demand.clone(),
            self.settings.clone(),
            self.control.clone(),
            SharedHandles::default(),
        );
        let slot = Arc::new(Mutex::new(None));
        let (ended_tx, ended) = std::sync::mpsc::channel();
        let task_slot = Arc::clone(&slot);
        self.rt
            .handle()
            .try_spawn_with_cx(move |cx| async move {
                *task_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(cx.clone());
                let _ = source::run(cx, io, src).await;
                let _ = ended_tx.send(());
            })
            .expect("source spawns");
        (slot, ended)
    }

    /// Every message the sources have sent so far.
    fn drain(&mut self) -> Vec<SourceUpdate> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            if let EngineMsg::Source(update) = msg {
                out.push(update);
            }
        }
        out
    }

    /// Leaves the runtime and its looping tasks behind: the sources never end on their own,
    /// and the process exits after the test.
    fn leak(self) {
        std::mem::forget(self);
    }
}

fn cancel(slot: &Mutex<Option<Cx>>) {
    let guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
    guard
        .as_ref()
        .expect("the task has started")
        .cancel_with(CancelKind::Shutdown, Some("test shutdown"));
}

/// The names of this process's threads, from `/proc/self/task/*/comm`.
fn thread_names() -> Vec<String> {
    std::fs::read_dir("/proc/self/task")
        .expect("/proc/self/task")
        .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
        .map(|name| name.trim_end().to_owned())
        .collect()
}

/// Waits up to 2 s for `test` to hold.
fn eventually(mut test: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        if test() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    test()
}

/// Runs the CPU source next to `blocked` blocked sources for [`RUN`] and returns its sample
/// count.
fn cpu_samples_beside(blocked: usize) -> u64 {
    let rig = Rig::new();
    let gate: Gate = Arc::default();
    let ids = [
        SourceId::Disk,
        SourceId::Net,
        SourceId::GpuDrm,
        SourceId::Sensors,
        SourceId::Memory,
    ];
    for &id in ids.iter().take(blocked) {
        let gate = Arc::clone(&gate);
        rig.start(BlockedSource { id, gate });
    }
    let count = Arc::new(AtomicU64::new(0));
    rig.start(CountingSource {
        id: SourceId::Cpu,
        count: Arc::clone(&count),
    });

    std::thread::sleep(RUN);
    let samples = count.load(Ordering::Acquire);
    open(&gate);
    rig.leak();
    samples
}

/// 3 blocked sources: fewer than the 4 blocking-pool threads.
#[test]
fn cpu_keeps_10hz_beside_three_blocked_sources() {
    let samples = cpu_samples_beside(BLOCKING_THREADS_MAX - 1);
    assert!(samples >= MIN_CPU_SAMPLES, "{samples} CPU samples in 3 s");
}

/// The tasklens-wlf.30 case: four sources blocked forever, as many as the blocking pool has
/// threads. Samples do not use the pool, so the CPU source keeps its rate.
#[test]
fn cpu_keeps_10hz_beside_four_blocked_sources() {
    let samples = cpu_samples_beside(4);
    assert!(
        samples >= MIN_CPU_SAMPLES,
        "{samples} CPU samples in 3 s beside 4 hung sources ({BLOCKING_THREADS_MAX} pool threads)"
    );
}

/// A sample that never returns sets the source's health to `Late` after `LATE_COUNT` periods,
/// without waiting for the sample.
#[test]
fn hung_source_reports_late() {
    let mut rig = Rig::new();
    let gate: Gate = Arc::default();
    rig.start(BlockedSource {
        id: SourceId::Sensors,
        gate: Arc::clone(&gate),
    });

    let mut seen = Vec::new();
    let late = eventually(|| {
        seen.extend(rig.drain());
        seen.iter().any(|u| {
            matches!(
                u,
                SourceUpdate::Health {
                    id: SourceId::Sensors,
                    state: SourceState::Late,
                    ..
                }
            )
        })
    });
    open(&gate);
    rig.leak();
    assert!(
        late,
        "no Late within 2 s of a hung sample ({LATE_COUNT} periods of {PERIOD:?}): {seen:?}"
    );
}

/// Shutdown: cancelling ends the loop at once even while its sample hangs. An idle sampling
/// thread ends with its loop; a hung one is left detached and ends when its sample returns.
#[test]
fn cancel_ends_the_loop_and_detaches_a_hung_sample() {
    let rig = Rig::new();
    let gate: Gate = Arc::default();
    let (hung_cx, hung_ended) = rig.start(BlockedSource {
        id: SourceId::UPower,
        gate: Arc::clone(&gate),
    });
    let (idle_cx, idle_ended) = rig.start(CountingSource {
        id: SourceId::Udisks,
        count: Arc::default(),
    });
    let has = |name: &str| thread_names().iter().any(|n| n == name);
    assert!(eventually(|| has("tl-src-UPower") && has("tl-src-Udisks")));

    cancel(&hung_cx);
    cancel(&idle_cx);
    let limit = Duration::from_secs(1);
    assert!(
        hung_ended.recv_timeout(limit).is_ok(),
        "hung loop did not end"
    );
    assert!(
        idle_ended.recv_timeout(limit).is_ok(),
        "idle loop did not end"
    );
    assert!(
        eventually(|| !has("tl-src-Udisks")),
        "idle thread still runs"
    );
    assert!(
        has("tl-src-UPower"),
        "hung thread ended before its sample returned"
    );

    open(&gate);
    assert!(
        eventually(|| !has("tl-src-UPower")),
        "hung thread did not end"
    );
    drop(rig);
}
