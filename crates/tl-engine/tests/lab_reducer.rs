//! LabRuntime tests for the reducer loop and publish (E09-001, ADR-004, ADR-005, ADR-013).
//!
//! Each `run` call gets its own task. The messages are queued before the task starts, and the
//! sender is dropped unless a test keeps it, so `run` drains the queue and then returns.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use smallvec::SmallVec;
use tl_collect::SysRoot;
use tl_collect::counters::Clock;
use tl_engine::reducer::{DRAIN_MAX, InvariantViolation, Publisher, Reducer};
use tl_engine::source::{WorkerCommand, WorkerControl};
use tl_engine::update::{
    ConnectionsUpdate, CpuCoreSample, CpuSample, DetailsUpdate, DeviceDesc, DeviceDetailsUpdate,
    DeviceKinds, DeviceList, DiskSample, DrmProcessVram, EngineMsg, GpuInfoUpdate, GpuSample,
    GpuStaticInfo, HelperUpdate, MemorySample, NetSample, ProcessSweep, PssEntry, SensorSample,
    ServicesUpdate, SourceUpdate, StartupUpdate,
};
use tl_engine::workers::cpu::CpuSource;
use tl_model::actions::{
    ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, RejectReason, SchedPolicy,
    Signal,
};
use tl_model::command::{Command, DeviceCommand, HistoryWhat, KWinCommand, SetupStep};
use tl_model::demand::{Demand, DetailDemand};
use tl_model::details::{
    AffinityInfo, DetailTab, DetailTarget, EnvDetail, HandlePhase, HandleSearchState, ScanId,
    SearchId, StringScanOpts, TabPayload,
};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity, NetKind};
use tl_model::gpu_proto::{ApiVersions, GpuProcessSample, Reading};
use tl_model::health::{HelperState, ManagerState, SourceId, SourceState};
use tl_model::history::{
    HistoryMetric, HistoryPoint, HistoryRange, HistorySeries, HistoryView, RecordSink,
};
use tl_model::ids::{ColumnId, DeviceIdx, GroupKey, MetricId, ProcessKey, SeriesId};
use tl_model::process::{ProcFlags, ProcState, ProcessExtras, ProcessMeta, ProcessRecord};
use tl_model::series::{Sample, SampleFlags};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{BatteryDetails, ComponentMask, DiskDetails, ServiceBackend, Snapshot};
use tl_model::system::{
    CpuSet, CpuTopology, CpuTotals, DiskTotals, GpuTotals, LinkState, MemoryTotals, Metric,
    NetTotals,
};
use tl_model::table::{
    Manager, SysColumnId, SysSortSpec, SysTable, SysTableQuery, TableQuery, Total,
};
use tl_model::time::{BootId, TimeNs, now_boot_ns};
use tl_model::validity::{FieldValidity, ProcessField, Validity};

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
    control_rx: watch::Receiver<WorkerControl>,
    events_rx: async_channel::Receiver<ActionEvent>,
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
            control_rx,
            events_rx,
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

/// What a `LogSink` saw: each system batch, the sweep count and each catalog's device count.
#[derive(Default)]
struct SinkLog {
    system: Vec<(TimeNs, Vec<(SeriesId, Sample)>)>,
    sweeps: u32,
    catalogs: Vec<usize>,
}

struct LogSink(Arc<Mutex<SinkLog>>);

impl LogSink {
    fn install(rig: &mut Rig) -> Arc<Mutex<SinkLog>> {
        let log: Arc<Mutex<SinkLog>> = Arc::default();
        rig.reducer()
            .set_record_sink(Some(Box::new(LogSink(Arc::clone(&log)))));
        log
    }
}

impl RecordSink for LogSink {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]) {
        let mut log = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        log.system.push((t_ns, series.to_vec()));
    }

    fn on_process_sweep(&mut self, _t_ns: TimeNs, _records: &Arc<[ProcessRecord]>) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).sweeps += 1;
    }

    fn on_device_catalog(&mut self, catalog: &DeviceCatalog) {
        let mut log = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        log.catalogs.push(catalog.entries.len());
    }
}

/// A valid CPU sample with `cores` logical CPUs: 4 package series and 3 per core.
fn cpu_cores_msg(t_ns: TimeNs, cores: u16) -> EngineMsg {
    src(SourceUpdate::Cpu(CpuSample {
        t_ns,
        totals: CpuTotals {
            util_pct: Metric::valid(10.0),
            kernel_pct: Metric::valid(5.0),
            ..CpuTotals::default()
        },
        irq_pct: 0.0,
        per_cpu: (0..cores)
            .map(|cpu| CpuCoreSample {
                cpu,
                pct: 10.0,
                kernel_pct: 5.0,
                freq_mhz: Some(3000),
                validity: Validity::Valid,
            })
            .collect(),
        freq_mhz: Some(3000),
        temp_c: None,
        temp_validity: Validity::Unsupported,
        topology: None,
        validity: Validity::Valid,
    }))
}

#[test]
fn sink_system_sample_batches() {
    let mut rig = Rig::new();
    let log = LogSink::install(&mut rig);
    // 7 package series (total, kernel, frequency, temperature, package and core power, fastest
    // core) and 3 per core.
    let cores = 8;
    let expected = 7 + 3 * usize::from(cores);
    rig.run(vec![cpu_cores_msg(SEC, cores)])
        .expect("no invariant violation");
    {
        let log = log.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(log.system.len(), 1, "one call per batch");
        let (t, batch) = &log.system[0];
        assert_eq!(*t, SEC);
        assert_eq!(batch.len(), expected);
        let snap = rig.latest();
        for (id, sample) in batch {
            let view = snap.series.get(*id).expect("the series was appended");
            assert_eq!(view.samples.last(), Some(sample));
        }
    }

    // Another batch kind is its own call with only its own samples: 4 disk, 3 aggregate.
    rig.run(vec![disks_msg(
        2 * SEC,
        vec![(disk_id("nvme0n1", 0), disk_totals(10.0, 1e6))],
    )])
    .expect("no invariant violation");
    let log = log.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(log.system.len(), 2);
    assert_eq!(log.system[1].0, 2 * SEC);
    assert_eq!(log.system[1].1.len(), 4 + 3);
}

#[test]
fn sink_device_catalog() {
    let mut rig = Rig::new();
    rig.run(vec![health_msg(0)])
        .expect("no invariant violation");
    let log = LogSink::install(&mut rig);
    let disk = disk_id("sda", 1);
    rig.run(vec![src(SourceUpdate::Devices(DeviceList {
        t_ns: SEC,
        kinds: DeviceKinds::DISKS,
        entries: vec![DeviceDesc {
            identity: disk.clone(),
            display_name: Arc::from("sda"),
            parent: None,
            persistent_key: Arc::from("disk:sda"),
            net_kind: None,
        }],
    }))])
    .expect("no invariant violation");
    assert!(rig.latest().devices.find(&disk).is_some());
    let log = log.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        log.catalogs,
        vec![rig.latest().devices.entries.len()],
        "one call with the published catalog"
    );
    assert!(log.system.is_empty());
}

#[test]
fn history_component_replaced() {
    let mut rig = Rig::new();
    rig.run(vec![health_msg(0)])
        .expect("no invariant violation");
    let prev = rig.latest();
    let points: Arc<[HistoryPoint]> = (0..1024u64)
        .map(|i| HistoryPoint {
            t_ns: i * SEC,
            min: 1.0,
            max: 2.0,
        })
        .collect();
    let series: Arc<[HistorySeries]> = [HistoryMetric::Cpu, HistoryMetric::Memory]
        .into_iter()
        .map(|metric| HistorySeries {
            key: None,
            metric,
            points: Arc::clone(&points),
        })
        .collect();
    let view = Arc::new(HistoryView {
        query_id: 7,
        series: Arc::clone(&series),
        ..HistoryView::default()
    });
    rig.run(vec![src(SourceUpdate::History(Arc::clone(&view)))])
        .expect("no invariant violation");
    let next = rig.latest();
    // The reducer owns the header version, so the view's payload is published, not its `Arc`.
    assert!(Arc::ptr_eq(&next.history.series, &series));
    assert_eq!(next.history.series.len(), 2);
    assert!(next.history.series.iter().all(|s| s.points.len() == 1024));
    assert_eq!(next.history.query_id, 7);
    assert_eq!(next.history.header.validity, Validity::Valid);
    assert_eq!(next.history.header.version, prev.history.header.version + 1);
    let mut changed = SmallVec::new();
    assert_eq!(
        next.changed_since(&prev, &mut changed),
        ComponentMask::HISTORY
    );
}

#[test]
fn sink_removed_stops_calls() {
    let mut rig = Rig::new();
    let log = LogSink::install(&mut rig);
    rig.run(vec![sweep_msg(SEC), cpu_cores_msg(SEC, 2)])
        .expect("no invariant violation");
    let calls = |log: &Arc<Mutex<SinkLog>>| {
        let log = log.lock().unwrap_or_else(PoisonError::into_inner);
        (log.system.len(), log.sweeps, log.catalogs.len())
    };
    let (system, sweeps, catalogs) = calls(&log);
    assert_eq!((system, sweeps), (1, 1));
    assert!(catalogs >= 1, "the first CPU sample adds its cores");

    rig.reducer().set_record_sink(None);
    assert!(rig.reducer().record_sink.is_none());
    rig.run(vec![
        sweep_msg(2 * SEC),
        cpu_cores_msg(2 * SEC, 4),
        disks_msg(
            2 * SEC,
            vec![(disk_id("nvme1n1", 2), disk_totals(1.0, 1.0))],
        ),
    ])
    .expect("no invariant violation");
    assert_eq!(calls(&log), (system, sweeps, catalogs));
    assert_eq!(rig.latest().processes.header.version, 2);
}

// ---- apply.rs (E09-007) ----

const MS: TimeNs = 1_000_000;

fn src(update: SourceUpdate) -> EngineMsg {
    EngineMsg::Source(update)
}

fn cmd(command: Command) -> EngineMsg {
    EngineMsg::Command(command)
}

fn cpus(n: u16) -> CpuSet {
    let mut set = CpuSet::empty();
    for id in 0..n {
        set.insert(id);
    }
    set
}

fn cpu_msg(t_ns: TimeNs, pct: f32, topology: bool) -> EngineMsg {
    let topology = topology.then(|| {
        Arc::new(CpuTopology {
            online: cpus(2),
            cores: Arc::from([]),
        })
    });
    src(SourceUpdate::Cpu(CpuSample {
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
        topology,
        validity: Validity::Valid,
    }))
}

fn disk_id(name: &str, minor: u32) -> DeviceIdentity {
    DeviceIdentity::Disk {
        major: 259,
        minor,
        name: Arc::from(name),
        wwn_or_serial: None,
    }
}

fn net_id(name: &str, ifindex: u32) -> DeviceIdentity {
    DeviceIdentity::Net {
        netns_ino: 1,
        ifindex,
        name: Arc::from(name),
    }
}

fn disk_totals(active: f32, read: f64) -> DiskTotals {
    DiskTotals {
        device: DeviceIdx(0),
        active_pct: Metric::valid(active),
        read_bps: Metric::valid(read),
        write_bps: Metric::valid(0.0),
        response_ms: Metric::valid(1.0),
        read_total: 0,
        written_total: 0,
        capacity: 0,
        temp_c: Metric::with(Validity::Unsupported),
    }
}

fn net_totals(rx: f64) -> NetTotals {
    NetTotals {
        device: DeviceIdx(0),
        rx_bps: Metric::valid(rx),
        tx_bps: Metric::valid(0.0),
        rx_total: 0,
        tx_total: 0,
        link_speed_mbps: Metric::with(Validity::Unsupported),
        state: LinkState::Up,
    }
}

fn disks_msg(t_ns: TimeNs, disks: Vec<(DeviceIdentity, DiskTotals)>) -> EngineMsg {
    src(SourceUpdate::Disks(DiskSample { t_ns, disks }))
}

fn battery(native_path: &str, pct: f32) -> BatteryDetails {
    BatteryDetails {
        kind: Arc::from("Battery"),
        vendor: Total::default(),
        model: Total::default(),
        serial: Total::default(),
        native_path: Arc::from(native_path),
        percentage: Total {
            value: pct,
            validity: Validity::Valid,
        },
        energy_wh: Total::default(),
        energy_rate_w: Total::default(),
        voltage_v: Total::default(),
        state: Arc::from("Discharging"),
        time_to_full_s: Total::default(),
        time_to_empty_s: Total::default(),
        charge_cycles: Total::default(),
        technology: Arc::from("Li-ion"),
        capacity_pct: Total::default(),
        energy_empty_wh: Total::default(),
        energy_full_wh: Total::default(),
        energy_full_design_wh: Total::default(),
        voltage_min_design_v: Total::default(),
        voltage_max_design_v: Total::default(),
        power_supply: true,
        charge_threshold: None,
        temperature_c: Total::default(),
        history: None,
    }
}

fn text(v: &str) -> Total<Arc<str>> {
    Total {
        value: Arc::from(v),
        validity: Validity::Valid,
    }
}

impl Rig {
    fn idx(&self, id: &DeviceIdentity) -> DeviceIdx {
        self.latest()
            .devices
            .find(id)
            .map(|e| e.idx)
            .expect("device in the catalog")
    }

