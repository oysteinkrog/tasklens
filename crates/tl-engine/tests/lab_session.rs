//! LabRuntime tests for `engine_root` and `engine_session` (E09-016, ADR-004, ADR-013).
//!
//! The reducer is made to panic through the action gate hook: `PanicGate::submit` panics, so a
//! `Command::Action` in the queue ends the session. Each gate factory call is one session start.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use asupersync::Cx;
use asupersync::channel::{mpsc, watch};
use asupersync::runtime::TaskHandle;
use asupersync::supervision::ManagedGeneration;
use smallvec::SmallVec;
use tl_engine::bridge::{EngineHooks, HookCtx};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::reducer::{ActionGate, AdmissionView, Publisher};
use tl_engine::session::{RESTART_DELAY, RootEndpoints, engine_root};
use tl_engine::source::{WorkerControl, WorkerIo};
use tl_engine::supervise::WorkerFactory;
use tl_engine::update::{CpuCoreSample, CpuSample, EngineMsg, SourceUpdate};
use tl_engine::workers::WorkerSpec;
use tl_model::actions::{ActionEvent, ActionId, ActionKind, ActionRequest, Signal};
use tl_model::command::Command;
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::series::SampleFlags;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::system::{CpuSet, CpuTopology, CpuTotals, Metric};
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::Validity;

use fake_sources::{FakeLab, FakeSource, Step};

const BOOT: BootId = BootId(0x5e55);
const MS: TimeNs = 1_000_000;

struct PanicGate;

impl ActionGate for PanicGate {
    fn submit(&mut self, _cx: &Cx, _req: ActionRequest, _view: &AdmissionView<'_>) {
        panic!("scripted reducer panic");
    }

    fn finished(&mut self, _id: ActionId) -> Option<SourceId> {
        None
    }
}

thread_local! {
    /// The source `fake_factory` clones. The lab runs every task on the test thread.
    static FAKE: RefCell<Option<FakeSource>> = const { RefCell::new(None) };
}

fn fake_factory(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    let src = FAKE
        .with(|f| f.borrow().clone())
        .expect("the test set FAKE");
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let src = src.clone();
        async move { tl_engine::source::run(cx, io, src).await }
    })
}

fn fake_spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Cpu,
        name: "cpu",
        modes: &[EngineMode::Full],
        factory: fake_factory,
    }
}

struct Rig {
    fake: FakeLab,
    root: Option<TaskHandle<()>>,
    latest: Arc<ArcSwap<Snapshot>>,
    msgs_tx: mpsc::Sender<EngineMsg>,
    /// Gate factory calls, one per session.
    sessions: Arc<AtomicU32>,
    // Held so the root's channels stay open.
    _wake_rx: async_channel::Receiver<()>,
    _events_rx: async_channel::Receiver<ActionEvent>,
    _demand_tx: watch::Sender<Demand>,
    _settings_tx: watch::Sender<EngineSettings>,
    ep: Option<RootEndpoints>,
}

impl Rig {
    fn new(workers: Vec<WorkerSpec>, next_sequence: u64) -> Rig {
        let fake = FakeLab::new();
        let (msgs_tx, msgs) = mpsc::channel(256);
        let (demand_tx, demand) = watch::channel(Demand::default());
        let (settings_tx, settings) = watch::channel(EngineSettings::default());
        let (control, _) = watch::channel(WorkerControl::default());
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (events_tx, events_rx) = async_channel::bounded(256);
        let latest = Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 0)));
        let sessions = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&sessions);
        let hooks = EngineHooks {
            action_gate: Some(Arc::new(move |_ctx: HookCtx| {
                counter.fetch_add(1, Ordering::Relaxed);
                Box::new(PanicGate) as Box<dyn ActionGate>
            })),
            ..EngineHooks::default()
        };
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
            config: Arc::new(EngineConfig::new(
                EngineMode::Full,
                "/var/tmp/claude/8q0.18-state".into(),
            )),
            boot_id: BOOT,
            hooks,
            workers,
            next_sequence,
        };
        Rig {
            fake,
            root: None,
            latest,
            msgs_tx,
            sessions,
            _wake_rx: wake_rx,
            _events_rx: events_rx,
            _demand_tx: demand_tx,
            _settings_tx: settings_tx,
            ep: Some(ep),
        }
    }

    fn start(&mut self) {
        let ep = self.ep.take().expect("one start per rig");
        let handle = self.fake.spawn(async move {
            let cx = Cx::current().expect("task has a Cx");
            engine_root(cx, ep).await;
        });
        self.root = Some(handle);
    }

    fn send(&mut self, msg: EngineMsg) {
        self.msgs_tx
            .try_send(msg)
            .expect("the queue has room for the message");
        self.fake.lab.run_until_idle();
    }

    fn latest(&self) -> Arc<Snapshot> {
        self.latest.load_full()
    }

    fn sessions(&self) -> u32 {
        self.sessions.load(Ordering::Relaxed)
    }

    fn root_finished(&self) -> bool {
        self.root.as_ref().expect("root started").is_finished()
    }

    /// Cancels the root and runs until it returns.
    fn stop(&mut self) {
        self.root.as_ref().expect("root started").abort();
        self.fake.advance(Duration::from_millis(10));
        assert!(self.root_finished(), "engine_root returned after cancel");
    }
}

