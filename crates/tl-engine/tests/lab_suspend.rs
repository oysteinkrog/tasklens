//! LabRuntime tests for suspend and resume in the reducer (E09-008, ADR-008, ADR-013).
//!
//! Each `run` call gets its own task. The messages are queued before the task starts and the
//! sender is dropped, so `run` drains the queue and then returns.

use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::reducer::{InvariantViolation, Publisher, Reducer};
use tl_engine::source::{WorkerCommand, WorkerControl};
use tl_engine::update::{CpuCoreSample, CpuSample, EngineMsg, SourceUpdate};
use tl_model::actions::ActionEvent;
use tl_model::health::SourceId;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SampleFlags};
use tl_model::snapshot::Snapshot;
use tl_model::system::{CpuSet, CpuTopology, CpuTotals, Metric};
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::Validity;

const BOOT: BootId = BootId(0x1234);
const MS: TimeNs = 1_000_000;
const SEC: TimeNs = 1_000_000_000;
const CPU_TOTAL: SeriesId = SeriesId {
    device: DeviceIdx(0),
    metric: MetricId::CPU_TOTAL,
};

struct Rig {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    reducer: Option<Reducer>,
    publisher: Option<Publisher>,
    _wake_rx: async_channel::Receiver<()>,
    _events_rx: async_channel::Receiver<ActionEvent>,
    control_rx: watch::Receiver<WorkerControl>,
}

type RunOutput = Arc<Mutex<Option<(Reducer, Publisher, Result<(), InvariantViolation>)>>>;

impl Rig {
    fn new() -> Self {
        let (control_tx, control_rx) = watch::channel(WorkerControl::default());
        let (events_tx, events_rx) = async_channel::bounded(64);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let publisher = Publisher {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
            wake_tx,
            stop_publishing: false,
        };
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(1_000_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        Rig {
            lab,
            root,
            reducer: Some(Reducer::new(1, BOOT, control_tx, events_tx)),
            publisher: Some(publisher),
            _wake_rx: wake_rx,
            _events_rx: events_rx,
            control_rx,
        }
    }

    /// Queues `msgs`, drops the sender and runs the loop until it returns.
    fn run(&mut self, msgs: Vec<EngineMsg>) {
        let (tx, mut rx) = mpsc::channel::<EngineMsg>(msgs.len().max(1));
        for msg in msgs {
            tx.try_send(msg)
                .expect("the channel has room for every message");
        }
        drop(tx);
        let mut reducer = self.reducer.take().expect("one run at a time");
        let mut publisher = self.publisher.take().expect("one run at a time");
        let out: RunOutput = Arc::default();
        let slot = Arc::clone(&out);
        let (task, _handle) = self
            .lab
            .state
            .create_task(self.root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let result = reducer.run(&cx, &mut rx, &mut publisher).await;
                *slot.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some((reducer, publisher, result));
            })
            .expect("task is created");
        self.lab.scheduler.lock().schedule(task, 0);
        self.lab.run_until_idle();
        let (reducer, publisher, result) = out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("run returned");
        self.reducer = Some(reducer);
        self.publisher = Some(publisher);
        result.expect("no invariant violation");
    }

    fn latest(&self) -> Arc<Snapshot> {
        self.publisher
            .as_ref()
            .expect("the publisher is back from the last run")
            .latest
            .load_full()
    }

    fn samples(&self, id: SeriesId) -> Vec<Sample> {
        self.latest()
            .series
            .get(id)
            .expect("series exists")
            .samples
            .to_vec()
    }

    fn last_sample(&self, id: SeriesId) -> Sample {
        *self.samples(id).last().expect("series has a sample")
    }

    fn reset_generation(&self) -> u64 {
        self.control_rx.borrow_and_clone().reset_generation
    }

    /// Targets of the forwarded `RefreshNow` commands.
    fn refreshed(&self) -> Vec<SourceId> {
        self.control_rx
            .borrow_and_clone()
            .commands
            .iter()
            .filter(|c| c.command == WorkerCommand::RefreshNow)
            .map(|c| c.target)
            .collect()
    }
}

fn src(update: SourceUpdate) -> EngineMsg {
    EngineMsg::Source(update)
}

fn sleep(entering: bool, t_ns: TimeNs) -> EngineMsg {
    src(SourceUpdate::Sleep { entering, t_ns })
}

fn clock_resume(id: SourceId, t_ns: TimeNs) -> EngineMsg {
    src(SourceUpdate::ResumeDetected { id, t_ns })
}

