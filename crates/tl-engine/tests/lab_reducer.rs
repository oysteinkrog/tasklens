//! LabRuntime tests for the reducer loop and publish (E09-001, ADR-004, ADR-005, ADR-013).
//!
//! Each `run` call gets its own task. The messages are queued before the task starts, and the
//! sender is dropped unless a test keeps it, so `run` drains the queue and then returns.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::reducer::{DRAIN_MAX, InvariantViolation, Publisher, Reducer};
use tl_engine::source::WorkerControl;
use tl_engine::update::{EngineMsg, ProcessSweep, SourceUpdate};
use tl_model::actions::ActionEvent;
use tl_model::devices::DeviceCatalog;
use tl_model::health::{SourceId, SourceState};
use tl_model::history::RecordSink;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::process::ProcessRecord;
use tl_model::series::{Sample, SampleFlags};
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::Validity;

const BOOT: BootId = BootId(0x1234);
const CPU_TOTAL: SeriesId = SeriesId {
    device: DeviceIdx(0),
    metric: MetricId::CPU_TOTAL,
};

struct Rig {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    reducer: Option<Reducer>,
    publisher: Option<Publisher>,
    wake_rx: Option<async_channel::Receiver<()>>,
    _control_rx: watch::Receiver<WorkerControl>,
    _events_rx: async_channel::Receiver<ActionEvent>,
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
            wake_rx: Some(wake_rx),
            _control_rx: control_rx,
            _events_rx: events_rx,
        }
    }

    fn reducer(&mut self) -> &mut Reducer {
        self.reducer
            .as_mut()
            .expect("the reducer is back from the last run")
    }

    fn latest(&self) -> Arc<Snapshot> {
        self.publisher
            .as_ref()
            .expect("the publisher is back from the last run")
            .latest
            .load_full()
    }

    /// Queues `msgs`, drops the sender and runs the loop until it returns.
    fn run(&mut self, msgs: Vec<EngineMsg>) -> Result<(), InvariantViolation> {
        let (tx, rx) = mpsc::channel::<EngineMsg>(msgs.len().max(1));
        for msg in msgs {
            tx.try_send(msg)
                .expect("the channel has room for every message");
        }
        drop(tx);
        let out = self.spawn_run(rx);
        self.finish(&out)
    }

    fn spawn_run(&mut self, mut rx: mpsc::Receiver<EngineMsg>) -> RunOutput {
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
        out
    }

    fn finish(&mut self, out: &RunOutput) -> Result<(), InvariantViolation> {
        let (reducer, publisher, result) = out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("run returned");
        self.reducer = Some(reducer);
        self.publisher = Some(publisher);
        result
    }
}

fn health_msg(i: u64) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Health {
        id: SourceId::Cpu,
        state: SourceState::Running,
        last_error: None,
        t_ns: i,
    })
}

fn health_msgs(n: u64) -> Vec<EngineMsg> {
    (0..n).map(health_msg).collect()
}

fn sweep_msg(t_ns: TimeNs) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Processes(ProcessSweep {
        observed_start: t_ns,
        observed_end: t_ns,
        records: Vec::new(),
        extras: Vec::new(),
        extras_p2: Vec::new(),
        app_stats: Vec::new(),
        unit_stats: Vec::new(),
        system_threads: 0,
        file_nr: None,
        online_cpus: 1,
        low_memory: None,
    }))
}

#[test]
fn coalesces_100_messages_into_one_publish() {
    let mut rig = Rig::new();
    let before = rig.latest().sequence;
    rig.run(health_msgs(100)).expect("no invariant violation");
    assert_eq!(rig.latest().sequence, before + 1);
    let wake = rig.wake_rx.as_ref().expect("receiver kept");
    assert_eq!(wake.len(), 1, "one wake token");
}

#[test]
fn drain_cap_256_splits_publishes() {
    assert_eq!(DRAIN_MAX, 256);
    let mut rig = Rig::new();
    let before = rig.latest().sequence;
    rig.run(health_msgs(300)).expect("no invariant violation");
    assert_eq!(rig.latest().sequence, before + 2);
    assert_eq!(rig.reducer().sequence, before + 2);
}

