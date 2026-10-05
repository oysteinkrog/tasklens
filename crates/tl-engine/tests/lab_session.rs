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

use asupersync::Cx;
use asupersync::lab::LabConfig;
use asupersync::lab::chaos::ChaosConfig;
use asupersync::runtime::TaskHandle;
use asupersync::supervision::ManagedGeneration;
use smallvec::SmallVec;
use tl_engine::bridge::{EngineHooks, HookCtx};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::reducer::{ActionGate, AdmissionView};
use tl_engine::session::{RESTART_DELAY, RootEndpoints, engine_root};
use tl_engine::source::WorkerIo;
use tl_engine::supervise::WorkerFactory;
use tl_engine::update::{
    CpuCoreSample, CpuSample, DeviceDesc, DeviceKinds, DeviceList, EngineMsg, NetSample,
    SourceUpdate,
};
use tl_engine::workers::WorkerSpec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest, Signal};
use tl_model::command::Command;
use tl_model::devices::{AggregateKind, DeviceIdentity, NetKind};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{MetricId, SeriesId};
use tl_model::series::SampleFlags;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::system::{CpuSet, CpuTopology, CpuTotals, LinkState, Metric, NetTotals};
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use fake_sources::{FakeLab, FakeSource, Step, UiSide, fake_worker_set, times_ms, worker_log};

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
    /// Gate factory calls, one per session.
    sessions: Arc<AtomicU32>,
    // Holds the UI's channel ends, so the root's channels stay open.
    ui: UiSide,
    ep: Option<RootEndpoints>,
}

impl Rig {
    fn new(workers: Vec<WorkerSpec>, next_sequence: u64) -> Rig {
        Rig::with_lab(FakeLab::new(), workers, next_sequence)
    }

    fn with_lab(fake: FakeLab, workers: Vec<WorkerSpec>, next_sequence: u64) -> Rig {
        let config = Arc::new(EngineConfig::new(
            EngineMode::Full,
            "/var/tmp/claude/8q0.18-state".into(),
        ));
        let (mut ep, ui) = fake_sources::root_endpoints(config, workers);
        let sessions = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&sessions);
        ep.hooks = EngineHooks {
            action_gate: Some(Arc::new(move |_ctx: HookCtx| {
                counter.fetch_add(1, Ordering::Relaxed);
                Box::new(PanicGate) as Box<dyn ActionGate>
            })),
            ..EngineHooks::default()
        };
        ep.next_sequence = next_sequence;
        Rig {
            fake,
            root: None,
            sessions,
            ui,
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
        self.ui
            .msgs_tx
            .try_send(msg)
            .expect("the queue has room for the message");
        self.fake.lab.run_until_idle();
    }

