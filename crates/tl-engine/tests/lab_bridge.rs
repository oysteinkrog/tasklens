//! Bridge tests on real threads (ADR-005 verification, ADR-013, E09-018, E09-019).
//!
//! These run on OS threads and the real runtime, not the lab, because they test cross-thread
//! behaviour. A publish costs one atomic store plus one `try_send` on the one-token wake channel.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use asupersync::Cx;
use asupersync::runtime::RuntimeBuilder;
use asupersync::supervision::ManagedGeneration;
use tl_collect::sysroot::SysRoot;
use tl_engine::bridge::{DRAIN_BUDGET, Engine, EngineHooks, HookCtx, TEARDOWN_BUDGET};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::reducer::{ActionGate, AdmissionView, Publisher};
use tl_engine::session::engine_root;
use tl_engine::source::{Source, WorkerIo};
use tl_engine::supervise::WorkerFactory;
use tl_engine::update::{CpuCoreSample, CpuSample, SourceUpdate};
use tl_engine::workers::WorkerSpec;
use tl_model::actions::{ActionId, ActionRequest};
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{MetricId, SeriesId};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::system::{CpuSet, CpuTopology, CpuTotals, Metric};
use tl_model::table::{TableMode, TableQuery};
use tl_model::time::{BootId, TimeNs, now_boot_ns};
use tl_model::validity::Validity;

const PUBLISHES: u64 = 10_000;
/// Command queue size from ADR-005.
const QUEUE: usize = 256;

/// A publisher over a new slot and wake channel, and the UI's ends of both.
fn publisher() -> (
    Publisher,
    Arc<ArcSwap<Snapshot>>,
    async_channel::Receiver<()>,
) {
    let latest = Arc::new(ArcSwap::from_pointee(Snapshot::empty(BootId(1), 1)));
    let (wake_tx, wake_rx) = async_channel::bounded(1);
    let publisher = Publisher {
        latest: Arc::clone(&latest),
        wake_tx,
        stop_publishing: false,
    };
    (publisher, latest, wake_rx)
}

/// Publishes sequences `from..=to`. Each snapshot shares the components of `base`.
fn publish_range(publisher: &mut Publisher, base: &Snapshot, from: u64, to: u64) {
    for sequence in from..=to {
        publisher.publish(Snapshot {
            sequence,
            ..base.clone()
        });
    }
}

#[test]
fn ten_thousand_publishes_monotonic() {
    let (mut publisher, latest, wake_rx) = publisher();
    let base = Snapshot::empty(BootId(1), 1);
    let ui = std::thread::spawn(move || {
        futures_lite::future::block_on(async {
            let mut last = 0;
            let mut wakes = 0u64;
            while last < PUBLISHES {
                wake_rx.recv().await.expect("the publisher sends a token");
                wakes += 1;
                let seq = latest.load().sequence;
                // A token sent after store N can be read after the UI already loaded N, so
                // an equal sequence is fine. A smaller one is not.
                assert!(seq >= last, "sequence went back from {last} to {seq}");
                last = seq;
            }
            (last, wakes)
        })
    });
    // The UI can see sequence 10,000 and drop its receiver before the last `try_send`, so
    // `stop_publishing` may end up set here; it is not checked.
    let producer = std::thread::spawn(move || {
        publish_range(&mut publisher, &base, 1, PUBLISHES);
    });
    producer.join().expect("the publisher thread ends");

    let (done_tx, done_rx) = std_mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(ui.join());
    });
    let (last, wakes) = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the UI loop ends within 5 s")
        .expect("the UI loop does not panic");
    assert_eq!(last, PUBLISHES);
    assert!((1..=PUBLISHES).contains(&wakes), "{wakes} wakes");
}