    fn last_sample(&self, id: SeriesId) -> Sample {
        *self
            .latest()
            .series
            .get(id)
            .expect("series exists")
            .samples
            .last()
            .expect("series has a sample")
    }

    fn commands(&self) -> Arc<[tl_engine::source::QueuedCommand]> {
        self.control_rx.borrow_and_clone().commands
    }
}

#[test]
fn cpu_update_appends_series() {
    let mut rig = Rig::new();
    rig.run(vec![cpu_msg(100 * MS, 50.0, true)])
        .expect("no invariant violation");
    let mut prev = rig.latest();
    for step in 2..=5u64 {
        rig.run(vec![cpu_msg(step * 100 * MS, 50.0, false)])
            .expect("no invariant violation");
        let next = rig.latest();
        assert_eq!(next.series.header.version, prev.series.header.version + 1);
        let view = next.series.get(CPU_TOTAL).expect("CPU_TOTAL exists");
        assert_eq!(view.samples.len() as u64, step);
        let last = view.samples.last().expect("a sample");
        assert_eq!(last.value, 50.0);
        assert!(last.flags.contains(SampleFlags::VALID));
        assert!(!last.flags.contains(SampleFlags::GAP_BEFORE));
        prev = next;
    }
    let core = rig.idx(&DeviceIdentity::CpuLogical { id: 1 });
    assert_eq!(
        rig.last_sample(SeriesId {
            device: core,
            metric: MetricId::CPU_TOTAL
        })
        .value,
        50.0
    );
    assert_eq!(rig.latest().system.cpu_topology.online, cpus(2));
}

/// A CPU sample with the secondary chart's values: package and core watts and the fastest core.
fn cpu_secondary_msg(t_ns: TimeNs, pkg: Metric<f32>, core: Metric<f32>) -> EngineMsg {
    let mut msg = cpu_msg(t_ns, 50.0, false);
    if let EngineMsg::Source(SourceUpdate::Cpu(s)) = &mut msg {
        s.totals.package_power_w = pkg;
        s.totals.core_power_w = core;
        s.totals.freq_fastest_mhz = Metric::valid(5050);
    }
    msg
}

#[test]
fn cpu_secondary_series_fill() {
    let mut rig = Rig::new();
    rig.run(vec![cpu_msg(100 * MS, 50.0, true)])
        .expect("no invariant violation");
    let pkg = rig.idx(&DeviceIdentity::CpuPackage);
    let id = |metric| SeriesId {
        device: pkg,
        metric,
    };
    rig.run(vec![cpu_secondary_msg(
        200 * MS,
        Metric::valid(31.9),
        Metric::valid(18.2),
    )])
    .expect("no invariant violation");
    for (metric, want) in [
        (MetricId::CPU_POWER_PKG, 31.9),
        (MetricId::CPU_POWER_CORE, 18.2),
        (MetricId::CPU_CLOCK_MAX, 5050.0),
    ] {
        let last = rig.last_sample(id(metric));
        assert_eq!(last.value, want, "{metric:?}");
        assert!(last.flags.contains(SampleFlags::VALID), "{metric:?}");
    }

    // Restricted or Unsupported RAPL is a gap, never a measured zero.
    rig.run(vec![cpu_secondary_msg(
        300 * MS,
        Metric::with(Validity::Restricted),
        Metric::with(Validity::Unsupported),
    )])
    .expect("no invariant violation");
    for metric in [MetricId::CPU_POWER_PKG, MetricId::CPU_POWER_CORE] {
        let last = rig.last_sample(id(metric));
        assert!(!last.flags.contains(SampleFlags::VALID), "{metric:?}");
    }
    assert!(
        rig.last_sample(id(MetricId::CPU_CLOCK_MAX))
            .flags
            .contains(SampleFlags::VALID)
    );
}

/// Copies the fixture trees in `parts` over each other into a fresh dir.
fn cpu_fixture(name: &str, parts: &[&str]) -> PathBuf {
    fn copy_tree(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).expect("create fixture dir");
        for entry in fs::read_dir(src).expect("list fixture dir") {
            let entry = entry.expect("fixture dir entry");
            let to = dst.join(entry.file_name());
            if entry.file_type().expect("fixture file type").is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                fs::copy(entry.path(), &to).expect("copy fixture file");
            }
        }
    }
    let fixtures = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tl-collect/tests/fixtures"
    ));
    let dst = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("lab-reducer-{name}"));
    if dst.exists() {
        fs::remove_dir_all(&dst).expect("remove old fixture copy");
    }
    for part in parts {
        copy_tree(&fixtures.join(part), &dst);
    }
    dst
}

/// The published `freq_is_bogomips` after one CPU worker sample of `dir`.
fn published_bogomips(dir: &Path) -> bool {
    let worker = CpuSource::new(SysRoot::at(dir)).expect("fixture topology reads");
    let clock = Clock {
        boot_ns: 100 * MS,
        mono_ns: 100 * MS,
    };
    let (_, update) = worker.sample_at(clock);
    assert!(matches!(update, SourceUpdate::Cpu(_)), "{update:?}");
    let mut rig = Rig::new();
    rig.run(vec![src(update)]).expect("no invariant violation");
    let bogomips = rig.latest().system.cpu.freq_is_bogomips;
    rig.passed();
    bogomips
}

#[test]
fn cpu_freq_source_bogomips_reaches_snapshot() {
    let bogo = cpu_fixture("bogomips", &["cpu-math/step0", "bogomips-only"]);
    assert!(published_bogomips(&bogo));
    let cpufreq = cpu_fixture("cpufreq", &["cpu-math/step0", "cpufreq-mixed"]);
    assert!(!published_bogomips(&cpufreq));
}

#[test]
fn cpu_per_cpu_clocks_reach_snapshot() {
    // policy0 (CPUs 0-1) reads cpuinfo_avg_freq 4200000 kHz, policy2 (CPUs 2-3)
    // scaling_cur_freq 2800000 kHz.
    let dir = cpu_fixture("per-cpu-clocks", &["cpu-math/step0", "cpufreq-mixed"]);
    let worker = CpuSource::new(SysRoot::at(&dir)).expect("fixture topology reads");
    let mut rig = Rig::new();
    // One slow tick per step, one second apart.
    fn sample(worker: CpuSource, rig: &mut Rig, step: u64) -> (CpuSource, Arc<[Metric<u32>]>) {
        let t = (100 + step * 1000) * MS;
        let (worker, update) = worker.sample_at(Clock {
            boot_ns: t,
            mono_ns: t,
        });
        rig.run(vec![src(update)]).expect("no invariant violation");
        (worker, Arc::clone(&rig.latest().system.cpu.per_cpu_mhz))
    }

    let (worker, first) = sample(worker, &mut rig, 0);
    assert_eq!(first.to_vec(), [4200, 4200, 2800, 2800].map(Metric::valid));

    // The same clocks one second later: the same Arc.
    let (worker, second) = sample(worker, &mut rig, 1);
    assert!(Arc::ptr_eq(&first, &second));

    // policy2 changes: a new Arc with CPUs 2 and 3 changed.
    let cur = dir.join("sys/devices/system/cpu/cpufreq/policy2/scaling_cur_freq");
    fs::write(&cur, "3100000\n").expect("write fixture copy");
    let (_, third) = sample(worker, &mut rig, 2);
    assert!(!Arc::ptr_eq(&second, &third));
    assert_eq!(third.to_vec(), [4200, 4200, 3100, 3100].map(Metric::valid));
    rig.passed();
}

#[test]
fn unknown_disk_identity_is_added() {
    let mut rig = Rig::new();
    let id = disk_id("nvme0n1", 0);
    rig.run(vec![disks_msg(
        SEC,
        vec![(id.clone(), disk_totals(10.0, 1e6))],
    )])
    .expect("no invariant violation");
    let idx = rig.idx(&id);
    let snap = rig.latest();
    assert!(
        snap.series
            .get(SeriesId {
                device: idx,
                metric: MetricId::DISK_READ
            })
            .is_some()
    );
    assert_eq!(snap.system.disks.len(), 1);
    assert_eq!(snap.system.disks[0].0, idx);
    assert_eq!(snap.system.disks[0].1.device, idx);
}

const SEC: TimeNs = 1_000_000_000;

fn query(generation: u64) -> EngineMsg {
    cmd(Command::SetTableQuery(TableQuery {
        generation,
        ..TableQuery::default()
    }))
}

#[test]
fn set_table_query_echoes_generation() {
    let mut rig = Rig::new();
    rig.run(vec![query(6)]).expect("no invariant violation");
    let prev = rig.latest();
    assert_eq!(prev.table.query_generation, 6);
    rig.run(vec![query(7)]).expect("no invariant violation");
    let next = rig.latest();
    assert_eq!(next.table.query_generation, 7);
    assert_eq!(next.table.rows, prev.table.rows);
    let mut changed = SmallVec::new();
    assert!(
        next.changed_since(&prev, &mut changed)
            .contains(ComponentMask::TABLE)
    );
}

#[test]
fn table_totals_follow_system_and_sweep() {
    let mut rig = Rig::new();
    let mut sweep = sweep_msg(SEC);
    if let EngineMsg::Source(SourceUpdate::Processes(s)) = &mut sweep {
        s.system_threads = 42;
        s.file_nr = Some(1234);
    }
    rig.run(vec![cpu_msg(SEC, 50.0, true), sweep])
        .expect("no invariant violation");
    let totals = rig.latest().table.totals.clone();
    assert_eq!(totals.cpu_pct.value, 50.0);
    assert_eq!(totals.cpu_pct.validity, Validity::Valid);
    assert_eq!(totals.threads.value, 42);
    assert_eq!(totals.handles.value, 1234);

    // A query change alone recomputes them too.
    rig.run(vec![cpu_msg(2 * SEC, 75.0, false), query(3)])
        .expect("no invariant violation");
    let table = &rig.latest().table;
    assert_eq!(table.totals.cpu_pct.value, 75.0);
    assert_eq!(table.query_generation, 3);
}

/// tasklens-9o0.36: Threads comes from the CPU worker's loadavg count. Without a sweep it still
/// reaches the system snapshot; with a sweep the header shows the same count, not the sweep's sum.
/// `table_totals_follow_system_and_sweep` covers the sweep fallback when the CPU count is not
/// readable.
#[test]
fn threads_from_loadavg_with_and_without_sweep() {
    let with_threads = |t_ns, threads| {
        let mut msg = cpu_msg(t_ns, 50.0, t_ns == SEC);
        if let EngineMsg::Source(SourceUpdate::Cpu(s)) = &mut msg {
            s.totals.threads = threads;
        }
        msg
    };
    let mut rig = Rig::new();
    rig.run(vec![with_threads(SEC, Metric::valid(6667))])
        .expect("no invariant violation");
    assert_eq!(rig.latest().system.cpu.threads, Metric::valid(6667));

    let mut sweep = sweep_msg(2 * SEC);
    if let EngineMsg::Source(SourceUpdate::Processes(s)) = &mut sweep {
        s.system_threads = 42;
    }
    rig.run(vec![with_threads(2 * SEC, Metric::valid(6668)), sweep])
        .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.system.cpu.threads, Metric::valid(6668));
    assert_eq!(snap.table.totals.threads.value, 6668);
    assert_eq!(snap.table.totals.threads.validity, Validity::Valid);

    // An unreadable loadavg stays Missing in the snapshot; the header falls back to the sweep.
    let mut sweep = sweep_msg(3 * SEC);
    if let EngineMsg::Source(SourceUpdate::Processes(s)) = &mut sweep {
        s.system_threads = 42;
    }
    rig.run(vec![
        with_threads(3 * SEC, Metric::with(Validity::Missing)),
        sweep,
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.system.cpu.threads.validity, Validity::Missing);
    assert_eq!(snap.table.totals.threads.value, 42);
}

#[test]
fn refresh_now_is_forwarded() {
    let mut rig = Rig::new();
    rig.run(vec![cmd(Command::RefreshNow(SourceId::Cpu))])
        .expect("no invariant violation");
    let before = rig.commands().last().expect("one command").seq;
    rig.run(vec![cmd(Command::RefreshNow(SourceId::Startup))])
        .expect("no invariant violation");
    let commands = rig.commands();
    let last = commands.last().expect("a command");
    assert_eq!(last.seq, before + 1);
    assert_eq!(last.target, SourceId::Startup);
    assert_eq!(last.command, WorkerCommand::RefreshNow);
}

#[test]
fn forward_queue_keeps_newest_64() {
    let mut rig = Rig::new();
    let progress = Arc::clone(&rig.control_rx.borrow_and_clone().progress);
    let msgs = (0..70)
        .map(|_| cmd(Command::RefreshNow(SourceId::Disk)))
        .collect();
    rig.run(msgs).expect("no invariant violation");
    let control = rig.control_rx.borrow_and_clone();
    let seqs: Vec<u64> = control.commands.iter().map(|c| c.seq).collect();
    assert_eq!(seqs, (7..=70).collect::<Vec<u64>>());
    // R-276: forwarding keeps the session's progress Arc.
    assert!(Arc::ptr_eq(&control.progress, &progress));
}

