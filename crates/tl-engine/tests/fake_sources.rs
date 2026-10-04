//! Scripted fake sources and a lab with a test-owned clock (ADR-013).
//!
//! Other test files include this with `#[path = "fake_sources.rs"] mod fake_sources;`.
//!
//! `Cx::spawn_blocking` runs the job inline in the lab and `Source::sample` gets no `Cx`, so a
//! job cannot await a sleep. `FakeLab` installs its own `VirtualClock` as the timer driver
//! instead, and a `Slow` step advances that clock while the job runs. Drive time only with
//! `FakeLab::advance`: `LabRuntime::advance_time` and `run_with_auto_advance` move the lab's
//! own clock, which no timer reads any more.

#![allow(dead_code)]

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::runtime::TaskHandle;
use asupersync::time::{TimeSource, TimerDriverHandle, VirtualClock};
use asupersync::types::{Budget, RegionId, Time};
use asupersync::{LabConfig, LabRuntime};
use tl_engine::source::{Source, WorkerControl, WorkerIo};
use tl_engine::update::{EngineMsg, SourceUpdate};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::settings::EngineSettings;
use tl_model::time::TimeNs;

/// One scripted sample.
#[derive(Clone, Copy, Debug)]
pub enum Step {
    Ok,
    Panic,
    /// Advances the virtual clock by this much while the job runs.
    Slow(Duration),
}

pub type Log = Arc<Mutex<Vec<(TimeNs, &'static str)>>>;

#[derive(Clone)]
pub struct FakeSource {
    pub id: SourceId,
    /// Consumed in order; the last step repeats.
    pub script: Vec<Step>,
    /// Returned by `period()` when `on(demand, settings)` is true.
    pub period_on: Option<Duration>,
    pub on: fn(&Demand, &EngineSettings) -> bool,
    /// `(t, "sample")` and `(t, "reset")` entries, `t` in virtual nanoseconds.
    pub log: Log,
    pub clock: Arc<VirtualClock>,
}

fn always_on(_: &Demand, _: &EngineSettings) -> bool {
    true
}

impl FakeSource {
    /// Always on at `period`, with an empty log.
    pub fn new(clock: &Arc<VirtualClock>, script: Vec<Step>, period: Duration) -> Self {
        assert!(!script.is_empty(), "a script needs at least one step");
        FakeSource {
            id: SourceId::Cpu,
            script,
            period_on: Some(period),
            on: always_on,
            log: Log::default(),
            clock: Arc::clone(clock),
        }
    }

    fn push(&self, what: &'static str) {
        let t = self.clock.now().as_nanos();
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((t, what));
    }
}

impl Source for FakeSource {
    fn id(&self) -> SourceId {
        self.id
    }

    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration> {
        if (self.on)(demand, settings) {
            self.period_on
        } else {
            None
        }
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        self.push("sample");
        let step = if self.script.len() > 1 {
            self.script.remove(0)
        } else {
            self.script[0]
        };
        match step {
            Step::Ok => {}
            Step::Panic => panic!("scripted panic"),
            Step::Slow(d) => self.clock.advance(d.as_nanos() as u64),
        }
        let update = SourceUpdate::Health {
            id: self.id,
            state: SourceState::Running,
            last_error: None,
            t_ns: self.clock.now().as_nanos(),
        };
        (self, update)
    }

    fn reset_baselines(&mut self) {
        self.push("reset");
    }
}

/// The time of every entry named `what`, in ms.
pub fn times_ms(log: &Log, what: &str) -> Vec<u64> {
    log.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|e| e.1 == what)
        .map(|e| e.0 / 1_000_000)
        .collect()
}

pub fn lab() -> LabRuntime {
    LabRuntime::new(LabConfig::new(42).max_steps(1_000_000))
}

/// The reducer's queue (both ends) and the three watch senders.
pub type Channels = (
    mpsc::Sender<EngineMsg>,
    mpsc::Receiver<EngineMsg>,
    watch::Sender<Demand>,
    watch::Sender<EngineSettings>,
    watch::Sender<WorkerControl>,
);

pub fn channels() -> Channels {
    let (updates, rx) = mpsc::channel::<EngineMsg>(256);
    let (demand, _) = watch::channel(Demand::default());
    let (settings, _) = watch::channel(EngineSettings::default());
    let (control, _) = watch::channel(WorkerControl::default());
    (updates, rx, demand, settings, control)
}

/// Clones the sender and makes new receivers from the watch senders.
pub fn io(
    updates: &mpsc::Sender<EngineMsg>,
    demand: &watch::Sender<Demand>,
    settings: &watch::Sender<EngineSettings>,
    control: &watch::Sender<WorkerControl>,
) -> WorkerIo {
    WorkerIo::new(
        updates.clone(),
        demand.subscribe(),
        settings.subscribe(),
        control.subscribe(),
    )
}

pub fn demand_with(f: impl FnOnce(&mut Demand)) -> Demand {
    let mut demand = Demand::default();
    f(&mut demand);
    demand
}

/// Every message queued so far.
pub fn drain(rx: &mut mpsc::Receiver<EngineMsg>) -> Vec<EngineMsg> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

/// A lab whose timers read `clock`, with one root region.
pub struct FakeLab {
    pub lab: LabRuntime,
    pub clock: Arc<VirtualClock>,
    pub root: RegionId,
}

impl Default for FakeLab {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeLab {
    pub fn new() -> Self {
        let mut lab = lab();
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        // Before any task exists: each `Cx` keeps the driver it was made with.
        lab.state
            .set_timer_driver(TimerDriverHandle::with_virtual_clock(Arc::clone(&clock)));
        let root = lab.state.create_root_region(Budget::INFINITE);
        FakeLab { lab, clock, root }
    }

    pub fn now_ms(&self) -> u64 {
        self.clock.now().as_nanos() / 1_000_000
    }

    /// Starts `fut` as a root task and runs the lab until it is idle.
    pub fn spawn<F>(&mut self, fut: F) -> TaskHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (task, handle) = self
            .lab
            .state
            .create_task(self.root, Budget::INFINITE, fut)
            .expect("task is created");
        self.lab.scheduler.lock().schedule(task, 0);
        self.lab.run_until_idle();
        handle
    }

    /// Moves the clock by `d` and runs every task that became ready.
    pub fn advance(&mut self, d: Duration) {
        self.clock.advance(d.as_nanos() as u64);
        self.lab.advance_time_to(self.clock.now());
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        self.lab.step_for_test();
        self.lab.run_until_idle();
    }

    /// Advances in `step` increments until `total` has passed.
    pub fn run_for(&mut self, total: Duration, step: Duration) {
        let end = self.clock.now().as_nanos() + total.as_nanos() as u64;
        while self.clock.now().as_nanos() < end {
            let left = end - self.clock.now().as_nanos();
            self.advance(step.min(Duration::from_nanos(left)));
        }
    }

    /// Runs to quiescence and checks the lab's oracles.
    pub fn finish(mut self) {
        let report = self.lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }
}
