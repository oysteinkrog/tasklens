//! LabRuntime tests for the sampling loop in `source.rs` (ADR-004, ADR-013).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::source::{
    self, CommandProgress, QueuedCommand, Source, WorkerCommand, WorkerControl, WorkerIo,
};
use tl_engine::update::{DeviceKinds, DeviceList, EngineMsg, SourceUpdate};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::settings::EngineSettings;

/// Always on at 100 ms. Its first sample leaves one `Devices` extra behind.
struct ExtraSource {
    extra: Option<SourceUpdate>,
    sampled: bool,
}

impl Source for ExtraSource {
    fn id(&self) -> SourceId {
        SourceId::Disk
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(Duration::from_millis(100))
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        if !self.sampled {
            self.sampled = true;
            self.extra = Some(SourceUpdate::Devices(DeviceList {
                t_ns: 0,
                kinds: DeviceKinds::DISKS,
                entries: Vec::new(),
            }));
        }
        let update = SourceUpdate::Health {
            id: SourceId::Disk,
            state: SourceState::Running,
            last_error: None,
            t_ns: 0,
        };
        (self, update)
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.extra.take()
    }
}

#[test]
fn extras_sent_before_sample() {
    let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(1_000_000));
    let (updates, mut rx) = mpsc::channel::<EngineMsg>(256);
    let (_demand_tx, demand) = watch::channel(Demand::default());
    let (_settings_tx, settings) = watch::channel(EngineSettings::default());
    let (_control_tx, control) = watch::channel(WorkerControl::default());
    let io = WorkerIo::new(updates, demand, settings, control);
    let src = ExtraSource {
        extra: None,
        sampled: false,
    };

    let root = lab.state.create_root_region(Budget::INFINITE);
    let (task, handle) = lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("task has a Cx");
            source::run(cx, io, src).await
        })
        .expect("task is created");
    lab.scheduler.lock().schedule(task, 0);
    lab.run_until_idle();

    let first = rx.try_recv().expect("the extra arrived");
    assert!(
        matches!(first, EngineMsg::Source(SourceUpdate::Devices(_))),
        "first message {first:?}"
    );
    let second = rx.try_recv().expect("the sample arrived");
    assert!(
        matches!(
            second,
            EngineMsg::Source(SourceUpdate::Health {
                state: SourceState::Running,
                ..
            })
        ),
        "second message {second:?}"
    );
    assert!(rx.try_recv().is_err(), "one sample, one extra");

    handle.abort();
    let report = lab.run_until_quiescent_with_report();
    assert!(report.lab_test_passed(), "{report:?}");
}

type Log = Arc<Mutex<Vec<&'static str>>>;

/// Logs each command kind and each sample. Panics on the command kind in `crash_on`, or on
/// sample number `crash_on_sample` (1-based).
struct CmdSource {
    log: Log,
    period: Option<Duration>,
    crash_on: Option<&'static str>,
    crash_on_sample: Option<u32>,
    samples: u32,
}

impl CmdSource {
    fn new(period: Option<Duration>) -> (Self, Log) {
        let log = Log::default();
        let src = CmdSource {
            log: Arc::clone(&log),
            period,
            crash_on: None,
            crash_on_sample: None,
            samples: 0,
        };
        (src, log)
    }
}

impl Source for CmdSource {
    fn id(&self) -> SourceId {
        SourceId::Udisks
    }

    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        self.period
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        self.samples += 1;
        self.log.lock().unwrap().push("sample");
        assert_ne!(Some(self.samples), self.crash_on_sample, "sample crash");
        let update = SourceUpdate::Health {
            id: SourceId::Udisks,
            state: SourceState::Running,
            last_error: None,
            t_ns: 0,
        };
        (self, update)
    }

    fn command(&mut self, cmd: &WorkerCommand) -> bool {
        self.log.lock().unwrap().push(cmd.kind());
        assert_ne!(Some(cmd.kind()), self.crash_on, "command crash");
        true
    }
}

/// One lab with the control watch and the reducer's queue.
struct Rig {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    rx: mpsc::Receiver<EngineMsg>,
    io: WorkerIo,
    control_tx: watch::Sender<WorkerControl>,
    _demand_tx: watch::Sender<Demand>,
    _settings_tx: watch::Sender<EngineSettings>,
    progress: Arc<CommandProgress>,
    queue: Vec<QueuedCommand>,
}