fn action(id: u64) -> ActionRequest {
    ActionRequest {
        id: ActionId(id),
        kind: ActionKind::Signal(Signal::Term),
        targets: SmallVec::new(),
        privileged: false,
    }
}

#[test]
fn action_without_gate_is_rejected_unsupported() {
    let mut rig = Rig::new();
    rig.run(vec![cmd(Command::Action(action(9)))])
        .expect("no invariant violation");
    let event = rig.events_rx.try_recv().expect("one event");
    assert_eq!(
        event,
        ActionEvent {
            id: ActionId(9),
            target: None,
            state: ActionState::Rejected(RejectReason::Unsupported),
        }
    );
    assert!(rig.events_rx.try_recv().is_err(), "exactly one event");
}

#[test]
fn resume_bumps_reset_and_gaps() {
    let mut rig = Rig::new();
    let disk = disk_id("sda", 0);
    rig.run(vec![
        cpu_msg(SEC, 10.0, true),
        disks_msg(SEC, vec![(disk.clone(), disk_totals(1.0, 1.0))]),
    ])
    .expect("no invariant violation");
    let before = rig.control_rx.borrow_and_clone().reset_generation;
    rig.run(vec![src(SourceUpdate::ResumeDetected {
        id: SourceId::Logind,
        t_ns: 2 * SEC,
    })])
    .expect("no invariant violation");
    assert_eq!(
        rig.control_rx.borrow_and_clone().reset_generation,
        before + 1
    );
    rig.run(vec![
        cpu_msg(3 * SEC, 10.0, false),
        disks_msg(3 * SEC, vec![(disk.clone(), disk_totals(1.0, 1.0))]),
    ])
    .expect("no invariant violation");
    let disk_idx = rig.idx(&disk);
    let snap = rig.latest();
    for view in snap.series.series.iter() {
        let last = view.samples.last().expect("a sample");
        assert!(
            last.flags.contains(SampleFlags::GAP_BEFORE),
            "{:?} has no gap",
            view.id
        );
    }
    assert!(
        snap.series
            .get(SeriesId {
                device: disk_idx,
                metric: MetricId::DISK_ACTIVE
            })
            .is_some()
    );
}

#[test]
fn aggregate_disk_max_and_sum() {
    let mut rig = Rig::new();
    rig.run(vec![disks_msg(
        SEC,
        vec![
            (disk_id("sda", 0), disk_totals(30.0, 1_000_000.0)),
            (disk_id("sdb", 16), disk_totals(70.0, 2_000_000.0)),
        ],
    )])
    .expect("no invariant violation");
    let agg = rig.idx(&DeviceIdentity::Aggregate(AggregateKind::Disks));
    let active = rig.last_sample(SeriesId {
        device: agg,
        metric: MetricId::DISK_ACTIVE,
    });
    let read = rig.last_sample(SeriesId {
        device: agg,
        metric: MetricId::DISK_READ,
    });
    assert_eq!(active.value, 70.0);
    assert_eq!(read.value, 3_000_000.0);
    assert!(read.flags.contains(SampleFlags::VALID));
}

fn net_desc(id: DeviceIdentity, kind: NetKind) -> DeviceDesc {
    DeviceDesc {
        identity: id,
        display_name: Arc::from("if"),
        parent: None,
        persistent_key: Arc::from("net:if"),
        net_kind: Some(kind),
    }
}

#[test]
fn aggregate_net_respects_visibility() {
    let mut rig = Rig::new();
    let (eno1, docker0) = (net_id("eno1", 2), net_id("docker0", 3));
    rig.run(vec![
        src(SourceUpdate::Devices(DeviceList {
            t_ns: SEC,
            kinds: DeviceKinds::NETS,
            entries: vec![
                net_desc(eno1.clone(), NetKind::Wired),
                net_desc(docker0.clone(), NetKind::Virtual),
            ],
        })),
        src(SourceUpdate::Nets(NetSample {
            t_ns: SEC,
            nets: vec![(eno1, net_totals(5000.0)), (docker0, net_totals(700.0))],
        })),
    ])
    .expect("no invariant violation");
    let agg = rig.idx(&DeviceIdentity::Aggregate(AggregateKind::Nets));
    let rx = rig.last_sample(SeriesId {
        device: agg,
        metric: MetricId::NET_RX,
    });
    assert_eq!(rx.value, 5000.0);
}

#[test]
fn battery_details_append_battery_pct() {
    let mut rig = Rig::new();
    let bat0 = DeviceIdentity::Battery {
        native_path: Arc::from("BAT0"),
    };
    rig.run(vec![health_msg(0)])
        .expect("no invariant violation");
    let prev = rig.latest();
    rig.run(vec![src(SourceUpdate::DeviceDetails(
        DeviceDetailsUpdate::Batteries(vec![(bat0.clone(), battery("BAT0", 81.0))]),
    ))])
    .expect("no invariant violation");
    let next = rig.latest();
    let idx = rig.idx(&bat0);
    let pct = rig.last_sample(SeriesId {
        device: idx,
        metric: MetricId::BATTERY_PCT,
    });
    assert_eq!(pct.value, 81.0);
    assert_eq!(next.device_details.batteries.len(), 1);
    let mut changed = SmallVec::new();
    assert!(
        next.changed_since(&prev, &mut changed)
            .contains(ComponentMask::DEVICE_DETAILS)
    );
}

#[test]
fn on_battery_publishes_only_on_change() {
    let mut rig = Rig::new();
    rig.run(vec![health_msg(0)])
        .expect("no invariant violation");
    let on = |on| {
        src(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::OnBattery(
            on,
        )))
    };
    let prev = rig.latest();
    rig.run(vec![on(Some(true))])
        .expect("no invariant violation");
    let next = rig.latest();
    assert_eq!(next.device_details.on_battery, Some(true));
    let mut changed = SmallVec::new();
    assert!(
        next.changed_since(&prev, &mut changed)
            .contains(ComponentMask::DEVICE_DETAILS)
    );
    rig.run(vec![on(Some(true))])
        .expect("no invariant violation");
    let same = rig.latest();
    assert!(
        Arc::ptr_eq(&same.device_details, &next.device_details),
        "an unchanged state keeps the component"
    );
}

#[test]
fn disk_details_merge_by_source() {
    let mut rig = Rig::new();
    let sda = disk_id("sda", 0);
    let from_udisks = DiskDetails {
        model: text("X"),
        ejectable: true,
        ..DiskDetails::default()
    };
    let from_sysfs = DiskDetails {
        model: text("Y"),
        scheduler: text("mq-deadline"),
        kind: Arc::from("SSD"),
        ..DiskDetails::default()
    };
    rig.run(vec![
        src(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
            from: SourceId::Udisks,
            entries: vec![(sda.clone(), from_udisks)],
        })),
        src(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
            from: SourceId::Disk,
            entries: vec![(sda.clone(), from_sysfs)],
        })),
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    let [(idx, disk)] = &*snap.device_details.disks else {
        panic!("one disk record");
    };
    assert_eq!(*idx, rig.idx(&sda));
    assert_eq!(&*disk.model.value, "X");
    assert!(disk.ejectable);
    assert_eq!(&*disk.scheduler.value, "mq-deadline");
    assert_eq!(&*disk.kind, "SSD");

    // A UDisks list without the disk drops the UDisks fields.
    rig.run(vec![src(SourceUpdate::DeviceDetails(
        DeviceDetailsUpdate::Disks {
            from: SourceId::Udisks,
            entries: Vec::new(),
        },
    ))])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(&*snap.device_details.disks[0].1.model.value, "Y");
    assert!(!snap.device_details.disks[0].1.ejectable);
}

#[test]
fn device_commands_forward_to_owner() {
    let mut rig = Rig::new();
    let disk: Arc<str> = Arc::from("sda");
    let cases = [
        (DeviceCommand::FetchPublicIp, SourceId::Inventory),
        (
            DeviceCommand::SmartRead {
                disk: disk.clone(),
                update: false,
            },
            SourceId::Udisks,
        ),
        (DeviceCommand::Eject { seq: 1, disk }, SourceId::Udisks),
        (
            DeviceCommand::BatteryHistory {
                native_path: Arc::from("BAT0"),
            },
            SourceId::UPower,
        ),
    ];
    for (device, target) in cases {
        rig.run(vec![cmd(Command::Device(device.clone()))])
            .expect("no invariant violation");
        let commands = rig.commands();
        let last = commands.last().expect("a command");
        assert_eq!(last.target, target);
        assert_eq!(last.command, WorkerCommand::Device(device));
    }
}

#[test]
fn helper_session_without_tx_is_dropped() {
    let mut rig = Rig::new();
    assert!(rig.reducer().helper_tx.is_none());
    rig.run(vec![
        cmd(Command::StartHelperSession),
        cmd(Command::EndHelperSession),
        cmd(Command::SetupStatus),
    ])
    .expect("no invariant violation");
    assert!(rig.commands().is_empty(), "nothing forwarded to workers");
}

#[test]
fn helper_session_with_tx_is_sent() {
    let mut rig = Rig::new();
    let (tx, mut rx) = mpsc::channel(4);
    rig.reducer().helper_tx = Some(tx);
    rig.run(vec![
        cmd(Command::StartHelperSession),
        cmd(Command::RunSetup {
            step: SetupStep::RaplEnable,
        }),
    ])
    .expect("no invariant violation");
    use tl_engine::reducer::HelperCommand;
    assert_eq!(rx.try_recv().ok(), Some(HelperCommand::Start));
    assert_eq!(
        rx.try_recv().ok(),
        Some(HelperCommand::RunSetup(SetupStep::RaplEnable))
    );
}

fn every_source_update() -> Vec<SourceUpdate> {
    vec![
        SourceUpdate::Health {
            id: SourceId::Connections,
            state: SourceState::Failed,
            last_error: Some(Arc::from("boom")),
            t_ns: 0,
        },
        SourceUpdate::ResumeDetected {
            id: SourceId::Logind,
            t_ns: 0,
        },
        SourceUpdate::Sleep {
            entering: true,
            t_ns: 0,
        },
        SourceUpdate::Cpu(CpuSample {
            t_ns: SEC,
            totals: CpuTotals::default(),
            irq_pct: 0.0,
            per_cpu: Vec::new(),
            freq_mhz: None,
            temp_c: None,
            temp_validity: Validity::default(),
            topology: None,
            validity: Validity::WarmingUp,
        }),
        SourceUpdate::Memory(MemorySample {
            t_ns: SEC,
            totals: MemoryTotals {
                validity: Validity::Valid,
                committed: 5,
                ..MemoryTotals::default()
            },
            zram: Vec::new(),
            validity: Validity::Valid,
        }),
        SourceUpdate::Disks(DiskSample {
            t_ns: SEC,
            disks: vec![(disk_id("sda", 0), disk_totals(1.0, 1.0))],
        }),
        SourceUpdate::Nets(NetSample {
            t_ns: SEC,
            nets: vec![(net_id("eno1", 2), net_totals(1.0))],
        }),
        SourceUpdate::Sensors(SensorSample {
            t_ns: SEC,
            readings: Vec::new(),
        }),
        SourceUpdate::Devices(DeviceList {
            t_ns: SEC,
            kinds: DeviceKinds::DISKS,
            entries: Vec::new(),
        }),
        SourceUpdate::Gpu(GpuSample {
            t_ns: SEC,
            devices: vec![(
                DeviceIdentity::Gpu {
                    uuid: [0; 16],
                    pci_bus_id: Arc::from("0000:01:00.0"),
                },
                GpuTotals {
                    device: DeviceIdx(0),
                    util_pct: Metric::valid(5.0),
                    mem_util_pct: Metric::default(),
                    mem_used: Metric::valid(1024),
                    mem_total: Metric::valid(4096),
                    shared_used: Metric::default(),
                    shared_total: Metric::default(),
                    enc_pct: Metric::default(),
                    dec_pct: Metric::default(),
                    mem_reserved: Metric::default(),
                    enc_sessions: Metric::default(),
                    enc_dec_shared: false,
                    temp_c: Metric::default(),
                    temp_edge_c: Metric::default(),
                    temp_junction_c: Metric::default(),
                    temp_mem_c: Metric::default(),
                    power_w: Metric::default(),
                    power_limit_w: Metric::default(),
                    clock_gfx_mhz: Metric::default(),
                    clock_mem_mhz: Metric::default(),
                    fan_pct: Metric::default(),
                    pcie_rx_bps: Metric::default(),
                    pcie_tx_bps: Metric::default(),
                },
            )],
            processes: Some(Vec::new()),
            drm_processes: None,
        }),
        SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![GpuStaticInfo {
                identity: DeviceIdentity::Gpu {
                    uuid: [0; 16],
                    pci_bus_id: Arc::from("0000:01:00.0"),
                },
                name: Arc::from("Card"),
                mem_total: 0,
                power_limit_mw: None,
                driver: Arc::from(""),
                nvml: Arc::from(""),
                supports_process_util: false,
                persistent_key: Arc::from("gpu:0000:01:00.0"),
            }],
        }),
        SourceUpdate::GpuApiVersions(ApiVersions {
            devices: Vec::new(),
        }),
        SourceUpdate::Processes(ProcessSweep {
            observed_start: SEC,
            observed_end: SEC,
            records: Vec::new(),
            extras: Vec::new(),
            extras_p2: Vec::new(),
            app_stats: Vec::new(),
            unit_stats: Vec::new(),
            system_threads: 0,
            file_nr: None,
            online_cpus: 1,
            low_memory: None,
        }),
        SourceUpdate::Details(tl_engine::update::DetailsUpdate::Pss {
            t_ns: 0,
            entries: Vec::new(),
        }),
        SourceUpdate::Connections(ConnectionsUpdate {
            t_ns: 0,
            records: Vec::new(),
            validity: Validity::default(),
            attribution: Default::default(),
        }),
        SourceUpdate::ConnectionsDestroySupport(true),
        SourceUpdate::Services(ServicesUpdate {
            manager: Manager::System,
            state: ManagerState::default(),
            units: Vec::new(),
            details: Vec::new(),
            backend: ServiceBackend::Systemd,
            validity: Validity::default(),
        }),
        SourceUpdate::Startup(StartupUpdate {
            entries: Vec::new(),
            validity: Validity::default(),
        }),
        SourceUpdate::Inventory(Box::default()),
        SourceUpdate::Windows(Box::default()),
        SourceUpdate::History(Arc::default()),
        SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Cpu(Arc::default())),
        SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Memory(Arc::default())),
        SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Nets {
            from: SourceId::NetworkManager,
            entries: Vec::new(),
        }),
        SourceUpdate::Helper(HelperUpdate {
            state: HelperState::default(),
            start: None,
        }),
        SourceUpdate::Setup(Arc::default()),
        SourceUpdate::ColorScheme(1),
    ]
}