fn action(id: u64) -> EngineMsg {
    EngineMsg::Command(Command::Action(ActionRequest {
        id: ActionId(id),
        kind: ActionKind::Signal(Signal::Term),
        targets: SmallVec::new(),
        privileged: false,
    }))
}

fn cpu_msg(t_ns: TimeNs, pct: f32) -> EngineMsg {
    let mut online = CpuSet::empty();
    online.insert(0);
    online.insert(1);
    EngineMsg::Source(SourceUpdate::Cpu(CpuSample {
        t_ns,
        totals: CpuTotals {
            util_pct: Metric::valid(pct),
            kernel_pct: Metric::valid(pct / 2.0),
            ..CpuTotals::default()
        },
        irq_pct: 0.0,
        per_cpu: (0..2)
            .map(|cpu| CpuCoreSample {
                cpu,
                pct,
                kernel_pct: 0.0,
                freq_mhz: Some(3000),
                validity: Validity::Valid,
            })
            .collect(),
        freq_mhz: Some(3000),
        temp_c: None,
        temp_validity: Validity::Unsupported,
        topology: Some(Arc::new(CpuTopology {
            online,
            cores: Arc::from([]),
        })),
        validity: Validity::Valid,
    }))
}

#[test]
fn reducer_panic_restarts_with_new_epoch() {
    let mut rig = Rig::new(Vec::new(), 0);
    rig.start();
    assert_eq!(rig.sessions(), 1);
    for i in 1..=4 {
        rig.send(cpu_msg(i * 100 * MS, 50.0));
    }
    let before = rig.latest();
    assert_eq!(before.epoch, 1);
    assert!(!before.series.series.is_empty());

    // The 5th message panics inside the reducer.
    rig.send(action(1));
    assert_eq!(rig.sessions(), 1);
    rig.fake.advance(RESTART_DELAY - Duration::from_millis(1));
    assert_eq!(rig.sessions(), 1, "no restart before the delay");
    rig.fake.advance(Duration::from_millis(1));
    assert_eq!(rig.sessions(), 2, "session 2 starts after 1 s");
    assert!(!rig.root_finished());

    // The same queue still reaches the new reducer.
    rig.send(cpu_msg(2_000 * MS, 25.0));
    let after = rig.latest();
    assert_eq!(after.epoch, 2);
    assert!(!after.series.series.is_empty());
    for view in after.series.series.iter() {
        let first = view.samples.first().expect("a sample in epoch 2");
        assert!(
            first.flags.contains(SampleFlags::GAP_BEFORE),
            "{:?} starts epoch 2 without GAP_BEFORE",
            view.id
        );
    }
    rig.stop();
}

#[test]
fn four_restarts_publish_all_failed_and_stay_alive() {
    let mut rig = Rig::new(Vec::new(), 0);
    // One action per session: each new reducer panics on its first message.
    for i in 1..=8 {
        rig.msgs_tx.try_send(action(i)).expect("room in the queue");
    }
    rig.start();
    for _ in 0..10 {
        rig.fake.advance(Duration::from_secs(1));
    }
    assert_eq!(rig.sessions(), 4, "sessions 1 to 4 start, then no more");
    let snap = rig.latest();
    assert_eq!(snap.epoch, 4);
    for health in snap.health.sources.iter() {
        assert_eq!(health.state, SourceState::Failed);
        assert_eq!(
            health.last_error.as_deref(),
            Some("engine restarted too often")
        );
    }

    rig.fake
        .run_for(Duration::from_secs(120), Duration::from_secs(10));
    assert!(!rig.root_finished(), "the root stays alive");
    assert_eq!(rig.sessions(), 4);
    rig.stop();
}

#[test]
fn cancel_stops_everything() {
    let mut rig = Rig::new(vec![fake_spec()], 0);
    let src = FakeSource::new(&rig.fake.clock, vec![Step::Ok], Duration::from_millis(100));
    let log = Arc::clone(&src.log);
    FAKE.with(|f| *f.borrow_mut() = Some(src));
    rig.start();
    rig.fake
        .run_for(Duration::from_secs(1), Duration::from_millis(100));
    let samples = || fake_sources::times_ms(&log, "sample").len();
    assert!(samples() >= 5, "the fake worker samples: {}", samples());

    rig.stop();
    let after_cancel = samples();
    rig.fake
        .run_for(Duration::from_secs(5), Duration::from_millis(100));
    assert_eq!(samples(), after_cancel, "no sample() after the cancel");
    FAKE.with(|f| *f.borrow_mut() = None);
}

#[test]
fn tick_every_second() {
    let mut rig = Rig::new(Vec::new(), 0);
    rig.start();
    rig.fake
        .run_for(Duration::from_secs(10), Duration::from_millis(100));
    // Nothing else reaches the reducer, so each publish is one `Tick`.
    assert_eq!(rig.latest().sequence, 10);
    rig.stop();
}

#[test]
fn sequence_continues_across_epochs() {
    let mut rig = Rig::new(Vec::new(), 499);
    rig.start();
    rig.send(cpu_msg(100 * MS, 50.0));
    let last = rig.latest();
    assert_eq!((last.epoch, last.sequence), (1, 500));

    rig.send(action(1));
    rig.fake.advance(RESTART_DELAY);
    assert_eq!(rig.sessions(), 2);
    rig.send(cpu_msg(1_200 * MS, 50.0));
    let next = rig.latest();
    assert_eq!((next.epoch, next.sequence), (2, 501));
    rig.stop();
}