#[test]
fn publish_never_blocks_without_reader() {
    let (mut publisher, latest, wake_rx) = publisher();
    let base = Snapshot::empty(BootId(1), 1);
    let begin = Instant::now();
    publish_range(&mut publisher, &base, 1, PUBLISHES);
    let took = begin.elapsed();
    assert!(
        took < Duration::from_secs(1),
        "{PUBLISHES} publishes took {took:?}"
    );
    assert_eq!(latest.load().sequence, PUBLISHES);
    // The receiver is alive, so the full channel did not stop publishing.
    assert!(!publisher.stop_publishing);
    assert_eq!(wake_rx.len(), 1);
}

#[test]
fn memory_bounded_one_snapshot_in_slot() {
    let (mut publisher, latest, _wake_rx) = publisher();
    let base = Snapshot::empty(BootId(1), 1);
    publish_range(&mut publisher, &base, 1, 1);
    let first: Weak<Snapshot> = Arc::downgrade(&latest.load_full());
    assert!(first.upgrade().is_some());
    publish_range(&mut publisher, &base, 2, 10);
    assert!(first.upgrade().is_none(), "sequence 1 is still alive");
    assert_eq!(latest.load().sequence, 10);
}

/// Never used: the factory parks the session before the reducer runs.
struct IdleGate;

impl ActionGate for IdleGate {
    fn submit(&mut self, _cx: &Cx, _req: ActionRequest, _view: &AdmissionView<'_>) {}

    fn finished(&mut self, _id: ActionId) -> Option<SourceId> {
        None
    }
}

fn fixture_config(mode: EngineMode) -> EngineConfig {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tl-collect/tests/fixtures/minimal-vm");
    EngineConfig::new(mode, base.join("state")).with_sys_root(SysRoot::at(&base))
}

#[test]
fn command_queue_busy_on_257th() {
    // The gate factory runs on the engine thread before the reducer's first `recv`. Blocking
    // it there parks the reducer, so nothing drains the queue.
    let (parked_tx, parked_rx) = std_mpsc::channel::<()>();
    let (release_tx, release_rx) = std_mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    let hooks = EngineHooks {
        action_gate: Some(Arc::new(move |_ctx: HookCtx| {
            let _ = parked_tx.send(());
            let _ = release_rx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .recv_timeout(Duration::from_secs(10));
            Box::new(IdleGate) as Box<dyn ActionGate>
        })),
        ..EngineHooks::default()
    };
    // Low-memory mode has no workers, so only the test fills the queue.
    let handle =
        Engine::start(fixture_config(EngineMode::LowMemory), hooks).expect("engine starts");
    parked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the session calls the gate factory");
    for i in 0..QUEUE {
        assert_eq!(handle.send(Command::ResetBaselines), Ok(()), "send {i}");
    }
    assert_eq!(
        handle.send(Command::ResetBaselines),
        Err(CommandError::Busy)
    );
    // Unpark before the drop, which joins the engine thread.
    let _ = release_tx.send(());
    drop(handle);
}

/// A CPU worker on the wall clock: every sample is one core at 50 %.
struct WallCpu;

impl Source for WallCpu {
    fn id(&self) -> SourceId {
        SourceId::Cpu
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(Duration::from_millis(100))
    }

    fn sample(self) -> (Self, SourceUpdate) {
        let update = SourceUpdate::Cpu(cpu_sample(now_boot_ns()));
        (self, update)
    }
}

fn cpu_sample(t_ns: TimeNs) -> CpuSample {
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

fn wall_cpu_factory(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        async move { tl_engine::source::run(cx, io, WallCpu).await }
    })
}