fn every_command() -> Vec<Command> {
    let key = ProcessKey {
        start_ticks: 100,
        boot_id: BOOT,
        pid: 7,
    };
    vec![
        Command::SetTableQuery(TableQuery::default()),
        Command::SetSysTableQuery(SysTableQuery {
            table: SysTable::Startup,
            generation: 1,
            sort: SysSortSpec {
                column: SysColumnId::all(SysTable::Startup)[0],
                ascending: true,
            },
            filter: Arc::from(""),
            expanded: Arc::from([]),
            services: None,
        }),
        Command::Action(action(1)),
        Command::RefreshNow(SourceId::Cpu),
        Command::ResetBaselines,
        Command::ReprobeSource(SourceId::Sensors),
        Command::KWin(KWinCommand::Pick { seq: 1 }),
        Command::DetailRefresh {
            target: DetailTarget::Process(key),
            tab: DetailTab::General,
        },
        Command::ComputeHashes(key),
        Command::VerifyPackage(key),
        Command::StringScan {
            scan: ScanId(1),
            key,
            opts: StringScanOpts::default(),
        },
        Command::CancelStringScan(ScanId(1)),
        Command::HandleSearch {
            search: SearchId(1),
            pattern: Arc::from("libc"),
            include_maps: false,
            privileged: false,
        },
        Command::CancelHandleSearch(SearchId(1)),
        Command::HistoryQuery {
            id: 1,
            range: HistoryRange::Live,
            what: HistoryWhat::TopAt {
                t: 0,
                metric: HistoryMetric::Cpu,
                n: 3,
            },
        },
        Command::HistoryDelete {
            keys: Arc::from([key]),
        },
        Command::SetupStatus,
        Command::RunSetup {
            step: SetupStep::RaplEnable,
        },
        Command::Device(DeviceCommand::FetchPublicIp),
        Command::StartHelperSession,
        Command::EndHelperSession,
    ]
}

#[test]
fn every_variant_applies_without_panic() {
    let mut rig = Rig::new();
    let mut msgs: Vec<EngineMsg> = every_source_update().into_iter().map(src).collect();
    msgs.extend(every_command().into_iter().map(cmd));
    msgs.push(EngineMsg::ActionFinished { id: ActionId(1) });
    msgs.push(EngineMsg::Tick);
    rig.run(msgs).expect("no invariant violation");
    let snap = rig.latest();
    // The Connections update after the Failed health carries its own validity (E09-027).
    assert_eq!(snap.connections.header.validity, Validity::WarmingUp);
    assert_eq!(snap.system.color_scheme, 1);
    assert_eq!(snap.inventory.memory.peak_committed_bytes.value, 5);
    // 11 forwarded variants, plus `RefreshNow` to Disk, Net and Sensors from the resume. The
    // two history commands go to the history sink instead, which this rig does not install.
    assert_eq!(
        rig.commands().len(),
        14,
        "one queued command per forwarded variant"
    );
}

// ---- Sequence, versions and Tick effects (E09, ADR-005, ADR-008, ADR-013) ----

impl Rig {
    /// Runs the lab to quiescence and checks its oracles.
    fn passed(mut self) {
        let report = self.lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }
}

/// xorshift64, so the message mix is the same on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

fn proc_key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid) * 10,
        boot_id: BOOT,
        pid,
    }
}

fn proc_record(pid: u32) -> ProcessRecord {
    ProcessRecord {
        key: proc_key(pid),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: "x".into(),
            name: "x".into(),
            exe: None,
            cmdline: None,
            user: "user".into(),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: "x user".into(),
        }),
        group: GroupKey(0),
        state: ProcState::Sleeping,
        flags: ProcFlags::empty(),
        nice: 0,
        threads: 1,
        cpu_pct: 0.0,
        rss_anon_bytes: 0,
        rss_bytes: 0,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.0,
        io_write_bps: 0.0,
        net_rx_bps: 0.0,
        net_tx_bps: 0.0,
        gpu_util_pct: 0.0,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.0,
        validity: FieldValidity::all(Validity::Valid),
    }
}

fn proc_extras() -> ProcessExtras {
    ProcessExtras {
        cpu_kernel_pct: 0.0,
        cpu_user_pct: 0.0,
        cpu_relative_pct: 0.0,
        utime_ticks: 0,
        stime_ticks: 0,
        start_boot_ns: 0,
        rss_peak_bytes: 0,
        rss_shared_bytes: 0,
        commit_bytes: 0,
        vsize_bytes: 0,
        vsize_peak_bytes: 0,
        page_faults: 0,
        page_faults_delta: 0,
        major_faults: 0,
        policy: SchedPolicy::Other,
        rt_prio: 0,
        affinity: CpuSet::empty(),
        handles: None,
        exe_name: "x".into(),
        exe_path: "/usr/bin/x".into(),
        exe_deleted: false,
        unit: None,
        state_letter: b'S',
        frozen: false,
        disk_read_bps: 0.0,
        disk_write_raw_bps: 0.0,
        flag_since_ns: 0,
    }
}

/// A sweep at `t_ns` that sees the processes `pids`, with index aligned extras.
fn sweep_of(t_ns: TimeNs, pids: &[u32]) -> EngineMsg {
    let mut sweep = sweep_msg(t_ns);
    if let EngineMsg::Source(SourceUpdate::Processes(s)) = &mut sweep {
        s.records = pids.iter().map(|&pid| proc_record(pid)).collect();
        s.extras = pids.iter().map(|_| proc_extras()).collect();
    }
    sweep
}

fn ticks(n: u64) -> Vec<EngineMsg> {
    (0..n).map(|_| EngineMsg::Tick).collect()
}

#[test]
fn sequence_strictly_increases_over_1000_mixed_messages() {
    let mut rig = Rig::new();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut prev = rig.latest().sequence;
    let mut sent = 0u64;
    let mut publishes = 0u64;
    while sent < 1000 {
        // Batches of 1 to 4 messages, each batch one publish.
        let size = (1 + rng.below(4)).min(1000 - sent);
        let mut batch = Vec::new();
        for _ in 0..size {
            sent += 1;
            let t = sent * 100 * MS;
            batch.push(match rng.below(4) {
                0 => cpu_msg(t, rng.below(100) as f32, sent == 1),
                1 => sweep_of(t, &[1, 2 + (rng.below(3) as u32)]),
                2 => query(sent),
                _ => EngineMsg::Tick,
            });
        }
        rig.run(batch).expect("no invariant violation");
        let next = rig.latest().sequence;
        assert_eq!(next, prev + 1, "publish after {sent} messages");
        assert_eq!(rig.reducer().sequence, next);
        prev = next;
        publishes += 1;
    }
    assert!(publishes >= 250, "{publishes} publishes");
    rig.passed();
}

#[test]
fn component_versions_bump_only_when_changed() {
    let mut rig = Rig::new();
    rig.run(vec![cpu_msg(SEC, 50.0, true), sweep_of(SEC, &[1])])
        .expect("no invariant violation");
    let prev = rig.latest();
    assert!(prev.series.header.version > 0);

    rig.run(vec![sweep_of(2 * SEC, &[1, 2])])
        .expect("no invariant violation");
    let next = rig.latest();
    assert!(next.processes.header.version > prev.processes.header.version);
    assert!(next.table.header.version > prev.table.header.version);
    assert_eq!(next.series.header.version, prev.series.header.version);
    assert!(Arc::ptr_eq(&next.series, &prev.series));
    assert_eq!(next.processes.records.len(), 2);
    rig.passed();
}

/// The reducer reads CLOCK_BOOTTIME on `Tick`, so the test places the last CPU sample `n`
/// seconds in the past: the same as `n` one-second ticks after the source stopped. Returns
/// the newest `CPU_TOTAL` flags and the CPU health state after `n` ticks.
fn after_cpu_stops(n: u64) -> (SampleFlags, SourceState) {
    let mut rig = Rig::new();
    let last = now_boot_ns() - n * SEC;
    rig.run(vec![
        health_msg(0),
        cpu_msg(last - 2 * SEC, 50.0, true),
        cpu_msg(last - SEC, 50.0, false),
        cpu_msg(last, 50.0, false),
    ])
    .expect("no invariant violation");
    assert!(
        !rig.last_sample(CPU_TOTAL)
            .flags
            .contains(SampleFlags::STALE)
    );
    assert_eq!(
        rig.latest().health.get(SourceId::Cpu).state,
        SourceState::Running
    );

    rig.run(ticks(n)).expect("no invariant violation");
    let flags = rig.last_sample(CPU_TOTAL).flags;
    let state = rig.latest().health.get(SourceId::Cpu).state;
    rig.passed();
    (flags, state)
}

#[test]
fn tick_marks_stale_series_and_health() {
    // Stale after max(3 × 1 s, 1.5 s) without data (ADR-008).
    let (flags, state) = after_cpu_stops(1);
    assert!(!flags.contains(SampleFlags::STALE));
    assert_eq!(state, SourceState::Running);

    let (flags, state) = after_cpu_stops(4);
    assert!(flags.contains(SampleFlags::STALE));
    assert_eq!(state, SourceState::Stale);
}

/// Fresh data clears both marks again, and the health change is published.
#[test]
fn fresh_data_clears_stale() {
    let mut rig = Rig::new();
    let last = now_boot_ns() - 4 * SEC;
    rig.run(vec![
        health_msg(0),
        cpu_msg(last - SEC, 50.0, true),
        cpu_msg(last, 50.0, false),
        EngineMsg::Tick,
    ])
    .expect("no invariant violation");
    assert_eq!(
        rig.latest().health.get(SourceId::Cpu).state,
        SourceState::Stale
    );
    rig.run(vec![cpu_msg(now_boot_ns(), 50.0, false), EngineMsg::Tick])
        .expect("no invariant violation");
    assert!(
        !rig.last_sample(CPU_TOTAL)
            .flags
            .contains(SampleFlags::STALE)
    );
    assert_eq!(
        rig.latest().health.get(SourceId::Cpu).state,
        SourceState::Running
    );
    rig.passed();
}

/// Process 2 exits in a sweep `n` seconds before now, as if `n` one-second ticks followed
/// it (see `after_cpu_stops`). Returns whether its record is in the snapshot after them.
fn exited_present_after(n: u64) -> bool {
    assert_eq!(EngineSettings::default().interval_ms, 1000);
    let mut rig = Rig::new();
    let exit = now_boot_ns() - n * SEC;
    rig.run(vec![sweep_of(exit - SEC, &[1, 2]), sweep_of(exit, &[1])])
        .expect("no invariant violation");
    let gone = proc_key(2);
    let snap = rig.latest();
    let record = snap
        .processes
        .records
        .iter()
        .find(|r| r.key == gone)
        .expect("the exited record is published at once");
    assert!(record.flags.contains(ProcFlags::EXITED));

    rig.run(ticks(n)).expect("no invariant violation");
    let snap = rig.latest();
    let present = snap.processes.records.iter().any(|r| r.key == gone);
    assert!(snap.processes.records.iter().any(|r| r.key == proc_key(1)));
    rig.passed();
    present
}

#[test]
fn tick_drops_exited_after_linger() {
    // Linger is clamp(2 × 1000 ms, 1 s, 3 s) = 2 s (R-041).
    assert!(exited_present_after(1), "present after 1 tick");
    assert!(!exited_present_after(3), "gone after 3 ticks");
}

#[test]
fn set_table_query_older_generation_still_applied() {
    let mut rig = Rig::new();
    rig.run(vec![query(7)]).expect("no invariant violation");
    assert_eq!(rig.latest().table.query_generation, 7);
    // The UI drops tables older than its newest query; the engine applies them in order.
    rig.run(vec![query(5)]).expect("no invariant violation");
    assert_eq!(rig.latest().table.query_generation, 5);
    rig.passed();
}

