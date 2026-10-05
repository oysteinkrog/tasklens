//! LabRuntime tests for the `HistoryRecorder` worker in rolling-window mode (ADR-012, ADR-013).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use asupersync::channel::{mpsc, oneshot, watch};
use asupersync::runtime::TaskHandle;
use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::types::{Budget, Outcome, RegionId};
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::dbus::SharedHandles;
use tl_engine::source::{WorkerControl, WorkerIo};
use tl_engine::supervise::{WorkerFactory, build_supervisor};
use tl_engine::update::{EngineMsg, SourceUpdate, WorkerError};
use tl_engine::workers::history::{
    HistoryMsg, HistorySink, HistorySlot, HistoryWorkerConfig, Mode, run_history,
};
use tl_model::command::HistoryWhat;
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::history::{HistoryMetric, HistoryRange, HistorySource, HistoryView, RecordSink};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::EngineSettings;
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};

const SEC: u64 = 1_000_000_000;
const MIN: u64 = 60 * SEC;
/// Sweep times start here, like a machine that booted a while ago.
const T0: u64 = 1_000 * SEC;
const BOOT: BootId = BootId(1);

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid),
        boot_id: BOOT,
        pid,
    }
}

fn sweep(n: u32) -> Arc<[ProcessRecord]> {
    let meta = Arc::new(ProcessMeta {
        comm: "x".into(),
        name: "x".into(),
        exe: None,
        cmdline: None,
        user: "user".into(),
        cgroup: None,
        app_id: None,
        icon: None,
        description: None,
        search_lower: "x".into(),
    });
    (1..=n)
        .map(|pid| ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: meta.clone(),
            group: GroupKey(0),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: f32::from(u8::try_from(pid % 50).unwrap_or(0)),
            rss_anon_bytes: u64::from(pid) << 20,
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
            validity: FieldValidity::all(Validity::Valid),
        })
        .collect()
}

fn config() -> HistoryWorkerConfig {
    HistoryWorkerConfig {
        boot: BOOT,
        history_dir: None,
    }
}

fn root(lab: &mut LabRuntime) -> RegionId {
    match lab.state.root_region {
        Some(root) => root,
        None => lab.state.create_root_region(Budget::INFINITE),
    }
}

struct Rig {
    lab: LabRuntime,
    rx: mpsc::Receiver<EngineMsg>,
    demand_tx: watch::Sender<Demand>,
    _settings_tx: watch::Sender<EngineSettings>,
    _control_tx: watch::Sender<WorkerControl>,
    io: WorkerIo,
    slot: HistorySlot,
    sink: HistorySink,
    next_id: u64,
}

