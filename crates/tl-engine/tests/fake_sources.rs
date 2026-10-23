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
        tl_engine::dbus::SharedHandles::default(),
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

// Fake worker set and root endpoints (E09) -------------------------------------------------

use std::cell::RefCell;

use arc_swap::ArcSwap;
use asupersync::Cx;
use asupersync::supervision::ManagedGeneration;
use tl_engine::config::EngineConfig;
use tl_engine::reducer::Publisher;
use tl_engine::session::RootEndpoints;
use tl_engine::supervise::WorkerFactory;
use tl_engine::update::{CpuCoreSample, CpuSample, MemorySample};
use tl_engine::workers::WorkerSpec;
use tl_model::actions::ActionEvent;
use tl_model::snapshot::Snapshot;
use tl_model::system::{CpuSet, CpuTopology, CpuTotals, MemoryTotals, Metric};
use tl_model::time::BootId;
use tl_model::validity::Validity;

/// The boot ID `root_endpoints` uses.
pub const FAKE_BOOT: BootId = BootId(0x5e55);

/// Most workers one `fake_worker_set` can hold. Each slot has its own factory fn.
pub const MAX_FAKE_WORKERS: usize = 8;

/// A fake worker: `Cpu` sends a `CpuSample`, `Memory` a `MemorySample`, any other source a
/// `Health { Running }`. Each sample is logged as `(t, "sample")`.
#[derive(Clone)]
pub struct ScriptedSource {
    pub id: SourceId,
    /// Consumed in order; the last step repeats.
    pub script: Vec<Step>,
    pub period: Duration,
    pub log: Log,
    pub clock: Arc<VirtualClock>,
}

impl Source for ScriptedSource {
    fn id(&self) -> SourceId {
        self.id
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(self.period)
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        let t_ns = self.clock.now().as_nanos();
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((t_ns, "sample"));
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
        let t_ns = self.clock.now().as_nanos();
        let update = match self.id {
            SourceId::Cpu => SourceUpdate::Cpu(fake_cpu(t_ns)),
            SourceId::Memory => SourceUpdate::Memory(fake_memory(t_ns)),
            id => SourceUpdate::Health {
                id,
                state: SourceState::Running,
                last_error: None,
                t_ns,
            },
        };
        (self, update)
    }
}

fn fake_cpu(t_ns: TimeNs) -> CpuSample {
    let mut online = CpuSet::empty();
    online.insert(0);
    CpuSample {
        t_ns,
        totals: CpuTotals {
            util_pct: Metric::valid(50.0),
            kernel_pct: Metric::valid(10.0),
            ..CpuTotals::default()
        },
        irq_pct: 0.0,
        per_cpu: vec![CpuCoreSample {
            cpu: 0,
            pct: 50.0,
            kernel_pct: 10.0,
            freq_mhz: Some(3000),
            validity: Validity::Valid,
        }],
        freq_mhz: Some(3000),
        temp_c: None,
        temp_validity: Validity::Unsupported,
        topology: Some(Arc::new(CpuTopology {
            online,
            cores: Arc::from([]),
        })),
        validity: Validity::Valid,
    }
}

fn fake_memory(t_ns: TimeNs) -> MemorySample {
    MemorySample {
        t_ns,
        totals: MemoryTotals {
            validity: Validity::Valid,
            total: 8 << 30,
            available: 6 << 30,
            used: 2 << 30,
            ..MemoryTotals::default()
        },
        zram: Vec::new(),
        swap_in_flags: Default::default(),
        validity: Validity::Valid,
    }
}

thread_local! {
    /// The sources the slot factories clone. The lab runs every task on the test thread.
    static WORKER_SET: RefCell<Vec<ScriptedSource>> = const { RefCell::new(Vec::new()) };
    /// The clock of the newest `FakeLab::with_config` on this thread.
    static LAB_CLOCK: RefCell<Option<Arc<VirtualClock>>> = const { RefCell::new(None) };
}

/// `WorkerSpec::factory` is a fn pointer, so each slot of `WORKER_SET` gets its own fn.
fn slot_factory<const N: usize>(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    let src = WORKER_SET
        .with(|set| set.borrow().get(N).cloned())
        .expect("fake_worker_set filled this slot");
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let src = src.clone();
        async move { tl_engine::source::run(cx, io, src).await }
    })
}