// ---- GPU process rows and PSS (E09-006) ----

/// `sweep_of` with PSS and USS `WarmingUp`, as a sweep that did not read `smaps_rollup`.
fn sweep_without_pss(t_ns: TimeNs, pids: &[u32]) -> EngineMsg {
    let mut sweep = sweep_of(t_ns, pids);
    if let EngineMsg::Source(SourceUpdate::Processes(s)) = &mut sweep {
        for r in &mut s.records {
            r.validity.set(ProcessField::Pss, Validity::WarmingUp);
            r.validity.set(ProcessField::Uss, Validity::WarmingUp);
        }
    }
    sweep
}

fn gpu_info(uuid: [u8; 16], supports_process_util: bool) -> EngineMsg {
    src(SourceUpdate::GpuInfo(GpuInfoUpdate {
        devices: vec![GpuStaticInfo {
            identity: DeviceIdentity::Gpu {
                uuid,
                pci_bus_id: Arc::from("0000:01:00.0"),
            },
            name: Arc::from("Card"),
            mem_total: 0,
            power_limit_mw: None,
            driver: Arc::from(""),
            nvml: Arc::from(""),
            supports_process_util,
            persistent_key: Arc::from("gpu:0000:01:00.0"),
        }],
    }))
}

fn pss_msg(keys: &[ProcessKey], pss_bytes: u64) -> EngineMsg {
    src(SourceUpdate::Details(DetailsUpdate::Pss {
        t_ns: now_boot_ns(),
        entries: keys
            .iter()
            .map(|&key| PssEntry {
                key,
                pss_bytes,
                uss_bytes: pss_bytes / 2,
                swap_bytes: 0,
                validity: Validity::Valid,
            })
            .collect(),
    }))
}

fn record_of(snap: &Snapshot, key: ProcessKey) -> ProcessRecord {
    snap.processes
        .records
        .iter()
        .find(|r| r.key == key)
        .expect("the record is published")
        .clone()
}

#[test]
fn gpu_rows_reach_records() {
    let mut rig = Rig::new();
    let uuid = [7; 16];
    let gpu = src(SourceUpdate::Gpu(GpuSample {
        t_ns: now_boot_ns(),
        devices: Vec::new(),
        processes: Some(vec![GpuProcessSample {
            uuid,
            pid: 2,
            sm_pct: Reading::Ok(40),
            mem_pct: Reading::Ok(0),
            enc_pct: Reading::Ok(55),
            dec_pct: Reading::Unsupported,
            mem_used: Reading::Ok(4096),
        }]),
        drm_processes: None,
    }));
    rig.run(vec![
        gpu_info(uuid, true),
        sweep_without_pss(now_boot_ns(), &[1, 2]),
        gpu,
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    let busy = record_of(&snap, proc_key(2));
    assert_eq!(busy.validity.get(ProcessField::GpuUtil), Validity::Valid);
    assert!(
        (busy.gpu_util_pct - 55.0).abs() < f32::EPSILON,
        "max of sm, enc, dec"
    );
    assert_eq!(busy.gpu_mem_bytes, 4096);
    let idle = record_of(&snap, proc_key(1));
    assert_eq!(idle.validity.get(ProcessField::GpuUtil), Validity::Valid);
    assert!(idle.gpu_util_pct.abs() < f32::EPSILON);
    rig.passed();
}

#[test]
fn pss_coverage_published() {
    let mut rig = Rig::new();
    let key = proc_key(2);
    rig.run(vec![sweep_without_pss(now_boot_ns(), &[1, 2])])
        .expect("no invariant violation");
    assert!(rig.latest().processes.coverage.pss_uss.is_empty());
    rig.run(vec![pss_msg(&[key], 8192)])
        .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(&*snap.processes.coverage.pss_uss, &[key]);
    let record = record_of(&snap, key);
    assert_eq!(record.validity.get(ProcessField::Pss), Validity::Valid);
    assert_eq!(record.pss_bytes, 8192);
    assert_eq!(
        record_of(&snap, proc_key(1))
            .validity
            .get(ProcessField::Pss),
        Validity::WarmingUp
    );
    rig.passed();
}

// ---- Power estimate (E09-030) ----

/// `sweep_of` with `utime_ticks` set per process.
fn sweep_ticks(t_ns: TimeNs, rows: &[(u32, u64)]) -> EngineMsg {
    let pids: Vec<u32> = rows.iter().map(|&(pid, _)| pid).collect();
    let mut sweep = sweep_of(t_ns, &pids);
    if let EngineMsg::Source(SourceUpdate::Processes(s)) = &mut sweep {
        for (e, &(_, ticks)) in s.extras.iter_mut().zip(rows) {
            e.utime_ticks = ticks;
        }
    }
    sweep
}

/// A CPU sample with RAPL package power `watts`.
fn cpu_power_msg(t_ns: TimeNs, watts: f32) -> EngineMsg {
    let mut msg = cpu_msg(t_ns, 50.0, false);
    if let EngineMsg::Source(SourceUpdate::Cpu(s)) = &mut msg {
        s.totals.package_power_w = Metric::valid(watts);
    }
    msg
}

/// A GPU sample with measured power `watts` and SM use per pid.
fn gpu_power_msg(uuid: [u8; 16], watts: f32, sm: &[(u32, u32)]) -> EngineMsg {
    src(SourceUpdate::Gpu(GpuSample {
        t_ns: now_boot_ns(),
        devices: vec![(
            DeviceIdentity::Gpu {
                uuid,
                pci_bus_id: Arc::from("0000:01:00.0"),
            },
            GpuTotals {
                device: DeviceIdx(0),
                util_pct: Metric::valid(80.0),
                mem_util_pct: Metric::default(),
                mem_used: Metric::valid(1024),
                mem_total: Metric::valid(4096),
                shared_used: Metric::default(),
                shared_total: Metric::default(),
                enc_pct: Metric::default(),
                dec_pct: Metric::default(),
                mem_reserved: Metric::default(),
                enc_sessions: Metric::default(),
                enc_dec_shared: false,
                temp_c: Metric::default(),
                temp_edge_c: Metric::default(),
                temp_junction_c: Metric::default(),
                temp_mem_c: Metric::default(),
                power_w: Metric::valid(watts),
                power_limit_w: Metric::default(),
                clock_gfx_mhz: Metric::default(),
                clock_mem_mhz: Metric::default(),
                fan_pct: Metric::default(),
                pcie_rx_bps: Metric::default(),
                pcie_tx_bps: Metric::default(),
            },
        )],
        processes: Some(
            sm.iter()
                .map(|&(pid, pct)| GpuProcessSample {
                    uuid,
                    pid,
                    sm_pct: Reading::Ok(pct),
                    mem_pct: Reading::Ok(0),
                    enc_pct: Reading::Ok(0),
                    dec_pct: Reading::Unsupported,
                    mem_used: Reading::Ok(0),
                })
                .collect(),
        ),
        drm_processes: None,
    }))
}

fn power_of(snap: &Snapshot, pid: u32) -> Option<f32> {
    let i = snap
        .processes
        .records
        .iter()
        .position(|r| r.key == proc_key(pid))
        .expect("the record is published");
    snap.processes.extras_p2[i].power_w
}

#[test]
fn power_estimate_survives_republish() {
    let mut rig = Rig::new();
    let uuid = [9; 16];
    let t = now_boot_ns();
    rig.run(vec![
        gpu_info(uuid, true),
        cpu_power_msg(t, 20.0),
        sweep_ticks(t, &[(1, 100), (2, 100)]),
        gpu_power_msg(uuid, 100.0, &[(1, 60), (2, 20)]),
    ])
    .expect("no invariant violation");
    let first = rig.latest();
    assert_eq!(power_of(&first, 1), None, "first sight has no estimate");

    rig.run(vec![sweep_ticks(t + SEC, &[(1, 130), (2, 110)])])
        .expect("no invariant violation");
    let snap = rig.latest();
    // CPU 20 W shared 30:10 plus GPU 100 W shared 60:20.
    assert_eq!(power_of(&snap, 1), Some(90.0));
    assert_eq!(power_of(&snap, 2), Some(30.0));

    rig.run(vec![pss_msg(&[proc_key(2)], 8192)])
        .expect("no invariant violation");
    let after_pss = rig.latest();
    assert!(!Arc::ptr_eq(&after_pss.processes, &snap.processes));
    assert_eq!(power_of(&after_pss, 1), Some(90.0));
    assert_eq!(power_of(&after_pss, 2), Some(30.0));

    rig.run(vec![gpu_power_msg(uuid, 50.0, &[(1, 10)])])
        .expect("no invariant violation");
    let after_gpu = rig.latest();
    assert!(!Arc::ptr_eq(&after_gpu.processes, &after_pss.processes));
    assert_eq!(
        power_of(&after_gpu, 1),
        Some(90.0),
        "kept until the next sweep"
    );
    assert_eq!(power_of(&after_gpu, 2), Some(30.0));
    rig.passed();
}

fn pss_demand(keys: &[ProcessKey], shown: bool) -> Demand {
    let mut d = Demand {
        detail_keys: Arc::from(keys),
        ..Demand::default()
    };
    if shown {
        d.process_columns.insert(ColumnId::Pss);
    }
    d
}

/// Sweeps processes 1 to 3, demands PSS for 2 and 3 and applies their PSS. Returns the rig
/// and the demand sender.
fn rig_with_pss() -> (Rig, watch::Sender<Demand>) {
    let mut rig = Rig::new();
    let (demand_tx, demand_rx) = watch::channel(pss_demand(&[proc_key(2), proc_key(3)], true));
    rig.reducer().demand_rx = Some(demand_rx);
    rig.run(vec![
        sweep_without_pss(now_boot_ns(), &[1, 2, 3]),
        pss_msg(&[proc_key(2), proc_key(3)], 8192),
    ])
    .expect("no invariant violation");
    assert_eq!(
        &*rig.latest().processes.coverage.pss_uss,
        &[proc_key(2), proc_key(3)]
    );
    (rig, demand_tx)
}

#[test]
fn pss_cleared_outside_detail_keys() {
    let (mut rig, demand_tx) = rig_with_pss();
    demand_tx
        .send(pss_demand(&[proc_key(3)], true))
        .expect("the reducer holds the receiver");
    rig.run(vec![EngineMsg::Tick])
        .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(&*snap.processes.coverage.pss_uss, &[proc_key(3)]);
    let left = record_of(&snap, proc_key(2));
    assert_eq!(left.validity.get(ProcessField::Pss), Validity::WarmingUp);
    assert_eq!(left.pss_bytes, 0);
    let kept = record_of(&snap, proc_key(3));
    assert_eq!(kept.validity.get(ProcessField::Pss), Validity::Valid);
    assert_eq!(kept.pss_bytes, 8192);
    rig.passed();
}

#[test]
fn pss_cleared_when_column_hidden() {
    let (mut rig, demand_tx) = rig_with_pss();
    demand_tx
        .send(pss_demand(&[proc_key(2), proc_key(3)], false))
        .expect("the reducer holds the receiver");
    rig.run(vec![EngineMsg::Tick])
        .expect("no invariant violation");
    let snap = rig.latest();
    assert!(snap.processes.coverage.pss_uss.is_empty());
    for pid in [2, 3] {
        let record = record_of(&snap, proc_key(pid));
        assert_eq!(record.validity.get(ProcessField::Pss), Validity::WarmingUp);
    }
    rig.passed();
}

fn cpu_health(state: SourceState, t_ns: TimeNs) -> EngineMsg {
    src(SourceUpdate::Health {
        id: SourceId::Cpu,
        state,
        last_error: None,
        t_ns,
    })
}

fn memory_msg(t_ns: TimeNs) -> EngineMsg {
    src(SourceUpdate::Memory(MemorySample {
        t_ns,
        totals: MemoryTotals {
            validity: Validity::Valid,
            used: 1 << 30,
            ..MemoryTotals::default()
        },
        zram: Vec::new(),
        validity: Validity::Valid,
    }))
}

fn has_gap(s: Sample) -> bool {
    s.flags.contains(SampleFlags::GAP_BEFORE)
}

/// R-289: a Cpu worker restart gaps the CPU series and leaves the memory series alone.
#[test]
fn restart_marks_gap_for_source_only() {
    let mut rig = Rig::new();
    rig.run(vec![
        cpu_health(SourceState::Running, 0),
        cpu_msg(SEC, 10.0, true),
        memory_msg(SEC),
        cpu_health(SourceState::Failed, SEC + 100 * MS),
        cpu_health(SourceState::Running, SEC + 600 * MS),
        cpu_msg(SEC + 700 * MS, 20.0, false),
        memory_msg(2 * SEC),
    ])
    .expect("no invariant violation");
    let mem = SeriesId {
        device: rig.idx(&DeviceIdentity::Memory),
        metric: MetricId::MEM_USED,
    };
    let snap = rig.latest();
    let cpu = &snap
        .series
        .get(CPU_TOTAL)
        .expect("CPU_TOTAL exists")
        .samples;
    assert_eq!(cpu.len(), 2);
    assert!(!has_gap(cpu[0]));
    assert!(has_gap(cpu[1]));
    let mem = &snap.series.get(mem).expect("MEM_USED exists").samples;
    assert_eq!(mem.len(), 2);
    assert!(mem.iter().all(|&s| !has_gap(s)));
}

/// tasklens-hjn.35: MemAvailable of 6 GiB gives a MEM_AVAILABLE sample of 6 GiB.
#[test]
fn memory_available_series() {
    // Parse a real meminfo buffer so the 6 GiB comes from `MemAvailable`.
    let meminfo = "MemTotal:        8388608 kB\nMemFree:          524288 kB\n\
        MemAvailable:    6291456 kB\nBuffers:               0 kB\nCached:           524288 kB\n\
        SwapCached:            0 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n\
        Dirty:                 0 kB\nWriteback:             0 kB\nShmem:                 0 kB\n\
        SReclaimable:          0 kB\nSUnreclaim:            0 kB\nCommitLimit:     4194304 kB\n\
        Committed_AS:    2097152 kB\n";
    let mut info = tl_collect::parse::meminfo::MemInfo::default();
    tl_collect::parse::meminfo::parse_meminfo(meminfo.as_bytes(), &mut info)
        .expect("meminfo parses");
    assert_eq!(info.mem_available, 6 << 30);
    let mut rig = Rig::new();
    rig.run(vec![src(SourceUpdate::Memory(MemorySample {
        t_ns: SEC,
        totals: MemoryTotals {
            validity: Validity::Valid,
            total: 8 << 30,
            available: info.mem_available,
            used: 2 << 30,
            ..MemoryTotals::default()
        },
        zram: Vec::new(),
        validity: Validity::Valid,
    }))])
    .expect("no invariant violation");
    let id = SeriesId {
        device: rig.idx(&DeviceIdentity::Memory),
        metric: MetricId::MEM_AVAILABLE,
    };
    let snap = rig.latest();
    let s = &snap.series.get(id).expect("MEM_AVAILABLE exists").samples;
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].value, (6u64 << 30) as f32);
}