impl Rig {
    fn new(process_table: bool) -> Self {
        let lab = LabRuntime::new(
            LabConfig::new(7)
                .max_steps(100_000_000)
                // 15 virtual minutes of sweeps overflow the default 4,096-event trace.
                .trace_capacity(1 << 20),
        );
        let (updates, rx) = mpsc::channel::<EngineMsg>(256);
        let (demand_tx, demand) = watch::channel(Demand {
            process_table,
            ..Demand::default()
        });
        let (settings_tx, settings) = watch::channel(EngineSettings::default());
        let (control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(updates, demand, settings, control, SharedHandles::default());
        let slot: HistorySlot = Arc::new(ArcSwapOption::empty());
        let sink = HistorySink::new(Arc::clone(&slot));
        Rig {
            lab,
            rx,
            demand_tx,
            _settings_tx: settings_tx,
            _control_tx: control_tx,
            io,
            slot,
            sink,
            next_id: 0,
        }
    }

    /// Runs one worker generation as a root task.
    fn spawn_worker(&mut self) -> TaskHandle<Outcome<(), WorkerError>> {
        let io = self.io.clone();
        let slot = Arc::clone(&self.slot);
        let root = root(&mut self.lab);
        let (task, handle) = self
            .lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                run_history(cx, io, slot, config()).await
            })
            .expect("task is created");
        self.lab.scheduler.lock().schedule(task, 0);
        self.lab.run_until_idle();
        handle
    }

    fn advance(&mut self, d: Duration) {
        self.lab.advance_time(d.as_nanos() as u64);
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        self.lab.step_for_test();
        self.lab.run_until_idle();
    }

    /// One sweep per second from `from` for `secs` seconds, with virtual time moving along.
    fn sweeps(&mut self, from: u64, secs: u64, records: &Arc<[ProcessRecord]>) {
        for s in 0..secs {
            self.sink.on_process_sweep(from + s * SEC, records);
            self.advance(Duration::from_secs(1));
        }
    }

    /// Sends a Live Frame query and returns its answer.
    fn query_live(&mut self) -> Arc<HistoryView> {
        self.next_id += 1;
        let id = self.next_id;
        self.sink.command(HistoryMsg::Query {
            id,
            range: HistoryRange::Live,
            what: HistoryWhat::Frame {
                metric: HistoryMetric::Cpu,
                view: (0, u64::MAX),
                keys: Arc::from([]),
                overview: false,
            },
        });
        self.lab.run_until_idle();
        while let Ok(msg) = self.rx.try_recv() {
            if let EngineMsg::Source(SourceUpdate::History(view)) = msg
                && view.query_id == id
            {
                return view;
            }
        }
        panic!("no answer to query {id}");
    }

    /// Runs `f` on the worker's mode, in message order, and returns its result.
    fn inspect<T: Send + 'static>(&mut self, f: impl FnOnce(&Mode) -> T + Send + 'static) -> T {
        let out: Arc<Mutex<Option<T>>> = Arc::new(Mutex::new(None));
        let put = Arc::clone(&out);
        self.sink.command(HistoryMsg::Inspect(Box::new(move |mode| {
            *put.lock().expect("inspect lock") = Some(f(mode));
        })));
        self.lab.run_until_idle();
        let got = out.lock().expect("inspect lock").take();
        got.expect("the worker ran the inspect message")
    }

    /// `last_error` of every HistoryRecorder health message received so far.
    fn health_texts(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            if let EngineMsg::Source(SourceUpdate::Health {
                id: SourceId::HistoryRecorder,
                state,
                last_error,
                ..
            }) = msg
            {
                assert_eq!(state, SourceState::Running, "R-249 keeps the state");
                out.push(last_error.as_deref().unwrap_or_default().to_owned());
            }
        }
        out
    }

    /// 600 sweeps while the worker cannot run: 512 fill the queue, 88 are dropped. Then the
    /// worker drains the queue and one more sweep carries the new drop total.
    fn overflow(&mut self, records: &Arc<[ProcessRecord]>) {
        for t in 0..600 {
            self.sink.on_process_sweep(T0 + t, records);
        }
        self.lab.run_until_idle();
        self.sink.on_process_sweep(T0 + 600, records);
        self.lab.run_until_idle();
    }

    fn finish(mut self, handle: &TaskHandle<Outcome<(), WorkerError>>) {
        handle.abort();
        let report = self.lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }
}

#[test]
fn rolling_window_keeps_10_minutes() {
    let mut rig = Rig::new(true);
    let handle = rig.spawn_worker();
    let records = sweep(20);
    rig.sweeps(T0, 15 * 60, &records);

    let view = rig.query_live();
    assert_eq!(view.source, HistorySource::RollingWindow);
    let (from, to) = view.span.expect("the window has a span");
    let span = to - from;
    assert!(
        (10 * MIN..=11 * MIN).contains(&span),
        "span {} s",
        span / SEC
    );
    assert_eq!(view.range, HistoryRange::Live);
    rig.finish(&handle);
}

#[test]
fn leaving_processes_tab_drops_window() {
    let mut rig = Rig::new(true);
    let handle = rig.spawn_worker();
    let records = sweep(5);
    rig.sweeps(T0, 90, &records);
    assert_eq!(rig.query_live().source, HistorySource::RollingWindow);

    rig.demand_tx
        .send(Demand::default())
        .expect("demand watch open");
    rig.advance(Duration::from_millis(500));
    // The 500 ms recv timeout dropped the window before any message arrived.
    let off = rig.inspect(|mode| matches!(mode, Mode::Off));
    assert!(off, "the window is dropped");

    let view = rig.query_live();
    assert_eq!(view.source, HistorySource::None);
    assert_eq!(view.span, None);
    assert!(view.rows.is_empty());
    rig.finish(&handle);
}