impl Rig {
    fn new() -> Self {
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(1_000_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (updates, rx) = mpsc::channel::<EngineMsg>(256);
        let (demand_tx, demand) = watch::channel(Demand::default());
        let (settings_tx, settings) = watch::channel(EngineSettings::default());
        let control = WorkerControl::default();
        let progress = Arc::clone(&control.progress);
        let (control_tx, control) = watch::channel(control);
        let io = WorkerIo::new(updates, demand, settings, control);
        Rig {
            lab,
            root,
            rx,
            io,
            control_tx,
            _demand_tx: demand_tx,
            _settings_tx: settings_tx,
            progress,
            queue: Vec::new(),
        }
    }

    /// What the reducer's `forward()` does: append, keep the same progress `Arc`, send.
    fn forward(&mut self, command: WorkerCommand) {
        let seq = self.queue.last().map_or(0, |c| c.seq) + 1;
        self.queue.push(QueuedCommand {
            seq,
            target: SourceId::Udisks,
            command,
        });
        let control = WorkerControl {
            reset_generation: 0,
            commands: self.queue.clone().into(),
            progress: Arc::clone(&self.progress),
        };
        self.control_tx.send(control).expect("watch has receivers");
    }

    /// Starts one worker generation and runs the lab until it is idle.
    fn generation(&mut self, src: CmdSource) -> asupersync::runtime::TaskHandle<()> {
        let io = self.io.clone();
        let (task, handle) = self
            .lab
            .state
            .create_task(self.root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let _ = source::run(cx, io, src).await;
            })
            .expect("task is created");
        self.lab.scheduler.lock().schedule(task, 0);
        self.lab.run_until_idle();
        handle
    }

    fn advance(&mut self, d: Duration) {
        self.lab.advance_time(d.as_nanos() as u64);
        self.lab.run_until_idle();
    }

    /// `last_error` of every `Failed` health message received so far.
    fn failures(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            if let EngineMsg::Source(SourceUpdate::Health {
                state: SourceState::Failed,
                last_error,
                ..
            }) = msg
            {
                out.push(last_error.as_deref().unwrap_or_default().to_owned());
            }
        }
        out
    }
}

fn entries(log: &Log) -> Vec<&'static str> {
    log.lock().unwrap().clone()
}

#[test]
fn crashing_command_is_not_rerun() {
    let mut rig = Rig::new();
    rig.forward(WorkerCommand::Reprobe);
    let (mut gen1, log1) = CmdSource::new(None);
    gen1.crash_on = Some("Reprobe");
    rig.generation(gen1);
    assert_eq!(entries(&log1), ["Reprobe"]);
    assert_eq!(rig.progress.started(SourceId::Udisks), 1);
    assert_eq!(rig.progress.done(SourceId::Udisks), 0);

    let (gen2, log2) = CmdSource::new(None);
    let handle = rig.generation(gen2);
    rig.advance(Duration::from_secs(2));
    assert!(entries(&log2).is_empty(), "rerun: {:?}", entries(&log2));
    assert_eq!(
        rig.failures(),
        ["Reprobe was not run again after the worker restarted"]
    );
    assert_eq!(rig.progress.done(SourceId::Udisks), 1);

    handle.abort();
    rig.lab.run_until_idle();
}

#[test]
fn command_queued_during_restart_runs_once() {
    let mut rig = Rig::new();
    rig.forward(WorkerCommand::Reprobe);
    let (mut gen1, _log1) = CmdSource::new(None);
    gen1.crash_on = Some("Reprobe");
    rig.generation(gen1);

    // Queued while no generation runs.
    rig.forward(WorkerCommand::SetupStatus);
    let (gen2, log2) = CmdSource::new(None);
    let handle = rig.generation(gen2);
    rig.advance(Duration::from_secs(2));
    assert_eq!(entries(&log2), ["SetupStatus", "sample"]);
    assert_eq!(rig.progress.started(SourceId::Udisks), 2);
    assert_eq!(rig.progress.done(SourceId::Udisks), 2);

    handle.abort();
    rig.lab.run_until_idle();
}

#[test]
fn sample_crash_blames_no_command() {
    let period = Some(Duration::from_millis(100));
    let mut rig = Rig::new();
    rig.forward(WorkerCommand::Reprobe);
    let (mut gen1, log1) = CmdSource::new(period);
    gen1.crash_on_sample = Some(2);
    rig.generation(gen1);
    rig.advance(Duration::from_millis(100));
    assert_eq!(entries(&log1), ["Reprobe", "sample", "sample"]);
    assert_eq!(rig.progress.done(SourceId::Udisks), 1);

    let (gen2, log2) = CmdSource::new(period);
    let handle = rig.generation(gen2);
    assert_eq!(entries(&log2), ["sample"]);
    assert!(rig.failures().is_empty());

    handle.abort();
    rig.lab.run_until_idle();
}