/// R-289: the first Running of a session adds no gap, even after Off.
#[test]
fn first_running_no_extra_gap() {
    let mut rig = Rig::new();
    rig.run(vec![
        cpu_health(SourceState::Off, 0),
        cpu_health(SourceState::Running, 100 * MS),
        cpu_msg(SEC, 10.0, true),
        cpu_health(SourceState::Running, SEC + 50 * MS),
        cpu_msg(SEC + 100 * MS, 20.0, false),
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    let cpu = &snap
        .series
        .get(CPU_TOTAL)
        .expect("CPU_TOTAL exists")
        .samples;
    assert_eq!(cpu.len(), 2);
    assert!(cpu.iter().all(|&s| !has_gap(s)));
}

// ---- Details (E09-026) ----

fn tab_msg(target: DetailTarget, tab: DetailTab, payload: &Arc<TabPayload>) -> EngineMsg {
    src(SourceUpdate::Details(DetailsUpdate::Tab {
        target,
        tab,
        payload: Arc::clone(payload),
        validity: Validity::Valid,
    }))
}

fn env_payload() -> Arc<TabPayload> {
    Arc::new(TabPayload::Environ(Arc::new(EnvDetail::default())))
}

fn threads_payload() -> Arc<TabPayload> {
    Arc::new(TabPayload::Threads(Arc::from([])))
}

fn details_demand(targets: &[DetailTarget], affinity: &[ProcessKey]) -> Demand {
    Demand {
        details: targets
            .iter()
            .map(|&target| DetailDemand {
                target,
                tab: DetailTab::General,
            })
            .collect(),
        affinity_targets: Arc::from(affinity),
        ..Demand::default()
    }
}

fn affinity(n: u16) -> AffinityInfo {
    AffinityInfo {
        mask: cpus(n),
        cpuset_limit: cpus(8),
    }
}

fn handle_state(progress_pct: u8) -> Arc<HandleSearchState> {
    Arc::new(HandleSearchState {
        search: SearchId(1),
        phase: HandlePhase::Retrieving,
        progress_pct,
        total_handles: 18_234,
        chunks: Arc::from([]),
        searched: 3,
        total: 10,
        skipped: 0,
    })
}

/// The payload `Arc` of one tab, which must be set.
fn tab_arc(snap: &Snapshot, target: DetailTarget, tab: DetailTab) -> Arc<TabPayload> {
    let slot = snap.details.targets[&target].tab(tab);
    Arc::clone(slot.payload.as_ref().expect("the tab has a payload"))
}

#[test]
fn details_tab_arc_reuse_and_version() {
    let mut rig = Rig::new();
    let a = DetailTarget::Process(proc_key(1));
    // Environment stands in for General: the reducer does not look inside a payload, and
    // GeneralDetail has no Default.
    let p1 = env_payload();
    rig.run(vec![tab_msg(a, DetailTab::Environment, &p1)])
        .expect("no invariant violation");
    let first = rig.latest();
    rig.run(vec![tab_msg(a, DetailTab::Threads, &threads_payload())])
        .expect("no invariant violation");
    let second = rig.latest();

    assert!(Arc::ptr_eq(
        &tab_arc(&first, a, DetailTab::Environment),
        &tab_arc(&second, a, DetailTab::Environment)
    ));
    assert!(Arc::ptr_eq(
        &p1,
        &tab_arc(&second, a, DetailTab::Environment)
    ));
    let detail = &second.details.targets[&a];
    assert_eq!(detail.tab(DetailTab::Threads).version, 1);
    assert_eq!(detail.tab(DetailTab::Threads).validity, Validity::Valid);
    assert_eq!(detail.tab(DetailTab::Environment).version, 1);
    assert!(second.details.header.version > first.details.header.version);
    rig.passed();
}

#[test]
fn details_retain_on_tick() {
    let mut rig = Rig::new();
    let a = DetailTarget::Process(proc_key(1));
    let b = DetailTarget::App(GroupKey(7));
    let (demand_tx, demand_rx) = watch::channel(details_demand(&[a, b], &[]));
    rig.reducer().demand_rx = Some(demand_rx);
    rig.run(vec![
        tab_msg(a, DetailTab::Threads, &threads_payload()),
        tab_msg(b, DetailTab::Threads, &threads_payload()),
    ])
    .expect("no invariant violation");
    assert_eq!(rig.latest().details.targets.len(), 2);

    demand_tx
        .send(details_demand(&[b], &[]))
        .expect("the reducer holds the receiver");
    rig.run(vec![EngineMsg::Tick])
        .expect("no invariant violation");
    let snap = rig.latest();
    assert!(!snap.details.targets.contains_key(&a));
    assert!(snap.details.targets.contains_key(&b));
    rig.passed();
}

#[test]
fn details_affinity_retain() {
    let mut rig = Rig::new();
    let (a, b) = (proc_key(1), proc_key(2));
    let (demand_tx, demand_rx) = watch::channel(details_demand(&[], &[a, b]));
    rig.reducer().demand_rx = Some(demand_rx);
    rig.run(vec![src(SourceUpdate::Details(DetailsUpdate::Affinity {
        t_ns: now_boot_ns(),
        entries: vec![(a, affinity(2)), (b, affinity(4))],
    }))])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.details.affinity.get(&a), Some(&affinity(2)));
    assert_eq!(snap.details.affinity.get(&b), Some(&affinity(4)));

    demand_tx
        .send(details_demand(&[], &[a]))
        .expect("the reducer holds the receiver");
    rig.run(vec![EngineMsg::Tick])
        .expect("no invariant violation");
    let snap = rig.latest();
    assert!(snap.details.affinity.contains_key(&a));
    assert!(!snap.details.affinity.contains_key(&b));
    rig.passed();
}

#[test]
fn details_handle_search_state() {
    let mut rig = Rig::new();
    rig.run(vec![src(SourceUpdate::Details(
        DetailsUpdate::HandleSearch(handle_state(42)),
    ))])
    .expect("no invariant violation");
    let snap = rig.latest();
    let state = snap
        .details
        .handle_search
        .as_ref()
        .expect("the search state is published");
    assert_eq!(state.progress_pct, 42);

    rig.reducer().details.clear_handle_search();
    rig.run(vec![EngineMsg::Tick])
        .expect("no invariant violation");
    assert!(rig.latest().details.handle_search.is_none());
    rig.passed();
}

// ---- P2 system components and sys table queries (E09-027) ----

use std::net::{Ipv4Addr, SocketAddr};

use tl_model::actions::{Family, FourTuple, Proto};
use tl_model::gpu_proto::GpuApiVersions;
use tl_model::snapshot::{
    ConnRecord, ConnState, GraphicsAdapter, InventorySnapshot, RecoverySettings, ServiceStatus,
    StartupType, UnitDetails, UnitRecord,
};
use tl_model::table::{UnitRef, UnitType};

fn conn(cookie: u64, proto: Proto, state: ConnState) -> ConnRecord {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    ConnRecord {
        cookie,
        proto,
        family: Family::Inet,
        four_tuple: FourTuple {
            local: addr,
            remote: addr,
        },
        local: Arc::from("127.0.0.1:0"),
        foreign: Arc::from("*:*"),
        state,
        owners: SmallVec::new(),
        owner_validity: Validity::Valid,
        uid: 0,
        inode: cookie,
        ifindex: 0,
        interface: None,
        recv_q: 0,
        send_q: 0,
        unit: None,
        tcp: None,
        remote_host: None,
    }
}

fn connections_msg(records: Vec<ConnRecord>) -> EngineMsg {
    src(SourceUpdate::Connections(ConnectionsUpdate {
        t_ns: SEC,
        records,
        validity: Validity::Valid,
        attribution: Default::default(),
    }))
}

fn unit_ref(manager: Manager, name: &str) -> UnitRef {
    UnitRef {
        manager,
        name: Arc::from(name),
    }
}

fn missing<T: Default>() -> Total<T> {
    Total {
        value: T::default(),
        validity: Validity::Missing,
    }
}

fn unit_rec(unit: UnitRef) -> UnitRecord {
    UnitRecord {
        unit,
        kind: UnitType::Service,
        description: Arc::from(""),
        status: ServiceStatus::Running,
        sub_state: Arc::from("running"),
        load_state: Arc::from("loaded"),
        startup: StartupType::Enabled,
        main_pid: 0,
        user: Arc::from("root"),
        path: Arc::from(""),
        unit_file: None,
        active_since_us: None,
        triggered_by: Arc::from([]),
        control_group: None,
        can_reload: false,
        has_cgroup_procs: false,
        cgroup_pids: Arc::from([]),
        cpu_pct: missing(),
        memory_bytes: missing(),
        disk_bps: missing(),
        tasks: missing(),
    }
}

fn unit_details(unit: &UnitRef) -> Arc<UnitDetails> {
    Arc::new(UnitDetails {
        id: Arc::clone(&unit.name),
        description: Arc::from(""),
        documentation: Arc::from([]),
        exec_start_argv: Arc::from([]),
        fragment_path: None,
        drop_in_paths: Arc::from([]),
        startup: StartupType::Enabled,
        active_state: Arc::from("active"),
        sub_state: Arc::from("running"),
        active_enter_timestamp_us: None,
        main_pid: 0,
        memory_current: None,
        tasks_current: None,
        cpu_usage_nsec: None,
        manager: unit.manager,
        user: None,
        group: None,
        dynamic_user: false,
        supplementary_groups: Arc::from([]),
        pam_name: None,
        main_pid_ids: None,
        graphical_session: false,
        recovery: RecoverySettings::default(),
        recovery_dropin_exists: false,
        deps: Arc::from([]),
    })
}

fn services_msg(
    manager: Manager,
    names: &[&str],
    details: Vec<(UnitRef, Arc<UnitDetails>)>,
) -> EngineMsg {
    src(SourceUpdate::Services(ServicesUpdate {
        manager,
        state: ManagerState::Connected,
        units: names
            .iter()
            .map(|n| unit_rec(unit_ref(manager, n)))
            .collect(),
        details,
        backend: ServiceBackend::Systemd,
        validity: Validity::Valid,
    }))
}

#[test]
fn connections_counts() {
    let mut rig = Rig::new();
    rig.run(vec![connections_msg(vec![
        conn(1, Proto::Tcp, ConnState::Established),
        conn(2, Proto::Tcp, ConnState::Established),
        conn(3, Proto::Tcp, ConnState::Listening),
        conn(4, Proto::Udp, ConnState::Unconnected),
    ])])
    .expect("no invariant violation");
    let c = rig.latest().connections.counts;
    assert_eq!(
        (c.total, c.tcp, c.udp, c.established, c.listening),
        (4, 3, 1, 2, 1)
    );
    assert_eq!(rig.latest().connections.records.len(), 4);
    rig.passed();
}

#[test]
fn connections_counts_follow_text_filter() {
    let mut rig = Rig::new();
    rig.run(vec![
        EngineMsg::Command(Command::SetSysTableQuery(SysTableQuery {
            table: SysTable::Connections,
            generation: 1,
            sort: SysSortSpec {
                column: SysColumnId::all(SysTable::Connections)[0],
                ascending: true,
            },
            filter: Arc::from("listen"),
            expanded: Arc::from([]),
            services: None,
        })),
        connections_msg(vec![
            conn(1, Proto::Tcp, ConnState::Established),
            conn(2, Proto::Tcp, ConnState::Established),
            conn(3, Proto::Tcp, ConnState::Listening),
        ]),
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.connections.records.len(), 3);
    assert_eq!(snap.connections.rows.len(), 1);
    let c = snap.connections.counts;
    assert_eq!(
        (c.total, c.tcp, c.udp, c.established, c.listening),
        (1, 1, 0, 0, 1)
    );
    rig.passed();
}

#[test]
fn set_sys_table_query_echoes_generation() {
    let mut rig = Rig::new();
    let before = rig.latest().services.header.version;
    rig.run(vec![EngineMsg::Command(Command::SetSysTableQuery(
        SysTableQuery {
            table: SysTable::Services,
            generation: 5,
            sort: SysSortSpec {
                column: SysColumnId::all(SysTable::Services)[0],
                ascending: true,
            },
            filter: Arc::from(""),
            expanded: Arc::from([]),
            services: None,
        },
    ))])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.services.query_generation, 5);
    assert!(
        snap.services.header.version > before,
        "SERVICES was marked even with no rows"
    );
    rig.passed();
}

#[test]
fn services_merge_two_managers() {
    let mut rig = Rig::new();
    rig.run(vec![services_msg(
        Manager::User,
        &["a.service"],
        Vec::new(),
    )])
    .expect("no invariant violation");
    rig.run(vec![services_msg(
        Manager::System,
        &["b.service", "c.service"],
        Vec::new(),
    )])
    .expect("no invariant violation");
    let snap = rig.latest();
    let units: Vec<_> = snap.services.units.iter().map(|u| u.unit.clone()).collect();
    assert_eq!(
        units,
        vec![
            unit_ref(Manager::System, "b.service"),
            unit_ref(Manager::System, "c.service"),
            unit_ref(Manager::User, "a.service"),
        ]
    );
    assert_eq!(
        snap.services.managers,
        [ManagerState::Connected, ManagerState::Connected]
    );

    // A new User update replaces only the User units.
    rig.run(vec![services_msg(
        Manager::User,
        &["d.service"],
        Vec::new(),
    )])
    .expect("no invariant violation");
    let names: Vec<_> = rig
        .latest()
        .services
        .units
        .iter()
        .map(|u| u.unit.name.to_string())
        .collect();
    assert_eq!(names, ["b.service", "c.service", "d.service"]);
    rig.passed();
}

#[test]
fn services_details_copied_and_retained() {
    let mut rig = Rig::new();
    let a = unit_ref(Manager::System, "a.service");
    let b = unit_ref(Manager::System, "b.service");
    let (demand_tx, demand_rx) = watch::channel(Demand {
        unit_details: Arc::from([a.clone(), b.clone()]),
        ..Demand::default()
    });
    rig.reducer().demand_rx = Some(demand_rx);
    let (da, db) = (unit_details(&a), unit_details(&b));
    let user = unit_ref(Manager::User, "u.service");
    let du = unit_details(&user);
    rig.run(vec![
        services_msg(
            Manager::System,
            &["a.service", "b.service"],
            vec![(a.clone(), Arc::clone(&da)), (b.clone(), Arc::clone(&db))],
        ),
        // Not demanded: dropped.
        services_msg(Manager::User, &["u.service"], vec![(user.clone(), du)]),
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.services.details.len(), 2);
    assert!(Arc::ptr_eq(&snap.services.details[&a], &da));
    assert!(!snap.services.details.contains_key(&user));

    demand_tx
        .send(Demand {
            unit_details: Arc::from([a.clone()]),
            ..Demand::default()
        })
        .expect("the reducer holds the receiver");
    rig.run(vec![EngineMsg::Tick])
        .expect("no invariant violation");
    let snap = rig.latest();
    assert!(Arc::ptr_eq(&snap.services.details[&a], &da));
    assert!(!snap.services.details.contains_key(&b));
    rig.passed();
}

#[test]
fn destroy_supported_persists() {
    let mut rig = Rig::new();
    rig.run(vec![src(SourceUpdate::ConnectionsDestroySupport(false))])
        .expect("no invariant violation");
    assert_eq!(rig.latest().connections.destroy_supported, Some(false));
    rig.run(vec![connections_msg(vec![conn(
        1,
        Proto::Tcp,
        ConnState::Established,
    )])])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.connections.destroy_supported, Some(false));
    assert_eq!(snap.connections.records.len(), 1);
    rig.passed();
}

const BUS: &str = "0000:01:00.0";

fn gpu_info_msg() -> EngineMsg {
    src(SourceUpdate::GpuInfo(GpuInfoUpdate {
        devices: vec![GpuStaticInfo {
            identity: DeviceIdentity::Gpu {
                uuid: [1; 16],
                pci_bus_id: Arc::from(BUS),
            },
            name: Arc::from("Card"),
            mem_total: 0,
            power_limit_mw: None,
            driver: Arc::from(""),
            nvml: Arc::from(""),
            supports_process_util: false,
            persistent_key: Arc::from("gpu:card"),
        }],
    }))
}

fn api_versions_msg() -> EngineMsg {
    src(SourceUpdate::GpuApiVersions(ApiVersions {
        devices: vec![GpuApiVersions {
            pci_bus_id: BUS.to_string(),
            vulkan_api: Some("1.4.312".to_string()),
            vulkan_driver: None,
            opengl: Some("4.6".to_string()),
        }],
    }))
}

fn inventory_msg(device: DeviceIdx) -> EngineMsg {
    src(SourceUpdate::Inventory(Box::new(InventorySnapshot {
        graphics: Arc::from([GraphicsAdapter {
            device: Some(device),
            ..GraphicsAdapter::default()
        }]),
        ..InventorySnapshot::default()
    })))
}

fn gpu_idx(rig: &Rig) -> DeviceIdx {
    rig.latest()
        .devices
        .find(&DeviceIdentity::Gpu {
            uuid: [1; 16],
            pci_bus_id: Arc::from(BUS),
        })
        .expect("GpuInfo added the device")
        .idx
}

fn assert_versions(rig: &Rig) {
    let snap = rig.latest();
    let g = &snap.inventory.graphics[0];
    assert_eq!(&*g.vulkan_version.value, "1.4.312");
    assert_eq!(g.vulkan_version.validity, Validity::Valid);
    assert_eq!(&*g.opengl_version.value, "4.6");
    assert_eq!(g.opengl_version.validity, Validity::Valid);
}

#[test]
fn gpu_api_versions_merge_order_independent() {
    // Inventory first, then the versions.
    let mut rig = Rig::new();
    rig.run(vec![gpu_info_msg()])
        .expect("no invariant violation");
    let idx = gpu_idx(&rig);
    rig.run(vec![inventory_msg(idx), api_versions_msg()])
        .expect("no invariant violation");
    assert_versions(&rig);
    rig.passed();

    // The versions first: kept and merged when Inventory arrives.
    let mut rig = Rig::new();
    rig.run(vec![gpu_info_msg(), api_versions_msg()])
        .expect("no invariant violation");
    let idx = gpu_idx(&rig);
    rig.run(vec![inventory_msg(idx)])
        .expect("no invariant violation");
    assert_versions(&rig);
    rig.passed();
}

#[test]
fn connections_failed_health_after_update() {
    let mut rig = Rig::new();
    rig.run(vec![connections_msg(Vec::new())])
        .expect("no invariant violation");
    assert_eq!(rig.latest().connections.header.validity, Validity::Valid);
    rig.run(vec![src(SourceUpdate::Health {
        id: SourceId::Connections,
        state: SourceState::Failed,
        last_error: None,
        t_ns: 0,
    })])
    .expect("no invariant violation");
    assert_eq!(rig.latest().connections.header.validity, Validity::Failed);
    rig.passed();
}

#[test]
fn connection_attribution_fills_process_network() {
    let mut rig = Rig::new();
    rig.run(vec![sweep_of(SEC, &[1, 2])])
        .expect("no invariant violation");
    let mut attribution = rustc_hash::FxHashMap::default();
    attribution.insert(
        proc_key(1),
        tl_collect::net::NetRate {
            rx_bps: 1000.0,
            tx_bps: 250.0,
            shared: false,
            validity: Validity::Valid,
        },
    );
    rig.run(vec![src(SourceUpdate::Connections(ConnectionsUpdate {
        t_ns: SEC,
        records: Vec::new(),
        validity: Validity::Valid,
        attribution,
    }))])
    .expect("no invariant violation");

    let check = |snap: &Snapshot| {
        let r = snap.processes.get(proc_key(1)).expect("pid 1 is live");
        assert_eq!((r.net_rx_bps, r.net_tx_bps), (1000.0, 250.0));
        assert_eq!(r.validity.get(ProcessField::NetRx), Validity::Valid);
        let other = snap.processes.get(proc_key(2)).expect("pid 2 is live");
        assert_eq!((other.net_rx_bps, other.net_tx_bps), (0.0, 0.0));
        assert_eq!(other.validity.get(ProcessField::NetTx), Validity::Valid);
    };
    check(&rig.latest());

    // A new sweep keeps the rates until the next ConnectionsUpdate.
    rig.run(vec![sweep_of(2 * SEC, &[1, 2])])
        .expect("no invariant violation");
    check(&rig.latest());
    rig.passed();
}

fn startup_entry(
    name: &str,
    status: tl_model::snapshot::StartupStatus,
) -> tl_model::snapshot::StartupEntry {
    use tl_model::snapshot::{Impact, StartupEntry, StartupKind};
    use tl_model::table::{StartupRef, StartupSource};
    StartupEntry {
        id: StartupRef {
            source: StartupSource::AutostartUser,
            id: format!("{name}.desktop").into(),
        },
        name: name.into(),
        icon_path: None,
        publisher: None,
        status,
        impact: Impact::NotMeasured,
        impact_detail: None,
        state_code: None,
        kind: StartupKind::AutostartUser,
        location: "/home/u/.config/autostart".into(),
        command: name.into(),
        source_path: format!("/home/u/.config/autostart/{name}.desktop").into(),
        can_toggle: true,
        can_delete: true,
        can_run: true,
        overridden_by_user: false,
    }
}

#[test]
fn startup_counts_follow_text_filter() {
    use tl_model::snapshot::StartupStatus;
    let mut rig = Rig::new();
    rig.run(vec![src(SourceUpdate::Startup(StartupUpdate {
        entries: vec![
            startup_entry("alpha", StartupStatus::Enabled),
            startup_entry("beta", StartupStatus::Enabled),
            startup_entry("zebra", StartupStatus::Disabled),
        ],
        validity: Validity::Valid,
    }))])
    .expect("no invariant violation");
    let c = rig.latest().startup.counts;
    assert_eq!((c.total, c.enabled, c.disabled), (3, 2, 1));

    rig.run(vec![cmd(Command::SetSysTableQuery(SysTableQuery {
        table: SysTable::Startup,
        generation: 2,
        sort: SysSortSpec {
            column: SysColumnId::all(SysTable::Startup)[0],
            ascending: true,
        },
        filter: Arc::from("zebra"),
        expanded: Arc::from([]),
        services: None,
    }))])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.startup.rows.len(), 1);
    let c = snap.startup.counts;
    assert_eq!((c.total, c.enabled, c.disabled), (1, 0, 1));
    rig.passed();
}