fn cpu_msg(t_ns: TimeNs, pct: f32, validity: Validity) -> EngineMsg {
    let mut online = CpuSet::empty();
    online.insert(0);
    let util_pct = if validity == Validity::Valid {
        Metric::valid(pct)
    } else {
        Metric::with(validity)
    };
    src(SourceUpdate::Cpu(CpuSample {
        t_ns,
        pressure_t_ns: t_ns,
        totals: CpuTotals {
            util_pct,
            kernel_pct: util_pct,
            ..CpuTotals::default()
        },
        irq_pct: 0.0,
        per_cpu: vec![CpuCoreSample {
            cpu: 0,
            pct,
            kernel_pct: 0.0,
            freq_mhz: Some(3000),
            validity,
        }],
        freq_mhz: Some(3000),
        temp_c: None,
        temp_validity: Validity::Unsupported,
        topology: Some(Arc::new(CpuTopology {
            online,
            cores: Arc::from([]),
        })),
        validity,
    }))
}

fn valid(s: &Sample) -> bool {
    s.flags.contains(SampleFlags::VALID)
}

/// The effects of one resume, checked after `resume` was applied to a rig that has CPU series.
fn assert_resumed(rig: &mut Rig, before: u64, resume: EngineMsg) {
    rig.run(vec![resume]);
    assert_eq!(rig.reset_generation(), before + 1);
    let refreshed = rig.refreshed();
    for id in [SourceId::Disk, SourceId::Net, SourceId::Sensors] {
        assert!(refreshed.contains(&id), "{id:?} got no RefreshNow");
    }
    assert!(!refreshed.contains(&SourceId::GpuNvidia));
    rig.run(vec![cpu_msg(20 * SEC, 10.0, Validity::Valid)]);
    let snap = rig.latest();
    assert!(!snap.series.series.is_empty());
    for view in snap.series.series.iter() {
        let last = view.samples.last().expect("a sample");
        assert!(
            last.flags.contains(SampleFlags::GAP_BEFORE),
            "{:?} has no gap",
            view.id
        );
    }
}

#[test]
fn sleep_pauses_rates() {
    let mut rig = Rig::new();
    rig.run(vec![cpu_msg(SEC, 40.0, Validity::Valid)]);
    assert!(valid(&rig.last_sample(CPU_TOTAL)));
    rig.run(vec![
        sleep(true, 2 * SEC),
        cpu_msg(2 * SEC, 40.0, Validity::Valid),
        cpu_msg(3 * SEC, 40.0, Validity::Valid),
    ]);
    let samples = rig.samples(CPU_TOTAL);
    assert_eq!(samples.len(), 3);
    assert!(!valid(&samples[1]));
    assert!(!valid(&samples[2]));
}

#[test]
fn resume_resets_and_gaps_and_reenumerates() {
    let mut rig = Rig::new();
    rig.run(vec![
        cpu_msg(SEC, 40.0, Validity::Valid),
        sleep(true, 2 * SEC),
    ]);
    let before = rig.reset_generation();
    assert!(rig.refreshed().is_empty());
    assert_resumed(&mut rig, before, sleep(false, 10 * SEC));
    assert!(valid(&rig.last_sample(CPU_TOTAL)), "rates flow again");
}

#[test]
fn clock_resume_from_two_workers_counts_once() {
    let mut rig = Rig::new();
    rig.run(vec![cpu_msg(SEC, 40.0, Validity::Valid)]);
    let before = rig.reset_generation();
    rig.run(vec![
        clock_resume(SourceId::Cpu, 10 * SEC),
        clock_resume(SourceId::Memory, 10 * SEC + 50 * MS),
    ]);
    assert_eq!(rig.reset_generation(), before + 1);
    let refreshed = rig.refreshed();
    assert_eq!(
        refreshed.iter().filter(|&&id| id == SourceId::Disk).count(),
        1
    );
}

#[test]
fn clock_resume_without_logind() {
    let mut rig = Rig::new();
    rig.run(vec![cpu_msg(SEC, 40.0, Validity::Valid)]);
    let before = rig.reset_generation();
    assert_resumed(&mut rig, before, clock_resume(SourceId::Cpu, 10 * SEC));
}

#[test]
fn no_spike_after_resume() {
    let mut rig = Rig::new();
    rig.run(vec![
        cpu_msg(SEC, 20.0, Validity::Valid),
        cpu_msg(2 * SEC, 20.0, Validity::Valid),
    ]);
    // The CPU worker saw a 5 s boottime gap against a 10 ms monotonic gap: it reports the
    // resume, then a `WarmingUp` sample with no rate.
    rig.run(vec![
        clock_resume(SourceId::Cpu, 7 * SEC),
        cpu_msg(7 * SEC, 0.0, Validity::WarmingUp),
    ]);
    let first = rig.last_sample(CPU_TOTAL);
    assert!(first.flags.contains(SampleFlags::GAP_BEFORE));
    assert!(!valid(&first));
    rig.run(vec![cpu_msg(8 * SEC, 20.0, Validity::Valid)]);
    let samples = rig.samples(CPU_TOTAL);
    let peak = samples
        .iter()
        .filter(|s| valid(s))
        .map(|s| s.value)
        .fold(0.0f32, f32::max);
    assert_eq!(peak, 20.0, "no rate spike");
}
