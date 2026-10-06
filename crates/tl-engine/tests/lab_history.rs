//! LabRuntime tests for the `HistoryRecorder` worker in rolling-window and recording mode
//! (ADR-012, ADR-013). Store files go under a per-test temp directory.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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
use tl_history::format::{FORMAT_VERSION, SegmentHeader, boot_dir_name};
use tl_history::segment::SegmentReader;
use tl_model::command::HistoryWhat;
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::history::{
    HistoryLoadProblem, HistoryMetric, HistoryRange, HistorySource, HistoryStoreState, HistoryView,
    RecordSink,
};
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
        load_delay: None,
    }
}

/// A fresh store root for one test, removed on drop.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("tl-lab-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        TempRoot(dir)
    }

    fn boot_dir(&self) -> PathBuf {
        self.0.join(boot_dir_name(BOOT))
    }

    /// `seg-*.tlh` file names of this boot, sorted.
    fn segments(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.boot_dir())
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("seg-") && n.ends_with(".tlh"))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn reader(&self, name: &str) -> (PathBuf, SegmentReader) {
        let path = self.boot_dir().join(name);
        let reader = SegmentReader::open(&path).expect("segment opens");
        (path, reader)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn recording(edit: impl FnOnce(&mut EngineSettings)) -> EngineSettings {
    let mut settings = EngineSettings::default();
    settings.history.enabled = true;
    edit(&mut settings);
    settings
}

fn segment_secs(name: &str) -> u64 {
    name["seg-".len().."seg-".len() + 10]
        .parse()
        .expect("segment name has 10 digits")
}

fn assert_dir_only(dir: &Path) {
    assert!(dir.is_dir());
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
    settings_tx: watch::Sender<EngineSettings>,
    _control_tx: watch::Sender<WorkerControl>,
    io: WorkerIo,
    slot: HistorySlot,
    sink: HistorySink,
    next_id: u64,
    history_dir: Option<PathBuf>,
    load_delay: Option<Duration>,
}

impl Rig {
    fn new(process_table: bool) -> Self {
        Self::with_settings(process_table, EngineSettings::default())
    }

    /// Recording on, with the store under `dir`.
    fn recording(dir: Option<&Path>, settings: EngineSettings) -> Self {
        let mut rig = Self::with_settings(false, settings);
        rig.history_dir = dir.map(Path::to_path_buf);
        rig
    }

    fn with_settings(process_table: bool, settings: EngineSettings) -> Self {
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
        let (settings_tx, settings) = watch::channel(settings);
        let (control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(updates, demand, settings, control, SharedHandles::default());
        let slot: HistorySlot = Arc::new(ArcSwapOption::empty());
        let sink = HistorySink::new(Arc::clone(&slot));
        Rig {
            lab,
            rx,
            demand_tx,
            settings_tx,
            _control_tx: control_tx,
            io,
            slot,
            sink,
            next_id: 0,
            history_dir: None,
            load_delay: None,
        }
    }

    fn config(&self) -> HistoryWorkerConfig {
        HistoryWorkerConfig {
            history_dir: self.history_dir.clone(),
            load_delay: self.load_delay,
            ..config()
        }
    }

    /// Runs one worker generation as a root task.
    fn spawn_worker(&mut self) -> TaskHandle<Outcome<(), WorkerError>> {
        let io = self.io.clone();
        let slot = Arc::clone(&self.slot);
        let cfg = self.config();
        let root = root(&mut self.lab);
        let (task, handle) = self
            .lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                run_history(cx, io, slot, cfg).await
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

/// Starts the worker and lets its first turn enter Recording.
fn start_recording(rig: &mut Rig) -> TaskHandle<Outcome<(), WorkerError>> {
    let handle = rig.spawn_worker();
    rig.advance(Duration::from_secs(1));
    handle
}

#[test]
fn recording_writes_segments() {
    let root = TempRoot::new("writes");
    let mut rig = Rig::recording(Some(&root.0), recording(|_| {}));
    let handle = start_recording(&mut rig);
    rig.sweeps(T0, 3 * 60, &sweep(10));

    let view = rig.query_live();
    assert_eq!(view.source, HistorySource::Store);
    assert_eq!(view.store_state, HistoryStoreState::Ready);
    rig.finish(&handle);

    assert_eq!(root.segments(), ["seg-0000001000.tlh"]);
    let (path, reader) = root.reader("seg-0000001000.tlh");
    assert!(!reader.recovered, "the footer was written at shutdown");
    let starts: Vec<u64> = reader.index.iter().map(|e| e.start_t_ns).collect();
    assert_eq!(starts, [T0, T0 + MIN, T0 + 2 * MIN]);
    let mode = |p: &Path| std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&root.boot_dir()), 0o700);
}

#[test]
fn version_mismatch_reported_and_recording_continues() {
    let root = TempRoot::new("version");
    std::fs::create_dir_all(root.boot_dir()).expect("boot dir");
    let mut header = SegmentHeader {
        boot_id: BOOT,
        realtime_offset_ns: 0,
        start_t_ns: 500 * SEC,
    }
    .encode();
    header[4..6].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    std::fs::write(root.boot_dir().join("seg-0000000500.tlh"), header).expect("old segment");

    let mut rig = Rig::recording(Some(&root.0), recording(|_| {}));
    let handle = start_recording(&mut rig);
    rig.sweeps(T0, 2 * 60 + 5, &sweep(4));
    assert_eq!(
        rig.query_live().store_state,
        HistoryStoreState::Problem(HistoryLoadProblem::VersionMismatch)
    );
    rig.finish(&handle);

    assert_eq!(
        root.segments(),
        ["seg-0000000500.tlh", "seg-0000001000.tlh"]
    );
    let (_, reader) = root.reader("seg-0000001000.tlh");
    assert_eq!(
        reader.index.len(),
        3,
        "two closed blocks and the flushed one"
    );
}

#[test]
fn load_timeout_starts_fresh() {
    let root = TempRoot::new("timeout");
    let mut rig = Rig::recording(Some(&root.0), recording(|_| {}));
    rig.load_delay = Some(Duration::from_secs(3));
    let handle = start_recording(&mut rig);
    // The worker is waiting on the load. The timeout fires 2.5 s after it started.
    rig.advance(Duration::from_secs(2));
    rig.sweeps(T0, 65, &sweep(4));

    let view = rig.query_live();
    assert_eq!(
        view.store_state,
        HistoryStoreState::Problem(HistoryLoadProblem::TimedOut)
    );
    assert_eq!(view.source, HistorySource::Store);
    let has_store = rig.inspect(|mode| matches!(mode, Mode::Recording(rec) if rec.has_store()));
    assert!(has_store, "recording goes on into a fresh store");
    rig.finish(&handle);

    assert_eq!(root.segments(), ["seg-0000001000.tlh"]);
    let (_, reader) = root.reader("seg-0000001000.tlh");
    assert_eq!(reader.index.len(), 2);
}

#[test]
fn persist_false_purges_at_shutdown() {
    let root = TempRoot::new("purge");
    let settings = recording(|s| s.history.persist = false);
    let mut rig = Rig::recording(Some(&root.0), settings);
    let handle = start_recording(&mut rig);
    rig.sweeps(T0, 70, &sweep(4));
    assert_eq!(
        root.segments(),
        ["seg-0000001000.tlh"],
        "the closed block was written"
    );
    rig.finish(&handle);

    assert_dir_only(&root.boot_dir());
    assert!(root.segments().is_empty(), "{:?}", root.segments());
}

#[test]
fn disk_cap_change_applies() {
    let root = TempRoot::new("cap");
    let mut rig = Rig::recording(Some(&root.0), recording(|_| {}));
    let handle = start_recording(&mut rig);
    let records = sweep(6);
    rig.sweeps(T0, 21 * 60, &records);
    assert_eq!(
        root.segments(),
        ["seg-0000001000.tlh", "seg-0000001600.tlh"]
    );

    // A cap of 0 MiB leaves only the open segment.
    rig.settings_tx
        .send(recording(|s| s.history.max_disk_mb = 0))
        .expect("settings watch open");
    rig.sweeps(T0 + 21 * MIN, 61, &records);
    assert_eq!(root.segments(), ["seg-0000002200.tlh"]);
    rig.finish(&handle);
}

#[test]
fn no_dir_no_files() {
    let mut rig = Rig::recording(None, recording(|_| {}));
    let handle = start_recording(&mut rig);
    rig.sweeps(T0, 65, &sweep(4));

    let view = rig.query_live();
    assert_eq!(view.source, HistorySource::Store);
    assert_eq!(
        view.store_state,
        HistoryStoreState::Problem(HistoryLoadProblem::OpenFailed)
    );
    let has_store = rig.inspect(|mode| matches!(mode, Mode::Recording(rec) if rec.has_store()));
    assert!(!has_store, "RAM only");
    // No retry without a directory.
    rig.advance(Duration::from_secs(61));
    let state = rig.inspect(|mode| match mode {
        Mode::Recording(rec) => Some((rec.has_store(), rec.store_state())),
        _ => None,
    });
    assert_eq!(
        state,
        Some((
            false,
            HistoryStoreState::Problem(HistoryLoadProblem::OpenFailed)
        ))
    );
    rig.finish(&handle);
}

#[test]
fn gap_across_segment_boundary_resets_meta_ids() {
    let root = TempRoot::new("gap");
    let mut rig = Rig::recording(Some(&root.0), recording(|_| {}));
    let handle = start_recording(&mut rig);
    let records = sweep(5);
    // Blocks at 1_000 .. 1_480 s; the block open at 1_480 s would be followed by 1_540 s.
    rig.sweeps(T0, 510, &records);
    rig.sink.command(HistoryMsg::SessionStart {
        epoch: 2,
        t_ns: T0 + 510 * SEC,
    });
    // After the gap the next block starts at 1_650 s, in a new segment.
    rig.sweeps(T0 + 650 * SEC, 65, &records);
    rig.finish(&handle);

    let names = root.segments();
    assert_eq!(names, ["seg-0000001000.tlh", "seg-0000001650.tlh"]);
    assert_eq!(segment_secs(&names[1]), 1_650);
    let (path, reader) = root.reader(&names[1]);
    let first = reader
        .read_block(&path, &reader.index[0])
        .expect("first block decodes");
    assert!(first.gap_before);
    let ids: Vec<u32> = first.new_meta.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, [0, 1, 2, 3, 4], "the new segment's ids start at 0");
    assert!(
        first.keyframe.iter().all(|p| ids.contains(&p.meta)),
        "every keyframe meta is listed in new_meta"
    );
}

// End to end through the engine session (E30-A2) ---------------------------------------------

#[path = "fake_sources.rs"]
mod fake_sources;

/// `engine_root` with the real history worker, a scripted Processes source and a lab clock.
/// The session tests in `lab_session.rs` use the same rig shape; `Engine::start` cannot move a
/// virtual clock or panic the reducer on cue.
mod engine {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use asupersync::Cx;
    use asupersync::runtime::TaskHandle;
    use asupersync::time::TimeSource;
    use smallvec::SmallVec;
    use tl_engine::bridge::{EngineHooks, HookCtx};
    use tl_engine::config::{EngineConfig, EngineMode};
    use tl_engine::reducer::{ActionGate, AdmissionView};
    use tl_engine::session::{RESTART_DELAY, engine_root};
    use tl_engine::update::{EngineMsg, ProcessSweep, SourceUpdate};
    use tl_history::format::boot_dir_name;
    use tl_history::segment::SegmentReader;
    use tl_model::actions::{ActionId, ActionKind, ActionRequest, SchedPolicy, Signal};
    use tl_model::command::{Command, HistoryWhat};
    use tl_model::demand::Demand;
    use tl_model::health::SourceId;
    use tl_model::history::{HistoryMetric, HistoryRange, HistorySource, HistoryView};
    use tl_model::ids::ProcessKey;
    use tl_model::process::{
        GroupKind, ProcFlags, ProcState, ProcessExtras, ProcessMeta, ProcessRecord,
    };
    use tl_model::settings::EngineSettings;
    use tl_model::snapshot::Snapshot;
    use tl_model::system::CpuSet;
    use tl_model::validity::{FieldValidity, Validity};

    use super::TempRoot;
    use super::fake_sources::{self, FAKE_BOOT, FakeLab, UiSide};

    /// The reducer panics on any `Command::Action`, which ends the session (`lab_session.rs`).
    struct PanicGate;

    impl ActionGate for PanicGate {
        fn submit(&mut self, _cx: &Cx, _req: ActionRequest, _view: &AdmissionView<'_>) {
            panic!("scripted reducer panic");
        }

        fn finished(&mut self, _id: ActionId) -> Option<SourceId> {
            None
        }
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid) * 10,
            boot_id: FAKE_BOOT,
            pid,
        }
    }

    fn record(pid: u32) -> ProcessRecord {
        ProcessRecord {
            key: key(pid),
            ppid: 0,
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
                search_lower: "x user".into(),
            }),
            group: GroupKind::Background.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 5.0,
            rss_anon_bytes: 1 << 20,
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
        }
    }

    fn extras() -> ProcessExtras {
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

    /// What the Processes worker sends: PIDs 1 to `n`.
    fn sweep_msg(t_ns: u64, n: u32) -> EngineMsg {
        EngineMsg::Source(SourceUpdate::Processes(ProcessSweep {
            observed_start: t_ns,
            observed_end: t_ns,
            records: (1..=n).map(record).collect(),
            extras: (1..=n).map(|_| extras()).collect(),
            extras_p2: Vec::new(),
            app_stats: Vec::new(),
            unit_stats: Vec::new(),
            system_threads: 0,
            file_nr: None,
            online_cpus: 1,
            low_memory: None,
        }))
    }

    fn frame_query(id: u64) -> EngineMsg {
        EngineMsg::Command(Command::HistoryQuery {
            id,
            range: HistoryRange::Live,
            what: HistoryWhat::Frame {
                metric: HistoryMetric::Cpu,
                view: (0, u64::MAX),
                keys: Arc::from([]),
                overview: false,
            },
        })
    }

    struct EngineRig {
        fake: FakeLab,
        root: Option<TaskHandle<()>>,
        ui: UiSide,
    }

    impl EngineRig {
        /// Starts `engine_root`. `history_dir` is the store root; settings and demand are set
        /// before the first session.
        fn start(history_dir: Option<&Path>, settings: EngineSettings, demand: Demand) -> Self {
            let mut config = EngineConfig::new(
                EngineMode::Full,
                Path::new(env!("CARGO_TARGET_TMPDIR")).join("tl-lab-history-state"),
            );
            if let Some(dir) = history_dir {
                config = config.with_history_dir(dir.to_path_buf());
            }
            let (mut ep, ui) = fake_sources::root_endpoints(Arc::new(config), Vec::new());
            ep.hooks = EngineHooks {
                action_gate: Some(Arc::new(|_ctx: HookCtx| {
                    Box::new(PanicGate) as Box<dyn ActionGate>
                })),
                ..EngineHooks::default()
            };
            ui.settings_tx.send(settings).expect("settings watch open");
            ui.demand_tx.send(demand).expect("demand watch open");
            let mut fake = FakeLab::new();
            let root = fake.spawn(async move {
                let cx = Cx::current().expect("task has a Cx");
                engine_root(cx, ep).await;
            });
            // The history worker's first turn picks its mode.
            fake.advance(Duration::from_secs(1));
            EngineRig {
                fake,
                root: Some(root),
                ui,
            }
        }

        fn send(&mut self, msg: EngineMsg) {
            self.ui
                .msgs_tx
                .try_send(msg)
                .expect("the queue has room for the message");
            self.fake.lab.run_until_idle();
        }

        /// One sweep of `n` processes per second for `secs` seconds.
        fn sweeps(&mut self, secs: u64, n: u32) {
            for _ in 0..secs {
                let t = self.fake.clock.now().as_nanos();
                self.send(sweep_msg(t, n));
                self.fake.advance(Duration::from_secs(1));
            }
        }

        fn latest(&self) -> Arc<Snapshot> {
            self.ui.latest.load_full()
        }

        fn history(&self) -> Arc<HistoryView> {
            Arc::clone(&self.latest().history)
        }

        fn history_error(&self) -> Option<Arc<str>> {
            self.latest().health.sources[SourceId::HistoryRecorder.index()]
                .last_error
                .clone()
        }

        /// Cancels the root, which closes the store, then checks the lab's oracles.
        fn finish(mut self) {
            let root = self.root.take().expect("root started");
            root.abort();
            self.fake.advance(Duration::from_millis(10));
            assert!(root.is_finished(), "engine_root returned after cancel");
            self.fake.finish();
        }
    }

    fn rolling() -> Demand {
        Demand {
            process_table: true,
            ..Demand::default()
        }
    }

    #[test]
    fn query_answer_reaches_snapshot() {
        let mut rig = EngineRig::start(None, EngineSettings::default(), rolling());
        let before = rig.history();
        assert_eq!(before.header.validity, Validity::WarmingUp);
        rig.sweeps(3, 5);
        rig.send(frame_query(7));

        let view = rig.history();
        assert_eq!(view.query_id, 7);
        assert_eq!(view.source, HistorySource::RollingWindow);
        assert_eq!(view.header.validity, Validity::Valid);
        assert!(view.span.is_some(), "the window holds the sweeps");
        assert_eq!(rig.history_error(), None, "nothing was dropped");
        rig.finish();
    }

    #[test]
    fn history_component_version_bumps() {
        let mut rig = EngineRig::start(None, EngineSettings::default(), rolling());
        rig.sweeps(2, 3);
        rig.send(frame_query(1));
        let first = rig.history();
        assert_eq!(first.query_id, 1);

        rig.send(frame_query(2));
        let second = rig.history();
        assert_eq!(second.query_id, 2);
        assert!(!Arc::ptr_eq(&first, &second), "a new Arc per answer");
        assert!(
            second.header.version > first.header.version,
            "version {} after {}",
            second.header.version,
            first.header.version
        );
        rig.finish();
    }

    #[test]
    fn session_restart_sends_gap() {
        let root = TempRoot::new("engine-restart");
        let mut settings = EngineSettings::default();
        settings.history.enabled = true;
        let mut rig = EngineRig::start(Some(&root.0), settings, Demand::default());
        rig.sweeps(3, 4);
        assert_eq!(rig.latest().epoch, 1);

        rig.send(EngineMsg::Command(Command::Action(ActionRequest {
            id: ActionId(1),
            kind: ActionKind::Signal(Signal::Term),
            targets: SmallVec::new(),
            privileged: false,
        })));
        rig.fake.advance(RESTART_DELAY + Duration::from_secs(1));
        rig.sweeps(3, 4);
        assert_eq!(rig.latest().epoch, 2, "the session restarted");
        rig.finish();

        // Both generations closed their store; collect every block in time order.
        let dir = root.0.join(boot_dir_name(FAKE_BOOT));
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .expect("boot dir")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "tlh"))
            .collect();
        names.sort();
        let mut blocks: Vec<(u64, bool)> = Vec::new();
        for path in &names {
            let reader = SegmentReader::open(path).expect("segment opens");
            for entry in &reader.index {
                let block = reader.read_block(path, entry).expect("block decodes");
                blocks.push((block.start_t_ns, block.gap_before));
            }
        }
        blocks.sort_unstable();
        assert_eq!(blocks.len(), 2, "one block per session: {blocks:?}");
        // Each session starts with SessionStart. Epoch 1 follows an earlier run of the app in the
        // same boot; epoch 2 follows the panic.
        assert!(blocks[1].1, "the block after the restart has gap_before");
        assert!(blocks[0].1, "the first session's block has gap_before");
    }

    #[test]
    fn idle_when_off_and_tab_closed() {
        let mut rig = EngineRig::start(None, EngineSettings::default(), Demand::default());
        rig.sweeps(60, 10);
        assert_eq!(rig.history().header.version, 0, "no view was sent");
        assert_eq!(rig.history_error(), None, "the sink dropped nothing");

        rig.send(frame_query(3));
        let view = rig.history();
        assert_eq!(view.query_id, 3);
        assert_eq!(
            view.source,
            HistorySource::None,
            "the worker holds no window"
        );
        assert_eq!(view.span, None);
        assert_eq!(view.process_count, 0);
        rig.finish();
    }

    #[test]
    fn delete_reaches_snapshot() {
        let mut rig = EngineRig::start(None, EngineSettings::default(), rolling());
        rig.sweeps(3, 5);
        rig.send(frame_query(4));
        let keys = |v: &HistoryView| v.rows.iter().map(|r| r.key).collect::<Vec<_>>();
        let before = rig.history();
        assert!(keys(&before).contains(&key(3)), "rows {:?}", keys(&before));

        rig.send(EngineMsg::Command(Command::HistoryDelete {
            keys: Arc::from([key(3)]),
        }));
        let after = rig.history();
        assert!(after.header.version > before.header.version);
        assert!(!keys(&after).contains(&key(3)), "rows {:?}", keys(&after));
        assert!(keys(&after).contains(&key(2)));
        rig.finish();
    }
}