use tl_model::snapshot::DiskDrive;

#[test]
fn inventory_devices_from_pci_slot_and_disk_name() {
    // The worker sends `device: None` with the slot or name; the reducer finds the catalog entry.
    let mut rig = Rig::new();
    let disk = disk_id("nvme0n1", 0);
    rig.run(vec![
        gpu_info_msg(),
        api_versions_msg(),
        disks_msg(SEC, vec![(disk.clone(), disk_totals(1.0, 1.0))]),
    ])
    .expect("no invariant violation");
    let gpu = gpu_idx(&rig);
    let disk_idx = rig.idx(&disk);
    rig.run(vec![src(SourceUpdate::Inventory(Box::new(
        InventorySnapshot {
            graphics: Arc::from([
                GraphicsAdapter {
                    pci_slot: Some(Arc::from(BUS.to_ascii_uppercase())),
                    ..GraphicsAdapter::default()
                },
                GraphicsAdapter {
                    pci_slot: Some(Arc::from("0000:09:00.0")),
                    ..GraphicsAdapter::default()
                },
            ]),
            disks: Arc::from([
                DiskDrive {
                    name: Some(Arc::from("nvme0n1")),
                    ..DiskDrive::default()
                },
                DiskDrive {
                    name: Some(Arc::from("sr0")),
                    ..DiskDrive::default()
                },
            ]),
            ..InventorySnapshot::default()
        },
    )))])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.inventory.graphics[0].device, Some(gpu));
    assert_versions(&rig);
    let other = &snap.inventory.graphics[1];
    assert_eq!(other.device, None);
    assert_eq!(other.vulkan_version.validity, Validity::WarmingUp);
    assert_eq!(snap.inventory.disks[0].device, Some(disk_idx));
    assert_eq!(snap.inventory.disks[1].device, None);
    rig.passed();
}