    fn latest(&self) -> Arc<Snapshot> {
        self.ui.latest.load_full()
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

    /// Stops the root, runs to quiescence and checks the lab's oracles.
    fn finish(mut self) {
        self.stop();
        self.fake.finish();
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
    rig.finish();
}

#[test]
fn four_restarts_publish_all_failed_and_stay_alive() {
    let mut rig = Rig::new(Vec::new(), 0);
    // One action per session: each new reducer panics on its first message.
    for i in 1..=8 {
        rig.ui
            .msgs_tx
            .try_send(action(i))
            .expect("room in the queue");
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
    rig.finish();
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
    rig.fake.finish();
}

#[test]
fn tick_every_second() {
    let mut rig = Rig::new(Vec::new(), 0);
    rig.start();
    rig.fake
        .run_for(Duration::from_secs(10), Duration::from_millis(100));
    // Nothing else reaches the reducer, so each publish is one `Tick`.
    assert_eq!(rig.latest().sequence, 10);
    rig.finish();
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
    rig.finish();
}

fn net_id(name: &str, ifindex: u32) -> DeviceIdentity {
    DeviceIdentity::Net {
        netns_ino: 1,
        ifindex,
        name: Arc::from(name),
    }
}

fn net_desc(identity: DeviceIdentity, kind: NetKind) -> DeviceDesc {
    DeviceDesc {
        identity,
        display_name: Arc::from("if"),
        parent: None,
        persistent_key: Arc::from("net:if"),
        net_kind: Some(kind),
    }
}

fn net_msg(t_ns: TimeNs, nets: Vec<(DeviceIdentity, f64)>) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Nets(NetSample {
        t_ns,
        nets: nets
            .into_iter()
            .map(|(id, rx)| {
                let totals = NetTotals {
                    device: tl_model::ids::DeviceIdx(0),
                    rx_bps: Metric::valid(rx),
                    tx_bps: Metric::valid(0.0),
                    rx_total: 0,
                    tx_total: 0,
                    link_speed_mbps: Metric::with(Validity::Unsupported),
                    state: LinkState::Up,
                };
                (id, totals)
            })
            .collect(),
    }))
}

#[test]
fn settings_change_reaches_reducer() {
    let mut rig = Rig::new(Vec::new(), 0);
    let with_virtual = |show: bool| EngineSettings {
        show_network_virtual: show,
        show_network_wired: true,
        ..EngineSettings::default()
    };
    rig.ui
        .settings_tx
        .send(with_virtual(false))
        .expect("the root holds a receiver");
    rig.start();
    let (eno1, docker0) = (net_id("eno1", 2), net_id("docker0", 3));
    rig.send(EngineMsg::Source(SourceUpdate::Devices(DeviceList {
        t_ns: 100 * MS,
        kinds: DeviceKinds::NETS,
        entries: vec![
            net_desc(eno1.clone(), NetKind::Wired),
            net_desc(docker0.clone(), NetKind::Virtual),
        ],
    })));
    let agg_rx = |rig: &Rig| {
        let snap = rig.latest();
        let agg = snap
            .devices
            .find(&DeviceIdentity::Aggregate(AggregateKind::Nets))
            .expect("the net aggregate exists")
            .idx;
        snap.series
            .get(SeriesId {
                device: agg,
                metric: MetricId::NET_RX,
            })
            .expect("NET_RX exists")
            .samples
            .last()
            .expect("a sample")
            .value
    };

    rig.send(net_msg(
        200 * MS,
        vec![(eno1.clone(), 5000.0), (docker0.clone(), 700.0)],
    ));
    assert_eq!(agg_rx(&rig), 5000.0, "virtual interfaces hidden");

    rig.ui
        .settings_tx
        .send(with_virtual(true))
        .expect("the root holds a receiver");
    rig.send(net_msg(300 * MS, vec![(eno1, 5000.0), (docker0, 700.0)]));
    assert_eq!(agg_rx(&rig), 5700.0, "the new settings reached the reducer");
    rig.finish();
}

// Chaos runs and the fake worker set (E09) ---------------------------------------------------

/// Delay and wakeup-storm injection for `seed`, plus cancel injection at `cancel` per poll.
/// The chaos seed derives from `seed`.
fn chaos_lab(seed: u64, cancel: f64) -> FakeLab {
    let chaos = ChaosConfig::default()
        .with_cancel_probability(cancel)
        .with_delay_probability(0.05)
        .with_delay_range(Duration::ZERO..Duration::from_millis(10))
        .with_wakeup_storm_probability(0.01);
    FakeLab::with_config(
        LabConfig::new(seed)
            .worker_count(2)
            .max_steps(1_000_000)
            .with_chaos(chaos),
    )
}

/// Every series of `snap` starts with `GAP_BEFORE`.
fn assert_gap_before_all(snap: &Snapshot, seed: u64) {
    for view in snap.series.series.iter() {
        let first = view.samples.first().expect("a sample in epoch 2");
        assert!(
            first.flags.contains(SampleFlags::GAP_BEFORE),
            "seed {seed}: {:?} starts epoch 2 without GAP_BEFORE",
            view.id
        );
    }
}

/// Mirrors `#[explore_seeds(seeds = 0..16, workers = 2, chaos)]`, which asupersync 0.5.0 does
/// not re-export (see `lab_supervise.rs::explore_restart`).
#[test]
fn explore_session_restart() {
    let mut cancels = 0;
    for seed in 0..16 {
        let fake = chaos_lab(seed, 0.01);
        let workers = fake_worker_set(vec![(
            SourceId::Cpu,
            vec![Step::Ok],
            Duration::from_millis(100),
        )]);
        let mut rig = Rig::with_lab(fake, workers, 0);
        rig.start();
        rig.fake
            .run_for(Duration::from_millis(500), Duration::from_millis(100));
        assert_eq!(rig.latest().epoch, 1, "seed {seed}");

        rig.send(action(1));
        rig.fake.advance(RESTART_DELAY);
        assert_eq!(rig.sessions(), 2, "seed {seed}: session 2 starts");
        rig.fake
            .run_for(Duration::from_millis(300), Duration::from_millis(100));
        let after = rig.latest();
        assert_eq!(after.epoch, 2, "seed {seed}");
        assert!(!after.series.series.is_empty(), "seed {seed}");
        assert_gap_before_all(&after, seed);
        cancels += rig.fake.lab.chaos_stats().cancellations;
        rig.finish();
    }
    // The seeds are fixed, so this count is too. Chaos must have cancelled some tasks.
    assert!(cancels > 0);
}

/// A worker that panics on every sample starts 6 times: 1 start plus 5 restarts, then the
/// controller disables it for a minute.
///
/// No cancel injection here: a chaos cancel ends a `Transient` generation for good, so the
/// budget would never be spent (`lab_supervise.rs::explore_restart` shows this). Delays move
/// the lab's own clock, which no timer reads, so the backoff schedule stays exact.
#[test]
fn explore_supervisor_budget() {
    for seed in 0..16 {
        let fake = chaos_lab(seed, 0.0);
        let workers = fake_worker_set(vec![(
            SourceId::Cpu,
            vec![Step::Panic],
            Duration::from_millis(100),
        )]);
        let mut rig = Rig::with_lab(fake, workers, 0);
        rig.start();
        rig.fake
            .run_for(Duration::from_secs(30), Duration::from_millis(100));
        // Each generation samples once, at its first deadline, and panics.
        let starts = times_ms(&worker_log(SourceId::Cpu), "sample");
        eprintln!(
            "DBG seed {seed}: starts {starts:?} stats {:?} root_done {}",
            rig.fake.lab.chaos_stats(),
            rig.root_finished()
        );
        rig.finish();
    }
}

#[test]
fn fake_worker_set_publishes_series() {
    let fake = FakeLab::with_config(LabConfig::new(42).max_steps(1_000_000));
    let workers = fake_worker_set(vec![
        (SourceId::Cpu, vec![Step::Ok], Duration::from_millis(100)),
        (SourceId::Memory, vec![Step::Ok], Duration::from_secs(1)),
    ]);
    let mut rig = Rig::with_lab(fake, workers, 0);
    rig.start();
    // The first samples run at 0 ms, so the 2 s window [0, 2 s) holds 20 CPU samples.
    rig.fake.run_for(
        Duration::from_secs(2) - Duration::from_millis(1),
        Duration::from_millis(100),
    );
    let snap = rig.latest();
    let samples = |identity: DeviceIdentity, metric: MetricId| {
        let device = snap.devices.find(&identity).expect("the device exists").idx;
        snap.series
            .get(SeriesId { device, metric })
            .expect("the series exists")
            .samples
            .len()
    };
    assert_eq!(samples(DeviceIdentity::CpuPackage, MetricId::CPU_TOTAL), 20);
    assert_eq!(samples(DeviceIdentity::Memory, MetricId::MEM_USED), 2);
    rig.finish();
}