/// `Engine::start` always takes `workers_for(config.mode)`, so this builds the runtime with the
/// ADR-004 settings and spawns `engine_root` with a fake `Cpu` worker at 100 ms, as
/// `Engine::start` would. The lab's `fake_worker_set` reads a virtual clock and a thread-local
/// set, so it cannot run on the runtime's threads.
#[test]
fn start_with_fake_workers_publishes() {
    let rt = RuntimeBuilder::new()
        .worker_threads(tl_engine::runtime::WORKER_THREADS)
        .blocking_threads(
            tl_engine::runtime::BLOCKING_THREADS_MIN,
            tl_engine::runtime::BLOCKING_THREADS_MAX,
        )
        .thread_name_prefix(tl_engine::runtime::THREAD_NAME_PREFIX)
        .global_queue_limit(tl_engine::runtime::GLOBAL_QUEUE_LIMIT)
        .build()
        .expect("runtime builds");
    let workers = vec![WorkerSpec {
        id: SourceId::Cpu,
        name: "wall-cpu",
        modes: &[EngineMode::Full],
        factory: wall_cpu_factory,
    }];
    let config = Arc::new(fixture_config(EngineMode::Full));
    let (ep, ui) = fake_sources::root_endpoints(config, workers);
    rt.handle()
        .try_spawn_with_cx(move |cx| engine_root(cx, ep))
        .expect("root spawns");

    let cpu_samples = || {
        let snap = ui.latest.load_full();
        let device = snap.devices.find(&DeviceIdentity::CpuPackage)?.idx;
        let series = snap.series.get(SeriesId {
            device,
            metric: MetricId::CPU_TOTAL,
        })?;
        Some(series.samples.len())
    };
    let begin = Instant::now();
    let mut samples = None;
    while begin.elapsed() < Duration::from_secs(2) {
        samples = cpu_samples();
        if samples.is_some_and(|n| n >= 10) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        samples.is_some_and(|n| n >= 10),
        "CPU_TOTAL has {samples:?} samples after {:?}",
        begin.elapsed()
    );
    drop(ui);
    drop(rt);
}

/// Header rows have neither a record nor a group row (`lab_table.rs::header_counts`).
fn header_rows(snap: &Snapshot) -> usize {
    snap.table
        .rows
        .iter()
        .filter(|row| row.record.is_none() && row.group.is_none())
        .count()
}

/// `Engine::start` on the fixture tree for 2 s of wall time, driven only through
/// `EngineHandle`. `Engine::start` always takes `workers_for(Full)`, so no fake worker can be
/// added: no `Cpu` or `Processes` worker exists yet (E12-A1, E10-A1), and the checks use the
/// memory worker and the table query instead.
#[test]
fn end_to_end_engine_real_runtime() {
    let handle =
        Engine::start(fixture_config(EngineMode::Full), EngineHooks::default()).expect("starts");
    let wake = handle.wake();
    handle.set_demand(Demand {
        memory: true,
        ..Demand::default()
    });
    let send = |mode: TableMode, generation: u64| {
        handle.send(Command::SetTableQuery(TableQuery {
            generation,
            mode,
            ..TableQuery::default()
        }))
    };
    assert_eq!(send(TableMode::Flat, 1), Ok(()));

    let mem_samples = |snap: &Snapshot| {
        let device = snap.devices.find(&DeviceIdentity::Memory)?.idx;
        let series = snap.series.get(SeriesId {
            device,
            metric: MetricId::MEM_USED,
        })?;
        Some(series.samples.len())
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut wakes = 0u32;
    let mut sent_grouped = false;
    let mut prev_sequence = 0;
    // The UI waits on `wake`; polling it every 10 ms keeps this test free of an executor.
    while Instant::now() < deadline {
        if wake.try_recv().is_err() {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        wakes += 1;
        let snap = handle.latest();
        assert!(snap.sequence > prev_sequence, "sequence goes up on wake");
        prev_sequence = snap.sequence;
        if !sent_grouped && snap.table.query_generation == 1 {
            assert_eq!(send(TableMode::Grouped, 2), Ok(()));
            sent_grouped = true;
        }
    }

    let snap = handle.latest();
    assert!(wakes >= 2, "{wakes} wakes in 2 s");
    assert!(sent_grouped, "query generation 1 never published");
    assert_eq!(snap.table.query_generation, 2);
    assert_eq!(header_rows(&snap), 3, "{:?}", snap.table.rows);
    assert!(
        mem_samples(&snap).is_some_and(|n| n >= 2),
        "MEM_USED has {:?} samples",
        mem_samples(&snap)
    );
    assert_eq!(
        snap.health.get(SourceId::Memory).state,
        SourceState::Running
    );

    drop(wake);
    // A clean report means teardown joined the runtime's worker and blocking threads. Other
    // tests of this binary start threads at the same time, and the logind worker leaves the
    // process-wide zbus and async-io threads running, so the thread count is not checked.
    let report = handle.shutdown(Duration::from_secs(2));
    assert!(report.clean(), "{report:?}");
}

// Shutdown (E09-A2) --------------------------------------------------------------------------

fn wall_cpu_spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Cpu,
        name: "wall-cpu",
        modes: &[EngineMode::Full],
        factory: wall_cpu_factory,
    }
}