#[test]
fn cpu_only_change_reuses_other_arcs() {
    let mut rig = Rig::new();
    rig.run(vec![health_msg(0)])
        .expect("no invariant violation");
    let prev = rig.latest();

    let r = rig.reducer();
    r.series.append(
        CPU_TOTAL,
        1_000_000_000,
        50.0,
        100_000_000,
        SampleFlags::VALID,
    );
    r.mark(ComponentMask::SERIES);
    rig.run(vec![health_msg(1)])
        .expect("no invariant violation");
    let next = rig.latest();

    assert_eq!(next.sequence, prev.sequence + 1);
    assert_eq!(next.series.header.version, prev.series.header.version + 1);
    assert!(!Arc::ptr_eq(&next.series, &prev.series));
    assert!(next.series.get(CPU_TOTAL).is_some());
    assert!(Arc::ptr_eq(&next.system, &prev.system));
    assert!(Arc::ptr_eq(&next.processes, &prev.processes));
    assert!(Arc::ptr_eq(&next.table, &prev.table));
    assert!(Arc::ptr_eq(&next.connections, &prev.connections));
    assert!(Arc::ptr_eq(&next.services, &prev.services));
    assert!(Arc::ptr_eq(&next.startup, &prev.startup));
    assert!(Arc::ptr_eq(&next.inventory, &prev.inventory));
    assert!(Arc::ptr_eq(&next.devices, &prev.devices));
    assert!(Arc::ptr_eq(&next.health, &prev.health));
    assert!(Arc::ptr_eq(&next.windows, &prev.windows));
    assert!(Arc::ptr_eq(&next.details, &prev.details));
    assert!(Arc::ptr_eq(&next.history, &prev.history));
    assert!(Arc::ptr_eq(&next.process_series, &prev.process_series));
    assert!(Arc::ptr_eq(&next.device_details, &prev.device_details));
    let mut changed = smallvec::SmallVec::new();
    assert_eq!(
        next.changed_since(&prev, &mut changed),
        ComponentMask::SERIES
    );
}

#[test]
fn fresh_windows_component_is_unsupported() {
    let mut rig = Rig::new();
    rig.run(vec![health_msg(0)])
        .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.windows.header.validity, Validity::Unsupported);
    assert_eq!(snap.system.header.validity, Validity::WarmingUp);
    assert_eq!((snap.epoch, snap.boot_id), (1, BOOT));
}

#[test]
fn publish_never_blocks_without_reader() {
    let mut rig = Rig::new();
    let mut publisher = rig.publisher.take().expect("publisher");
    for sequence in 1..=10_000 {
        let mut next = Snapshot::empty(BOOT, 1);
        next.sequence = sequence;
        publisher.publish(next);
    }
    assert_eq!(publisher.latest.load().sequence, 10_000);
    assert!(!publisher.stop_publishing);
    let wake = rig.wake_rx.as_ref().expect("receiver kept");
    assert_eq!(wake.len(), 1, "the wake channel holds one token");
}

#[test]
fn closed_wake_stops_publishing() {
    let mut rig = Rig::new();
    drop(rig.wake_rx.take());
    let (tx, rx) = mpsc::channel::<EngineMsg>(4);
    tx.try_send(health_msg(0)).expect("room");
    // The sender stays open, so only the closed wake channel can end the loop.
    let out = rig.spawn_run(rx);
    let result = rig.finish(&out);
    assert!(result.is_ok());
    let publisher = rig.publisher.as_ref().expect("publisher");
    assert!(publisher.stop_publishing);
    assert_eq!(publisher.latest.load().sequence, 1);
    drop(tx);
}

/// Counts each hook call: system samples, process sweeps, device catalogs.
struct CountingSink(Arc<[AtomicU32; 3]>);

impl RecordSink for CountingSink {
    fn on_system_sample(&mut self, _t_ns: TimeNs, _series: &[(SeriesId, Sample)]) {
        self.0[0].fetch_add(1, Ordering::Relaxed);
    }

    fn on_process_sweep(&mut self, _t_ns: TimeNs, _records: &Arc<[ProcessRecord]>) {
        self.0[1].fetch_add(1, Ordering::Relaxed);
    }

    fn on_device_catalog(&mut self, _catalog: &DeviceCatalog) {
        self.0[2].fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn record_sink_called_when_installed() {
    // Without a sink a sweep still applies and publishes.
    let mut rig = Rig::new();
    assert!(rig.reducer().record_sink.is_none());
    rig.run(vec![sweep_msg(1)]).expect("no invariant violation");
    assert_eq!(rig.latest().processes.header.version, 1);

    let counts: Arc<[AtomicU32; 3]> = Arc::default();
    let mut rig = Rig::new();
    rig.reducer().record_sink = Some(Box::new(CountingSink(Arc::clone(&counts))));
    rig.run(vec![sweep_msg(1), sweep_msg(2)])
        .expect("no invariant violation");
    assert_eq!(counts[1].load(Ordering::Relaxed), 2, "once per sweep");
    assert_eq!(counts[0].load(Ordering::Relaxed), 0);
    assert_eq!(counts[2].load(Ordering::Relaxed), 0);
}