#[test]
fn restart_reinstalls_sender() {
    let mut rig = Rig::new(true);
    let io = rig.io.clone();
    let slot = Arc::clone(&rig.slot);
    // Generation 1 (numbers start at one) runs the worker for 1 s, then panics. Later ones run
    // it for good.
    let factory: WorkerFactory = Arc::new(move |cx: Cx, generation: ManagedGeneration| {
        let io = io.clone();
        let slot = Arc::clone(&slot);
        async move {
            if generation.number == 1 {
                let run = run_history(cx.clone(), io, slot, config());
                let _ = time::timeout(cx.now(), Duration::from_secs(1), run).await;
                panic!("generation 1 panics");
            }
            run_history(cx, io, slot, config()).await
        }
    });

    let (abort_tx, mut abort_rx) = oneshot::channel::<()>();
    let root = root(&mut rig.lab);
    let (task, handle) = rig
        .lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("task has a Cx");
            let supervisor = build_supervisor("history", factory).expect("supervisor builds");
            let mut sup = supervisor.spawn(&cx).expect("supervisor spawns");
            let _ = abort_rx.recv(&cx).await;
            sup.abort();
            let _ = sup.join().await;
        })
        .expect("task is created");
    rig.lab.scheduler.lock().schedule(task, 0);
    rig.lab.run_until_idle();

    let first = rig
        .slot
        .load_full()
        .expect("generation 1 installed a sender");
    // Generation 1 panics at 1 s; the restart backoff is 500 ms.
    rig.advance(Duration::from_secs(1));
    assert!(
        rig.slot.load().is_none(),
        "the panicking generation cleared the slot"
    );
    rig.advance(Duration::from_secs(1));
    let second = rig
        .slot
        .load_full()
        .expect("generation 2 installed a sender");
    assert!(!Arc::ptr_eq(&first, &second), "a new sender");
    drop(first);
    drop(second);

    rig.sweeps(T0, 5, &sweep(3));
    assert_eq!(rig.query_live().source, HistorySource::RollingWindow);

    abort_tx.send_blocking(()).expect("abort sent");
    let report = rig.lab.run_until_quiescent_with_report();
    assert!(report.lab_test_passed(), "{report:?}");
    drop(handle);
}

#[test]
fn cancel_clears_slot() {
    let mut rig = Rig::new(true);
    let handle = rig.spawn_worker();
    assert!(rig.slot.load().is_some(), "the worker installed its sender");
    rig.sweeps(T0, 3, &sweep(3));

    handle.abort();
    let report = rig.lab.run_until_quiescent_with_report();
    assert!(report.lab_test_passed(), "{report:?}");
    assert!(rig.slot.load().is_none(), "the slot is empty");
}

#[test]
fn session_start_marks_gap() {
    let mut rig = Rig::new(true);
    let handle = rig.spawn_worker();
    let records = sweep(4);
    rig.sweeps(T0, 30, &records);
    let gap_t = T0 + 30 * SEC;
    rig.sink.command(HistoryMsg::SessionStart {
        epoch: 2,
        t_ns: gap_t,
    });
    // Past the close of the block that starts after the gap.
    rig.sweeps(gap_t, 2 * 60 + 5, &records);

    let flags = rig.inspect(|mode| {
        let Mode::Rolling(window) = mode else {
            return Vec::new();
        };
        let (blocks, _) = window.blocks(0, u64::MAX).expect("blocks decode");
        blocks
            .iter()
            .map(|b| (b.start_t_ns, b.gap_before))
            .collect::<Vec<_>>()
    });
    assert!(flags.len() >= 3, "blocks {flags:?}");
    assert_eq!(flags[0], (T0, false), "the first block has no gap");
    assert_eq!(flags[1], (gap_t, true), "the block after the gap");
    assert!(
        flags[2..].iter().all(|&(_, gap)| !gap),
        "later blocks have no gap: {flags:?}"
    );
    rig.finish(&handle);
}

#[test]
fn dropped_events_reported_once_per_minute() {
    let mut rig = Rig::new(false);
    let handle = rig.spawn_worker();
    let records = sweep(2);

    rig.overflow(&records);
    assert_eq!(rig.sink.dropped(), 88);
    assert_eq!(rig.health_texts(), ["History dropped 88 events"]);

    // More drops inside the same minute wait for the next report.
    rig.overflow(&records);
    assert_eq!(rig.sink.dropped(), 176);
    assert!(
        rig.health_texts().is_empty(),
        "at most one report per minute"
    );

    rig.advance(Duration::from_secs(60));
    assert_eq!(rig.health_texts(), ["History dropped 176 events"]);
    rig.finish(&handle);
}