/// Starts the engine with the fake `Cpu` worker and waits until its samples reach a snapshot.
fn start_wall_cpu() -> tl_engine::EngineHandle {
    let handle = Engine::start_with_workers(
        fixture_config(EngineMode::Full),
        EngineHooks::default(),
        vec![wall_cpu_spec()],
    )
    .expect("engine starts");
    let begin = Instant::now();
    while handle.latest().sequence < 2 {
        assert!(
            begin.elapsed() < Duration::from_secs(5),
            "no publish within 5 s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    handle
}

#[test]
fn shutdown_clean_with_fake_workers() {
    let handle = start_wall_cpu();
    let report = handle.shutdown(Duration::from_secs(2));
    assert!(report.drained, "{report:?}");
    assert!(report.torn_down, "{report:?}");
    assert!(report.clean());
    assert!(
        report.elapsed < DRAIN_BUDGET + TEARDOWN_BUDGET,
        "{report:?}"
    );
}

/// Set when `BlockedCpu::sample` starts. Only `shutdown_times_out_on_blocked_job` reads it.
static BLOCKED_STARTED: AtomicBool = AtomicBool::new(false);

/// A CPU worker whose first sample blocks its blocking-pool thread for 3 s.
struct BlockedCpu;

impl Source for BlockedCpu {
    fn id(&self) -> SourceId {
        SourceId::Cpu
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(Duration::from_millis(100))
    }

    fn sample(self) -> (Self, SourceUpdate) {
        BLOCKED_STARTED.store(true, Ordering::Release);
        std::thread::sleep(Duration::from_secs(3));
        let update = SourceUpdate::Cpu(cpu_sample(now_boot_ns()));
        (self, update)
    }
}

fn blocked_cpu_factory(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        async move { tl_engine::source::run(cx, io, BlockedCpu).await }
    })
}

#[test]
fn shutdown_times_out_on_blocked_job() {
    let handle = Engine::start_with_workers(
        fixture_config(EngineMode::Full),
        EngineHooks::default(),
        vec![WorkerSpec {
            id: SourceId::Cpu,
            name: "blocked-cpu",
            modes: &[EngineMode::Full],
            factory: blocked_cpu_factory,
        }],
    )
    .expect("engine starts");
    let begin = Instant::now();
    while !BLOCKED_STARTED.load(Ordering::Acquire) {
        assert!(
            begin.elapsed() < Duration::from_secs(5),
            "the blocking job did not start within 5 s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let report = handle.shutdown(Duration::from_secs(2));
    // The job is not interrupted. Teardown goes on in the background and the process would
    // exit anyway.
    assert!(!report.torn_down, "{report:?}");
    assert!(!report.clean());
    assert!(report.elapsed < Duration::from_millis(1600), "{report:?}");
}

#[test]
fn shutdown_not_torn_down_with_live_clone() {
    let handle = start_wall_cpu();
    let clone = handle.clone();
    let report = handle.shutdown(Duration::from_secs(2));
    // The clone still holds `Inner` and with it a `RuntimeHandle` (API notes section 8).
    assert!(!report.torn_down, "{report:?}");
    assert!(
        report.elapsed < DRAIN_BUDGET + TEARDOWN_BUDGET,
        "{report:?}"
    );
    // The clone sees the stopped engine.
    assert_eq!(
        clone.send(Command::ResetBaselines),
        Err(CommandError::EngineStopped)
    );
    assert!(clone.action_events().is_closed());
    // Dropping the clone lets the background teardown finish.
    drop(clone);
}