const SLOT_FACTORIES: [fn(Arc<EngineConfig>, WorkerIo) -> WorkerFactory; MAX_FAKE_WORKERS] = [
    slot_factory::<0>,
    slot_factory::<1>,
    slot_factory::<2>,
    slot_factory::<3>,
    slot_factory::<4>,
    slot_factory::<5>,
    slot_factory::<6>,
    slot_factory::<7>,
];

/// One `ScriptedSource` worker per entry, sampling every `Duration`. Replaces the set of any
/// earlier call on this thread. Build the lab with `FakeLab::with_config` first: the sources
/// read its clock.
pub fn fake_worker_set(scripts: Vec<(SourceId, Vec<Step>, Duration)>) -> Vec<WorkerSpec> {
    assert!(
        scripts.len() <= MAX_FAKE_WORKERS,
        "at most {MAX_FAKE_WORKERS} fake workers"
    );
    let clock = LAB_CLOCK
        .with(|c| c.borrow().clone())
        .expect("FakeLab::with_config ran on this thread");
    let sources: Vec<ScriptedSource> = scripts
        .into_iter()
        .map(|(id, script, period)| {
            assert!(!script.is_empty(), "a script needs at least one step");
            ScriptedSource {
                id,
                script,
                period,
                log: Log::default(),
                clock: Arc::clone(&clock),
            }
        })
        .collect();
    let specs = sources
        .iter()
        .zip(SLOT_FACTORIES)
        .map(|(src, factory)| WorkerSpec {
            id: src.id,
            name: "fake",
            modes: &[tl_engine::config::EngineMode::Full],
            factory,
        })
        .collect();
    WORKER_SET.with(|set| *set.borrow_mut() = sources);
    specs
}

/// The log of the fake worker for `id` in the current set.
pub fn worker_log(id: SourceId) -> Log {
    WORKER_SET
        .with(|set| {
            set.borrow()
                .iter()
                .find(|src| src.id == id)
                .map(|src| Arc::clone(&src.log))
        })
        .expect("the current fake_worker_set has this source")
}

/// The UI's ends of the channels `root_endpoints` makes.
pub struct UiSide {
    pub latest: Arc<ArcSwap<Snapshot>>,
    pub wake_rx: async_channel::Receiver<()>,
    pub events_rx: async_channel::Receiver<ActionEvent>,
    pub demand_tx: watch::Sender<Demand>,
    pub settings_tx: watch::Sender<EngineSettings>,
    pub msgs_tx: mpsc::Sender<EngineMsg>,
}

/// Root endpoints with default hooks, `FAKE_BOOT` and sequence 0, and the UI side to drive them.
pub fn root_endpoints(
    config: Arc<EngineConfig>,
    workers: Vec<WorkerSpec>,
) -> (RootEndpoints, UiSide) {
    let (msgs_tx, msgs) = mpsc::channel(256);
    let (demand_tx, demand) = watch::channel(Demand::default());
    let (settings_tx, settings) = watch::channel(EngineSettings::default());
    let (control, _) = watch::channel(WorkerControl::default());
    let (wake_tx, wake_rx) = async_channel::bounded(1);
    let (events_tx, events_rx) = async_channel::bounded(256);
    let latest = Arc::new(ArcSwap::from_pointee(Snapshot::empty(FAKE_BOOT, 0)));
    let ep = RootEndpoints {
        msgs,
        msgs_tx: msgs_tx.clone(),
        demand,
        settings,
        control,
        publisher: Publisher {
            latest: Arc::clone(&latest),
            wake_tx,
            stop_publishing: false,
        },
        events_tx,
        config,
        boot_id: FAKE_BOOT,
        hooks: tl_engine::bridge::EngineHooks::default(),
        workers,
        next_sequence: 0,
    };
    let ui = UiSide {
        latest,
        wake_rx,
        events_rx,
        demand_tx,
        settings_tx,
        msgs_tx,
    };
    (ep, ui)
}

impl FakeLab {
    /// Like `new`, with `config` (a seed, chaos). `fake_worker_set` reads this lab's clock.
    pub fn with_config(config: LabConfig) -> Self {
        let mut lab = LabRuntime::new(config);
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        lab.state
            .set_timer_driver(TimerDriverHandle::with_virtual_clock(Arc::clone(&clock)));
        let root = lab.state.create_root_region(Budget::INFINITE);
        LAB_CLOCK.with(|c| *c.borrow_mut() = Some(Arc::clone(&clock)));
        FakeLab { lab, clock, root }
    }
}