#[test]
fn inventory_devices_remapped_when_catalog_gains_nvidia_gpu_or_disk() {
    // Inventory first: no catalog entry matches yet.
    let mut rig = Rig::new();
    rig.run(vec![
        api_versions_msg(),
        src(SourceUpdate::Inventory(Box::new(InventorySnapshot {
            graphics: Arc::from([GraphicsAdapter {
                pci_slot: Some(Arc::from(BUS)),
                ..GraphicsAdapter::default()
            }]),
            disks: Arc::from([DiskDrive {
                name: Some(Arc::from("nvme0n1")),
                ..DiskDrive::default()
            }]),
            ..InventorySnapshot::default()
        }))),
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.inventory.graphics[0].device, None);
    assert_eq!(snap.inventory.disks[0].device, None);

    // NVML names the bus with an 8-digit domain; the adapter and the versions use 4 digits.
    let nvml = DeviceIdentity::Gpu {
        uuid: [2; 16],
        pci_bus_id: Arc::from("00000000:01:00.0"),
    };
    let disk = disk_id("nvme0n1", 0);
    rig.run(vec![
        src(SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![GpuStaticInfo {
                identity: nvml.clone(),
                name: Arc::from("NVIDIA Card"),
                mem_total: 0,
                power_limit_mw: None,
                driver: Arc::from(""),
                nvml: Arc::from(""),
                supports_process_util: true,
                persistent_key: Arc::from("gpu:nvidia"),
            }],
        })),
        disks_msg(SEC, vec![(disk.clone(), disk_totals(1.0, 1.0))]),
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    assert_eq!(snap.inventory.graphics[0].device, Some(rig.idx(&nvml)));
    assert_versions(&rig);
    assert_eq!(snap.inventory.disks[0].device, Some(rig.idx(&disk)));
    rig.passed();
}

// ---- Mixed-vendor GPUs (tasklens-8q0.37) ----

fn gpu_id(uuid: [u8; 16], bus: &str) -> DeviceIdentity {
    DeviceIdentity::Gpu {
        uuid,
        pci_bus_id: Arc::from(bus),
    }
}

fn gpu_static(identity: &DeviceIdentity, supports_process_util: bool) -> GpuStaticInfo {
    GpuStaticInfo {
        identity: identity.clone(),
        name: Arc::from("Card"),
        mem_total: 0,
        power_limit_mw: None,
        driver: Arc::from(""),
        nvml: Arc::from(""),
        supports_process_util,
        persistent_key: Arc::from("gpu:card"),
    }
}

/// The `Devices` list the DRM worker sends for `cards`.
fn drm_list(cards: &[&DeviceIdentity]) -> EngineMsg {
    src(SourceUpdate::Devices(DeviceList {
        t_ns: now_boot_ns(),
        kinds: DeviceKinds::GPUS,
        entries: cards
            .iter()
            .map(|&id| DeviceDesc {
                identity: id.clone(),
                display_name: Arc::from("AMD Card"),
                parent: None,
                persistent_key: Arc::from("gpu:amd"),
                net_kind: None,
            })
            .collect(),
    }))
}

/// A GPU sample with measured power per card and no process rows.
fn gpu_sample(cards: &[(&DeviceIdentity, f32)]) -> EngineMsg {
    src(SourceUpdate::Gpu(GpuSample {
        t_ns: now_boot_ns(),
        devices: cards
            .iter()
            .map(|&(id, watts)| {
                (
                    id.clone(),
                    GpuTotals {
                        device: DeviceIdx(0),
                        util_pct: Metric::valid(10.0),
                        mem_util_pct: Metric::default(),
                        mem_used: Metric::valid(1024),
                        mem_total: Metric::valid(4096),
                        shared_used: Metric::default(),
                        shared_total: Metric::default(),
                        enc_pct: Metric::default(),
                        dec_pct: Metric::default(),
                        mem_reserved: Metric::default(),
                        enc_sessions: Metric::default(),
                        enc_dec_shared: false,
                        temp_c: Metric::default(),
                        temp_edge_c: Metric::default(),
                        temp_junction_c: Metric::default(),
                        temp_mem_c: Metric::default(),
                        power_w: Metric::valid(watts),
                        power_limit_w: Metric::default(),
                        clock_gfx_mhz: Metric::default(),
                        clock_mem_mhz: Metric::default(),
                        fan_pct: Metric::default(),
                        pcie_rx_bps: Metric::default(),
                        pcie_tx_bps: Metric::default(),
                    },
                )
            })
            .collect(),
        processes: None,
        drm_processes: None,
    }))
}

/// `gpu_sample` for `card` with these NVIDIA and DRM process rows.
fn gpu_rows_msg(
    card: &DeviceIdentity,
    nvidia: Option<Vec<GpuProcessSample>>,
    drm: Option<&[(u32, u64)]>,
) -> EngineMsg {
    let mut msg = gpu_sample(&[(card, 10.0)]);
    let pdev = match card {
        DeviceIdentity::Gpu { pci_bus_id, .. } => Arc::clone(pci_bus_id),
        _ => unreachable!("a GPU identity"),
    };
    if let EngineMsg::Source(SourceUpdate::Gpu(s)) = &mut msg {
        s.processes = nvidia;
        s.drm_processes = drm.map(|rows| {
            rows.iter()
                .map(|&(pid, vram_bytes)| DrmProcessVram {
                    pdev: Arc::clone(&pdev),
                    pid,
                    vram_bytes,
                })
                .collect()
        });
    }
    msg
}

/// `gpu_vram_bytes` of `pid` in `snap`.
fn vram_of(snap: &Snapshot, pid: u32) -> Vec<u64> {
    let i = snap
        .processes
        .records
        .iter()
        .position(|r| r.key == proc_key(pid))
        .expect("the record is published");
    snap.processes.extras_p2[i].gpu_vram_bytes.to_vec()
}

#[test]
fn drm_vram_valid_on_amd_only() {
    let mut rig = Rig::new();
    let amd = gpu_id([0; 16], "0000:03:00.0");
    rig.run(vec![
        drm_list(&[&amd]),
        src(SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![gpu_static(&amd, false)],
        })),
        sweep_without_pss(now_boot_ns(), &[1, 2]),
    ])
    .expect("no invariant violation");
    let swept = record_of(&rig.latest(), proc_key(1)).validity;
    // No worker sent process rows yet: the GPU fields keep the sweep's validity.
    rig.run(vec![gpu_sample(&[(&amd, 10.0)])])
        .expect("no invariant violation");
    assert_eq!(record_of(&rig.latest(), proc_key(1)).validity, swept);

    rig.run(vec![gpu_rows_msg(&amd, None, Some(&[(2, 4096)]))])
        .expect("no invariant violation");
    let snap = rig.latest();
    let busy = record_of(&snap, proc_key(2));
    assert_eq!(busy.gpu_mem_bytes, 4096);
    assert_eq!(busy.validity.get(ProcessField::GpuMem), Validity::Valid);
    assert_eq!(
        busy.validity.get(ProcessField::GpuUtil),
        Validity::Unsupported,
        "DRM gives no per-process utilization"
    );
    assert_eq!(vram_of(&snap, 2), [4096], "the AMD card is GPU 0");
    let idle = record_of(&snap, proc_key(1));
    assert_eq!(idle.gpu_mem_bytes, 0);
    assert_eq!(
        idle.validity.get(ProcessField::GpuMem),
        Validity::Valid,
        "a process without a DRM row uses no VRAM"
    );
    assert_eq!(
        idle.validity.get(ProcessField::GpuUtil),
        Validity::Unsupported
    );
    assert!(vram_of(&snap, 1).iter().all(|&b| b == 0));
    rig.passed();
}

#[test]
fn drm_vram_valid_with_nvidia_and_amd() {
    let mut rig = Rig::new();
    // Columns by PCI bus: NVIDIA GPU 0, AMD GPU 1.
    let nvidia = gpu_id([9; 16], "00000000:01:00.0");
    let amd = gpu_id([0; 16], "0000:03:00.0");
    let nv_row = |pid, mem| GpuProcessSample {
        uuid: [9; 16],
        pid,
        sm_pct: Reading::Ok(20),
        mem_pct: Reading::Ok(0),
        enc_pct: Reading::Ok(0),
        dec_pct: Reading::Ok(0),
        mem_used: Reading::Ok(mem),
    };
    rig.run(vec![
        src(SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![gpu_static(&nvidia, true)],
        })),
        drm_list(&[&amd]),
        src(SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![gpu_static(&amd, false)],
        })),
        sweep_without_pss(now_boot_ns(), &[1, 2, 3]),
        gpu_sample(&[(&nvidia, 10.0)]),
        gpu_sample(&[(&amd, 10.0)]),
    ])
    .expect("no invariant violation");
    // The GPU columns come from the published catalog, so the rows go in a later run.
    rig.run(vec![
        gpu_rows_msg(&nvidia, Some(vec![nv_row(1, 1000), nv_row(2, 2000)]), None),
        gpu_rows_msg(&amd, None, Some(&[(2, 300), (3, 400)])),
    ])
    .expect("no invariant violation");
    let snap = rig.latest();

    // NVIDIA only.
    let r = record_of(&snap, proc_key(1));
    assert_eq!(r.gpu_mem_bytes, 1000);
    assert_eq!(r.validity.get(ProcessField::GpuMem), Validity::Valid);
    assert_eq!(r.validity.get(ProcessField::GpuUtil), Validity::Valid);
    assert_eq!(vram_of(&snap, 1), [1000]);
    // Both cards.
    let r = record_of(&snap, proc_key(2));
    assert_eq!(r.gpu_mem_bytes, 2300);
    assert_eq!(r.validity.get(ProcessField::GpuMem), Validity::Valid);
    assert!((r.gpu_util_pct - 20.0).abs() < f32::EPSILON);
    assert_eq!(vram_of(&snap, 2), [2000, 300]);
    // AMD only: utilization is a valid 0 because the NVIDIA card reports per process.
    let r = record_of(&snap, proc_key(3));
    assert_eq!(r.gpu_mem_bytes, 400);
    assert_eq!(r.validity.get(ProcessField::GpuMem), Validity::Valid);
    assert_eq!(r.validity.get(ProcessField::GpuUtil), Validity::Valid);
    assert!(r.gpu_util_pct.abs() < f32::EPSILON);
    assert_eq!(vram_of(&snap, 3), [0, 400]);
    rig.passed();
}

#[test]
fn nvidia_and_amd_gpus_coexist() {
    let mut rig = Rig::new();
    let nvidia = gpu_id([9; 16], "00000000:01:00.0");
    let amd_a = gpu_id([0; 16], "0000:03:00.0");
    let amd_b = gpu_id([0; 16], "0000:04:00.0");
    rig.run(vec![
        src(SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![gpu_static(&nvidia, true)],
        })),
        drm_list(&[&amd_a, &amd_b]),
        src(SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![gpu_static(&amd_a, false), gpu_static(&amd_b, false)],
        })),
        gpu_sample(&[(&nvidia, 100.0)]),
        gpu_sample(&[(&amd_a, 30.0), (&amd_b, 40.0)]),
        gpu_sample(&[(&nvidia, 90.0)]),
        drm_list(&[&amd_a, &amd_b]),
    ])
    .expect("no invariant violation");

    let snap = rig.latest();
    let ids = [rig.idx(&nvidia), rig.idx(&amd_a), rig.idx(&amd_b)];
    for id in [&nvidia, &amd_a, &amd_b] {
        let entry = snap.devices.find(id).expect("device in the catalog");
        assert!(entry.present, "{id:?} stays present");
    }
    let mut rows: Vec<DeviceIdx> = snap.system.gpus.iter().map(|(idx, _)| *idx).collect();
    rows.sort_unstable();
    let mut want = ids.to_vec();
    want.sort_unstable();
    assert_eq!(
        rows, want,
        "a sample from one worker keeps the other worker's rows"
    );

    let state = &rig.reducer().apply_state;
    assert_eq!(state.gpu_cap(&[9; 16]), Some(true), "NVIDIA caps stay");
    assert_eq!(state.gpu_cap(&[0; 16]), Some(false), "AMD caps stay");
    assert_eq!(state.gpu_power_w(ids[0]), Some(90.0));
    assert_eq!(
        state.gpu_power_w(ids[1]),
        Some(30.0),
        "one entry per AMD card"
    );
    assert_eq!(
        state.gpu_power_w(ids[2]),
        Some(40.0),
        "one entry per AMD card"
    );

    // The DRM worker loses one card: only that card leaves.
    rig.run(vec![
        drm_list(&[&amd_a]),
        src(SourceUpdate::GpuInfo(GpuInfoUpdate {
            devices: vec![gpu_static(&amd_a, false)],
        })),
        gpu_sample(&[(&amd_a, 35.0)]),
    ])
    .expect("no invariant violation");
    let snap = rig.latest();
    let present = |id: &DeviceIdentity| snap.devices.find(id).is_some_and(|e| e.present);
    assert!(present(&nvidia));
    assert!(present(&amd_a));
    assert!(!present(&amd_b));
    let rows: Vec<DeviceIdx> = snap.system.gpus.iter().map(|(idx, _)| *idx).collect();
    assert!(rows.contains(&ids[0]) && rows.contains(&ids[1]));
    assert!(!rows.contains(&ids[2]));
    assert_eq!(rig.reducer().apply_state.gpu_cap(&[9; 16]), Some(true));
    rig.passed();
}
