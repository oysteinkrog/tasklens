//! LabRuntime tests for the `Processes` worker on fixture roots (ADR-013, ADR-008, ADR-016).
//!
//! Each test copies `owner-desktop` with `process-edge` laid over it into the target tmpdir, so
//! it can add pid directories and rewrite files between sweeps. `source::run` drives a real
//! `ProcessesSource` in a `FakeLab`; the sampler reads the lab's virtual clock.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use asupersync::Cx;
use asupersync::channel::{mpsc, watch};
use asupersync::runtime::TaskHandle;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::{TimeSource, VirtualClock};
use fake_sources::{FakeLab, channels, drain, io};
use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::session::engine_root;
use tl_engine::source::{self, WorkerControl, WorkerIo};
use tl_engine::supervise::WorkerFactory;
use tl_engine::update::{EngineMsg, ProcessSweep, SourceUpdate};
use tl_engine::workers::memory;
use tl_engine::workers::metric_history::State;
use tl_engine::workers::processes::{self, ProcessesSource};
use tl_engine::workers::{WorkerSpec, workers_for};
use tl_history::metrics::{MetricConfig, MetricHistory, Tier};
use tl_model::demand::Demand;
use tl_model::devices::DeviceCatalog;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{ColumnId, MetricId, ProcessKey, SeriesId};
use tl_model::process::{GroupRowKind, ProcessRecord};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::time::TimeNs;
use tl_model::validity::{ProcessField, Validity};

/// The process-edge process the metadata tests change: no exe link, uid 1000.
const TOKENPROC: u32 = 4209;

fn fixtures() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tl-collect/tests/fixtures/"
    ))
}

/// Copies every entry of `src` over `dst`, replacing files and links that exist in both.
fn overlay(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create fixture dir");
    for entry in fs::read_dir(src).expect("list fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type().expect("fixture file type");
        if file_type.is_symlink() {
            let target = fs::read_link(entry.path()).expect("read fixture symlink");
            let _ = fs::remove_file(&to);
            std::os::unix::fs::symlink(target, &to).expect("create fixture symlink");
        } else if file_type.is_dir() {
            overlay(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).expect("copy fixture file");
        }
    }
}

/// A writable copy of one or more tl-collect fixtures under the target tmpdir. Later names win
/// for files in both. The directory name holds the process id and a counter, so parallel test
/// runs never share a copy. Dropping the root removes the copy.
struct FixtureRoot {
    dir: PathBuf,
}

impl FixtureRoot {
    fn new(names: &[&str]) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("lab-processes-{}-{n}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("remove old fixture copy");
        }
        for name in names {
            overlay(&fixtures().join(name), &dir.join("root"));
        }
        FixtureRoot { dir }
    }

    /// `owner-desktop` with `process-edge` over it, as the tl-collect sampler tests use.
    fn merged() -> Self {
        Self::new(&["owner-desktop", "process-edge"])
    }

    fn root(&self) -> PathBuf {
        self.dir.join("root")
    }

    fn pid_dir(&self, pid: u32) -> PathBuf {
        self.root().join(format!("proc/{pid}"))
    }

    fn write(&self, pid: u32, file: &str, bytes: impl AsRef<[u8]>) {
        fs::write(self.pid_dir(pid).join(file), bytes).expect("write fixture file");
    }

    fn read(&self, pid: u32, file: &str) -> String {
        fs::read_to_string(self.pid_dir(pid).join(file)).expect("read fixture file")
    }

    /// Copies pid `from` to a new pid directory `to`, with the pid in `stat` rewritten.
    fn clone_pid(&self, from: u32, to: u32) {
        overlay(&self.pid_dir(from), &self.pid_dir(to));
        let stat = self.read(from, "stat");
        let rest = stat
            .strip_prefix(&format!("{from} "))
            .expect("stat starts with the pid");
        self.write(to, "stat", format!("{to} {rest}"));
    }

    /// Changes both the `comm` file and the stat `comm` field, as a real rename does.
    fn set_comm(&self, pid: u32, comm: &str) {
        let stat = self.read(pid, "stat");
        let open = stat.find('(').expect("stat has an open paren");
        let close = stat.rfind(')').expect("stat has a close paren");
        let new = format!("{}({comm}){}", &stat[..open], &stat[close + 1..]);
        self.write(pid, "stat", new);
        self.write(pid, "comm", format!("{comm}\n"));
    }

    /// The fixture root, with app lookups on the tl-collect app fixtures and the icon cache in
    /// this copy, so no test writes the real `~/.cache`.
    fn config(&self) -> Arc<EngineConfig> {
        let share = |name: &str| fixtures().join(name).join("usr/share");
        let dirs = [share("apps-desktop"), share("apps-icons")];
        let paths = AppPaths {
            home: fixtures().join("apps-icons/home"),
            xdg_data_home: None,
            xdg_data_dirs: Some(std::env::join_paths(dirs).expect("join data dirs")),
            xdg_config_home: None,
            icon_cache: self.dir.join("cache/icons.toml"),
        };
        Arc::new(
            EngineConfig::new(EngineMode::Full, self.dir.join("state"))
                .with_sys_root(SysRoot::at(&self.root()))
                .with_app_paths(paths),
        )
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        // Best effort: a failed cleanup must not hide the test result.
        let _ = fs::remove_dir_all(&self.dir);
    }
}

thread_local! {
    /// The clock of the current test's lab. The lab runs every task on the test thread.
    static CLOCK: RefCell<Option<Arc<VirtualClock>>> = const { RefCell::new(None) };
}

/// The sampler clock: lab virtual time for both CLOCK_BOOTTIME and CLOCK_MONOTONIC.
fn lab_clock() -> (TimeNs, u64) {
    let t = CLOCK.with(|c| {
        c.borrow()
            .as_ref()
            .expect("the rig set the clock")
            .now()
            .as_nanos()
    });
    (t, t)
}

/// A `ProcessesSource` running under `source::run` in a `FakeLab`, and the receive time in ms of
/// every sweep it sent. With demand on from the start, the first sweep runs at 0 ms.
struct Rig {
    fl: FakeLab,
    rx: mpsc::Receiver<EngineMsg>,
    demand: watch::Sender<Demand>,
    settings: watch::Sender<EngineSettings>,
    // Kept so the worker's watch receivers stay open.
    _updates: mpsc::Sender<EngineMsg>,
    _control: watch::Sender<WorkerControl>,
    handle: TaskHandle<()>,
    sweeps: Vec<(u64, ProcessSweep)>,
    /// Sweeps `next_sweep` has returned.
    taken: usize,
}

impl Rig {
    fn start(root: &FixtureRoot, demand: Demand, interval_ms: u32) -> Self {
        let settings = EngineSettings {
            interval_ms,
            ..EngineSettings::default()
        };
        Self::start_with(root, demand, settings)
    }

    fn start_with(root: &FixtureRoot, demand: Demand, settings: EngineSettings) -> Self {
        let mut fl = FakeLab::new();
        CLOCK.with(|c| *c.borrow_mut() = Some(Arc::clone(&fl.clock)));
        let (updates, rx, demand_tx, settings_tx, control) = channels();
        demand_tx.send(demand).expect("watch has receivers");
        settings_tx.send(settings).expect("watch has receivers");
        let worker_io = io(&updates, &demand_tx, &settings_tx, &control);
        let src = ProcessesSource::new(root.config())
            .expect("fixture source opens")
            .with_clock(lab_clock);
        let handle = fl.spawn(async move {
            let cx = Cx::current().expect("task has a Cx");
            let _ = source::run(cx, worker_io, src).await;
        });
        let mut rig = Rig {
            fl,
            rx,
            demand: demand_tx,
            settings: settings_tx,
            _updates: updates,
            _control: control,
            handle,
            sweeps: Vec::new(),
            taken: 0,
        };
        rig.collect();
        rig
    }

    fn collect(&mut self) {
        for msg in drain(&mut self.rx) {
            match msg {
                EngineMsg::Source(SourceUpdate::Processes(sweep)) => {
                    self.sweeps.push((self.fl.now_ms(), sweep));
                }
                EngineMsg::Source(other) => panic!("unexpected update {other:?}"),
                _ => {}
            }
        }
    }

    /// Runs for `total` in `step` increments, collecting every sweep.
    fn run(&mut self, total: Duration, step: Duration) {
        let end = self.fl.now_ms() + total.as_millis() as u64;
        while self.fl.now_ms() < end {
            self.fl.advance(step);
            self.collect();
        }
    }

    /// The oldest sweep not yet returned, running the lab until one arrives.
    fn next_sweep(&mut self) -> &ProcessSweep {
        for _ in 0..1_000 {
            if self.taken < self.sweeps.len() {
                self.taken += 1;
                return &self.sweeps[self.taken - 1].1;
            }
            self.run(Duration::from_millis(10), Duration::from_millis(10));
        }
        panic!("no sweep within 10 s");
    }

    /// Receive times of the sweeps before `end_ms`.
    fn times_before(&self, end_ms: u64) -> Vec<u64> {
        self.sweeps
            .iter()
            .map(|(t, _)| *t)
            .filter(|t| *t < end_ms)
            .collect()
    }

    fn times(&self) -> Vec<u64> {
        self.times_before(u64::MAX)
    }

    fn finish(self) {
        self.handle.abort();
        drop((self.demand, self.settings));
        self.fl.finish();
    }
}

fn table_demand() -> Demand {
    Demand {
        any_surface_visible: true,
        process_table: true,
        ..Demand::default()
    }
}

fn keys(sweep: &ProcessSweep) -> Vec<ProcessKey> {
    sweep.records.iter().map(|r| r.key).collect()
}

fn record(sweep: &ProcessSweep, pid: u32) -> &ProcessRecord {
    sweep
        .records
        .iter()
        .find(|r| r.key.pid == pid)
        .unwrap_or_else(|| panic!("pid {pid} not swept"))
}

#[test]
fn rate_follows_interval() {
    let root = FixtureRoot::merged();
    let mut rig = Rig::start(&root, table_demand(), 1000);
    rig.run(Duration::from_secs(5), Duration::from_millis(10));
    let times = rig.times_before(5_000);
    assert_eq!(times, [0, 1_000, 2_000, 3_000, 4_000]);
    assert!(rig.sweeps.iter().all(|(_, s)| !s.records.is_empty()));
    rig.finish();
}

#[test]
fn hidden_zero_sweeps_60s() {
    let root = FixtureRoot::merged();
    let hidden = Demand {
        any_surface_visible: false,
        process_table: false,
        recording: false,
        ..Demand::default()
    };
    let mut rig = Rig::start(&root, hidden, 1000);
    rig.run(Duration::from_secs(60), Duration::from_millis(100));
    assert!(
        rig.sweeps.is_empty(),
        "swept while hidden: {:?}",
        rig.times()
    );
    rig.finish();
}

/// Device history alone: the memory group sweep with no surface shown (D-036, UI-PRF-060).
fn history_demand() -> Demand {
    Demand {
        device_history: true,
        ..Demand::default()
    }
}

/// tasklens-8fx.45: with only `history.device_metrics` on, the sweep runs every 10 s, also with
/// no window.
#[test]
fn history_only_sweeps_every_10s() {
    let root = FixtureRoot::merged();
    let mut rig = Rig::start(&root, history_demand(), 1000);
    rig.run(Duration::from_secs(60), Duration::from_millis(100));
    let times = rig.times_before(60_000);
    assert_eq!(times, [0, 10_000, 20_000, 30_000, 40_000, 50_000]);
    assert!(rig.sweeps.iter().all(|(_, s)| !s.records.is_empty()));
    rig.finish();
}

/// With `history.device_metrics` off, device history demand alone sweeps nothing.
#[test]
fn history_off_no_sweeps() {
    let root = FixtureRoot::merged();
    let mut settings = EngineSettings {
        interval_ms: 1000,
        ..EngineSettings::default()
    };
    settings.history.device_metrics = false;
    let mut rig = Rig::start_with(&root, history_demand(), settings);
    rig.run(Duration::from_secs(60), Duration::from_millis(100));
    assert!(
        rig.sweeps.is_empty(),
        "swept with history off: {:?}",
        rig.times()
    );
    rig.finish();
}

/// Opening the Processes tab ends the 10 s wait at once, then the sweep follows the interval.
#[test]
fn table_demand_ends_history_wait() {
    let root = FixtureRoot::merged();
    let mut rig = Rig::start(&root, history_demand(), 1000);
    rig.run(Duration::from_secs(25), Duration::from_millis(100));
    assert_eq!(rig.times(), [0, 10_000, 20_000]);

    rig.demand
        .send(Demand {
            device_history: true,
            ..table_demand()
        })
        .expect("watch has receivers");
    rig.run(Duration::from_secs(3), Duration::from_millis(10));
    let after: Vec<u64> = rig
        .times_before(28_000)
        .into_iter()
        .filter(|t| *t >= 25_000)
        .collect();
    assert_eq!(after.len(), 3, "{after:?}");
    assert!(after[0] <= 25_010, "first table sweep at {} ms", after[0]);
    assert_eq!(after[1..], [after[0] + 1_000, after[0] + 2_000]);
    rig.finish();
}

#[test]
fn recording_keeps_sweeps() {
    let root = FixtureRoot::merged();
    let recording = Demand {
        any_surface_visible: false,
        process_table: false,
        recording: true,
        ..Demand::default()
    };
    let mut rig = Rig::start(&root, recording, 1000);
    rig.run(Duration::from_secs(10), Duration::from_millis(10));
    let times = rig.times_before(10_000);
    assert_eq!(times.len(), 10, "{times:?}");
    assert!(times.windows(2).all(|w| w[1] - w[0] == 1_000), "{times:?}");
    rig.finish();
}

#[test]
fn new_pid_appears() {
    let root = FixtureRoot::merged();
    let mut rig = Rig::start(&root, table_demand(), 1000);
    let before = keys(rig.next_sweep());
    assert!(before.iter().all(|k| k.pid != 4299));

    root.clone_pid(TOKENPROC, 4299);
    let after = keys(rig.next_sweep());
    let added: Vec<&ProcessKey> = after.iter().filter(|k| !before.contains(k)).collect();
    assert_eq!(added.len(), 1, "{added:?}");
    assert_eq!(added[0].pid, 4299);
    let kept: Vec<ProcessKey> = after.iter().copied().filter(|k| k.pid != 4299).collect();
    assert_eq!(kept, before, "the previous keys changed");
    rig.finish();
}

#[test]
fn comm_change_refreshes_metadata_at_once() {
    let root = FixtureRoot::merged();
    let mut rig = Rig::start(&root, table_demand(), 1000);
    let old = Arc::clone(&record(rig.next_sweep(), TOKENPROC).meta);
    assert_eq!(&*old.comm, "tokenproc");
    // A few sweeps so the refresh counter is mid-way.
    for _ in 0..3 {
        rig.next_sweep();
    }

    root.set_comm(TOKENPROC, "renamed");
    let new = Arc::clone(&record(rig.next_sweep(), TOKENPROC).meta);
    assert_eq!(&*new.comm, "renamed");
    assert_ne!(new.name, old.name);
    assert!(new.name.contains("renamed"), "name {:?}", new.name);
    rig.finish();
}

#[test]
fn cmdline_change_waits_for_refresh_sweep() {
    let root = FixtureRoot::merged();
    let mut rig = Rig::start(&root, table_demand(), 1000);
    // The first sweep reads the metadata. The first refresh is staggered by pid: it comes
    // 10 - (pid % 10) sweeps later, then every 10 sweeps, so no read is older than 10 (R-064,
    // R-286).
    let old = record(rig.next_sweep(), TOKENPROC).meta.cmdline.clone();
    assert!(old.is_some());
    let first = 10 - TOKENPROC % 10;
    assert!((1..=10).contains(&first));

    // A change before the first refresh shows at that sweep and not before.
    root.write(TOKENPROC, "cmdline", b"first-cmd\0--x\0");
    for n in 1..first {
        let seen = record(rig.next_sweep(), TOKENPROC).meta.cmdline.clone();
        assert_eq!(seen, old, "sweep {n}: read before the staggered refresh");
    }
    let seen = record(rig.next_sweep(), TOKENPROC).meta.cmdline.clone();
    assert_eq!(seen.as_deref(), Some("first-cmd --x"), "sweep {first}");

    // After that the counter restarts: a change shows 10 sweeps later.
    root.write(TOKENPROC, "cmdline", b"changed-cmd\0--y\0");
    for n in 1..10 {
        let seen = record(rig.next_sweep(), TOKENPROC).meta.cmdline.clone();
        assert_eq!(
            seen.as_deref(),
            Some("first-cmd --x"),
            "sweep {n} after the refresh re-read the cmdline"
        );
    }
    let tenth = record(rig.next_sweep(), TOKENPROC).meta.cmdline.clone();
    assert_eq!(tenth.as_deref(), Some("changed-cmd --y"));
    rig.finish();
}

/// Security regression (ADR-016): another user's unreadable `io` stays Restricted with zero
/// values, and is never shown as Valid.
#[test]
fn restricted_io_stays_restricted() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let root = FixtureRoot::merged();
    // owner-desktop pid 1 runs as root and has no io file in the capture.
    let io = root.pid_dir(1).join("io");
    fs::copy(root.pid_dir(4201).join("io"), &io).expect("copy io");
    fs::set_permissions(&io, fs::Permissions::from_mode(0o000)).expect("chmod io");

    // `io` is read only while an I/O column is shown (R-286).
    let mut demand = table_demand();
    demand.process_columns.insert(ColumnId::IoReadRate);
    let mut rig = Rig::start(&root, demand, 1000);
    rig.run(Duration::from_secs(2), Duration::from_millis(100));
    assert_eq!(rig.times(), [0, 1_000, 2_000]);
    for (t, sweep) in &rig.sweeps {
        let r = record(sweep, 1);
        assert_eq!(r.uid, 0);
        for field in [ProcessField::IoRead, ProcessField::IoWrite] {
            assert_eq!(
                r.validity.get(field),
                Validity::Restricted,
                "{field:?} at {t} ms"
            );
        }
        assert_eq!(r.io_read_bps, 0.0);
        assert_eq!(r.io_write_bps, 0.0);
    }
    rig.finish();
}

/// Pid directories under the fixture's `proc`.
fn pid_count(root: &FixtureRoot) -> usize {
    fs::read_dir(root.root().join("proc"))
        .expect("list fixture proc")
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.parse::<u32>().is_ok())
        })
        .count()
}

/// E10-A1: `engine_root` with the real `Processes` spec publishes the sweep and the grouped
/// table while the table is demanded, and stops sweeping once it is not.
#[test]
fn engine_publishes_table_from_fixture() {
    let root = FixtureRoot::new(&["owner-desktop"]);
    // The capture has no cgroup tree; the sampler only needs the directory to open.
    fs::create_dir_all(root.root().join("sys/fs/cgroup")).expect("create cgroup dir");
    let mut fl = FakeLab::new();
    let (ep, ui) = fake_sources::root_endpoints(root.config(), vec![processes::spec()]);
    // Device history off: its background demand would keep the memory group sweep at 10 s
    // with no table demanded (ADR-018).
    let mut settings = EngineSettings::default();
    settings.history.device_metrics = false;
    ui.settings_tx.send(settings).expect("watch has receivers");
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        engine_root(cx, ep).await;
    });

    ui.demand_tx
        .send(table_demand())
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));

    let snap = ui.latest.load_full();
    assert_eq!(snap.processes.records.len(), pid_count(&root));
    let headers = snap
        .table
        .rows
        .iter()
        .filter(|row| row.record.is_none() && row.group.is_none())
        .count();
    assert!(!snap.table.rows.is_empty());
    assert_eq!(headers, 3, "{:?}", snap.table.rows);
    assert_eq!(
        snap.health.get(SourceId::Processes).state,
        SourceState::Running
    );

    ui.demand_tx
        .send(Demand::default())
        .expect("watch has receivers");
    // A sweep already running when the demand changed may still land.
    fl.run_for(Duration::from_secs(1), Duration::from_millis(100));
    let version = ui.latest.load_full().processes.header.version;
    fl.run_for(Duration::from_secs(60), Duration::from_millis(100));
    assert_eq!(ui.latest.load_full().processes.header.version, version);

    handle.abort();
    fl.advance(Duration::from_millis(10));
    assert!(handle.is_finished(), "engine_root returned after cancel");
    drop(ui);
    fl.finish();
}

/// The `MEM_GROUP` series of a band key in `snap`, if the band is published.
fn band_series(snap: &Snapshot, key: &str) -> Option<Arc<SeriesView>> {
    let band = snap.system.memory.groups.iter().find(|b| &*b.key == key)?;
    snap.series
        .get(SeriesId {
            device: band.device,
            metric: MetricId::MEM_GROUP,
        })
        .cloned()
}

/// Runs `total` in 100 ms steps and returns the lab time in ms of each new sample of the "Other
/// apps" band, checking at each one that the bands sum to In use.
fn other_band_times(fl: &mut FakeLab, ui: &fake_sources::UiSide, total: Duration) -> Vec<u64> {
    let len = |ui: &fake_sources::UiSide| {
        band_series(&ui.latest.load_full(), "other").map_or(0, |v| v.samples.len())
    };
    let mut seen = len(ui);
    let mut times = Vec::new();
    let end = fl.now_ms() + total.as_millis() as u64;
    while fl.now_ms() < end {
        fl.advance(Duration::from_millis(100));
        let n = len(ui);
        if n == seen {
            continue;
        }
        seen = n;
        times.push(fl.now_ms());
        if fl.now_ms() < 1_000 {
            // The first sweep can run before the first memory sample.
            continue;
        }
        let snap = ui.latest.load_full();
        assert_eq!(snap.system.memory.validity, Validity::Valid);
        let used = snap.system.memory.used;
        assert!(used > 0);
        let sum: f64 = snap
            .system
            .memory
            .groups
            .iter()
            .filter_map(|b| band_series(&snap, &b.key))
            .filter_map(|v| v.samples.last().copied())
            .filter(|s| s.is_valid())
            .map(|s| f64::from(s.value))
            .sum();
        let diff = (sum - used as f64).abs();
        assert!(
            diff <= used as f64 * 1e-5,
            "bands sum to {sum}, In use is {used}, at {} ms",
            fl.now_ms()
        );
    }
    times
}

/// tasklens-8fx.45: with only device history on and no surface, the memory group series get a
/// sample every 10 s from the engine's own sweep, Processes stays healthy, and the bands sum to
/// In use. Opening the table brings back the interval.
#[test]
fn mem_group_series_every_10s() {
    let root = FixtureRoot::new(&["owner-desktop"]);
    fs::create_dir_all(root.root().join("sys/fs/cgroup")).expect("create cgroup dir");
    let mut fl = FakeLab::new();
    let (ep, ui) =
        fake_sources::root_endpoints(root.config(), vec![processes::spec(), memory::spec()]);
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        engine_root(cx, ep).await;
    });

    ui.demand_tx
        .send(history_demand())
        .expect("watch has receivers");
    let times = other_band_times(&mut fl, &ui, Duration::from_secs(60));
    assert_eq!(times.len(), 6, "{times:?}");
    assert!(times.windows(2).all(|w| w[1] - w[0] == 10_000), "{times:?}");
    let snap = ui.latest.load_full();
    let other = band_series(&snap, "other").expect("other band");
    assert_eq!(other.period_ns, 10_000_000_000);
    assert!(other.samples.iter().all(|s| s.is_valid()));
    assert_eq!(
        snap.health.get(SourceId::Processes).state,
        SourceState::Running
    );

    ui.demand_tx
        .send(Demand {
            device_history: true,
            ..table_demand()
        })
        .expect("watch has receivers");
    let times = other_band_times(&mut fl, &ui, Duration::from_secs(5));
    assert!(times.len() >= 5, "{times:?}");
    assert!(times.windows(2).all(|w| w[1] - w[0] == 1_000), "{times:?}");
    let other = band_series(&ui.latest.load_full(), "other").expect("other band");
    assert_eq!(other.period_ns, 1_000_000_000);

    handle.abort();
    fl.advance(Duration::from_millis(10));
    assert!(handle.is_finished(), "engine_root returned after cancel");
    drop(ui);
    fl.finish();
}

/// The real `ProcessesSource` on the lab clock, so its CPU rates use lab time.
fn lab_processes_factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let src = ProcessesSource::new(Arc::clone(&cfg))
            .expect("fixture source opens")
            .with_clock(lab_clock);
        async move { source::run(cx, io, src).await }
    })
}

fn lab_processes_spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Processes,
        name: "processes",
        modes: &[EngineMode::Full],
        factory: lab_processes_factory,
    }
}

/// Adds `utime` and `stime` clock ticks to the `stat` of `pid`.
fn add_pid_ticks(root: &FixtureRoot, pid: u32, utime: u64, stime: u64) {
    let stat = root.read(pid, "stat");
    let close = stat.rfind(')').expect("stat has a close paren");
    let mut fields: Vec<String> = stat[close + 1..]
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    // fields[0] is field 3 (state), so utime (field 14) is fields[11] and stime fields[12].
    for (i, add) in [(11, utime), (12, stime)] {
        let ticks: u64 = fields[i].parse().expect("tick count");
        fields[i] = (ticks + add).to_string();
    }
    root.write(
        pid,
        "stat",
        format!("{} {}\n", &stat[..=close], fields.join(" ")),
    );
}

/// Adds `delta` to the first fields of the `cpu` line of `/proc/stat` (user, nice, system,
/// idle, iowait, irq, softirq, steal).
fn add_stat_ticks(root: &FixtureRoot, delta: [u64; 8]) {
    let path = root.root().join("proc/stat");
    let text = fs::read_to_string(&path).expect("read /proc/stat");
    let (first, rest) = text.split_once('\n').expect("/proc/stat has lines");
    let mut fields: Vec<u64> = first
        .strip_prefix("cpu ")
        .expect("first line is the cpu line")
        .split_whitespace()
        .map(|f| f.parse().expect("tick count"))
        .collect();
    for (field, add) in fields.iter_mut().zip(delta) {
        *field += add;
    }
    let line: Vec<String> = fields.iter().map(u64::to_string).collect();
    fs::write(&path, format!("cpu  {}\n{rest}", line.join(" "))).expect("write /proc/stat");
}

/// The last `CPU_GROUP` value of every band in `snap.system.cpu.groups`, by key.
fn cpu_band_values(snap: &Snapshot) -> Vec<(Arc<str>, f64)> {
    snap.system
        .cpu
        .groups
        .iter()
        .map(|b| {
            let id = SeriesId {
                device: b.device,
                metric: MetricId::CPU_GROUP,
            };
            let last = snap
                .series
                .get(id)
                .and_then(|v| v.samples.last().copied())
                .expect("a published band has a sample");
            assert!(last.is_valid(), "band {} is a gap", b.key);
            (Arc::clone(&b.key), f64::from(last.value))
        })
        .collect()
}

/// D-042, UI-PRF-062 "Sweep rate": with only device history on and no window, the 10 s group
/// sweep keeps each pid's CPU baseline, so the `CPU_GROUP` series get a sample every 10 s with
/// the process CPU in the groups. Every sample sums to busy CPU of the sweep's `/proc/stat`
/// window. The reducer hands each of these batches to the metric sink. Fed through the history
/// worker's `State` and read back from disk, the 10 s tier holds one point per sweep, and the
/// points of each 10 s sum to busy CPU.
#[test]
fn cpu_group_history_only_sweep() {
    let root = FixtureRoot::new(&["owner-desktop"]);
    fs::create_dir_all(root.root().join("sys/fs/cgroup")).expect("create cgroup dir");
    let mut fl = FakeLab::new();
    CLOCK.with(|c| *c.borrow_mut() = Some(Arc::clone(&fl.clock)));
    let (ep, ui) = fake_sources::root_endpoints(root.config(), vec![lab_processes_spec()]);
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        engine_root(cx, ep).await;
    });
    ui.demand_tx
        .send(history_demand())
        .expect("watch has receivers");

    // Each second of lab time, on the fixture's 32 online CPUs: systemd (pid 1) uses 1% of all
    // CPUs, the kernel thread ksoftirqd/0 (pid 15) 0.5%. The `cpu` line moves by 3200 ticks:
    // user 20%, system 10%, irq 1%, softirq 2%, the rest idle. So busy is 33% and
    // "Short-lived processes" is 30% minus the 1.5% of the two processes.
    let one_pct = rustix::param::clock_ticks_per_second() * 32 / 100;
    let stat_delta = [640, 0, 320, 2144, 0, 32, 64, 0];
    let close = |got: f64, want: f64, what: &str| {
        assert!((got - want).abs() <= 1e-3, "{what} is {got}, want {want}");
    };
    let other_len = |snap: &Snapshot| {
        let band = snap.system.cpu.groups.iter().find(|b| &*b.key == "other")?;
        let id = SeriesId {
            device: band.device,
            metric: MetricId::CPU_GROUP,
        };
        snap.series.get(id).map(|v| v.samples.len())
    };

    let mut seen = 0;
    let mut times = Vec::new();
    // Each sweep's `CPU_GROUP` samples, as the reducer gives them to the metric sink.
    let mut batches: Vec<Vec<(SeriesId, Sample)>> = Vec::new();
    while fl.now_ms() < 60_000 {
        fl.advance(Duration::from_millis(100));
        // On the half second, so every 10 s sweep window holds exactly ten of these.
        if fl.now_ms() % 1_000 == 500 {
            add_pid_ticks(&root, 1, one_pct, 0);
            add_pid_ticks(&root, 15, 0, one_pct / 2);
            add_stat_ticks(&root, stat_delta);
        }
        let snap = ui.latest.load_full();
        let Some(n) = other_len(&snap) else { continue };
        if n == seen {
            continue;
        }
        seen = n;
        times.push(fl.now_ms());
        let bands = cpu_band_values(&snap);
        let value = |key: &str| {
            bands
                .iter()
                .find(|(k, _)| &**k == key)
                .map(|(_, v)| *v)
                .unwrap_or_else(|| panic!("no band {key}"))
        };
        assert!(bands.iter().all(|(_, v)| *v >= 0.0), "{bands:?}");
        close(
            bands.iter().map(|(_, v)| v).sum(),
            33.0,
            "the sum of the bands",
        );
        close(value("cpu:irq"), 1.0, "Hardware interrupts");
        close(value("cpu:softirq"), 2.0, "Software interrupts");
        close(value("cpu:steal"), 0.0, "Steal");
        close(value("cpu:kthreads"), 0.5, "Kernel threads");
        close(value("cpu:short"), 28.5, "Short-lived processes");
        let apps: f64 = bands
            .iter()
            .filter(|(k, _)| !k.starts_with("cpu:"))
            .map(|(_, v)| v)
            .sum();
        close(apps, 1.0, "the groups and Other apps");
        let batch = snap.system.cpu.groups.iter().map(|b| {
            let id = SeriesId {
                device: b.device,
                metric: MetricId::CPU_GROUP,
            };
            let last = snap.series.get(id).and_then(|v| v.samples.last().copied());
            (id, last.expect("a published band has a sample"))
        });
        batches.push(batch.collect());
    }
    // The sweep at 0 s has no window, so it gives no CPU values. Each later one does.
    assert_eq!(times, [10_000, 20_000, 30_000, 40_000, 50_000, 60_000]);
    let snap = ui.latest.load_full();
    let band = snap.system.cpu.groups.first().expect("a CPU band");
    let id = SeriesId {
        device: band.device,
        metric: MetricId::CPU_GROUP,
    };
    assert_eq!(
        snap.series.get(id).expect("series").period_ns,
        10_000_000_000
    );

    // The history worker's store: the batches go in under this session's catalog, and closing
    // the store writes both minutes. A new store on the same folder reads the 10 s tier.
    const SEC: u64 = 1_000_000_000;
    // A whole multiple of 60 s, so 10 s and 1 min points start on whole realtime buckets.
    let offset = 1_699_999_200 * SEC as i64;
    let open = |now: u64| {
        let cfg = MetricConfig {
            root: root.dir.join("metrics"),
            boot: fake_sources::FAKE_BOOT,
            realtime_offset_ns: offset,
            max_disk_bytes: 64 << 20,
            sync: false,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        MetricHistory::open(cfg, now, deadline)
            .expect("the device history opens")
            .0
    };
    let mut st = State::new();
    st.install(open(0), offset);
    st.set_catalog(DeviceCatalog::clone(&snap.devices));
    for batch in &batches {
        st.push(batch);
    }
    st.close(61 * SEC).expect("the device history closes");
    let span = open(61 * SEC)
        .read(Tier::S10, offset, offset + 70 * SEC as i64, 61 * SEC)
        .expect("the 10 s tier reads");
    assert_eq!(span.step_ns, 10 * SEC);
    assert_eq!(span.series.len(), snap.system.cpu.groups.len());
    assert!(
        span.series
            .iter()
            .all(|(key, _)| key.metric == MetricId::CPU_GROUP)
    );
    let mut sums: BTreeMap<i64, f64> = BTreeMap::new();
    for (_, points) in &span.series {
        for p in points.iter().filter(|p| !p.mean.is_nan()) {
            *sums.entry(p.t_realtime_ns - offset).or_default() += f64::from(p.mean);
        }
    }
    let stored: Vec<i64> = sums.keys().copied().collect();
    let want: Vec<i64> = (1..=6).map(|i| i * 10 * SEC as i64).collect();
    assert_eq!(stored, want, "10 s points with CPU values");
    for (t, sum) in &sums {
        close(*sum, 33.0, &format!("the stored bands at {t} ns"));
    }

    handle.abort();
    fl.advance(Duration::from_millis(10));
    assert!(handle.is_finished(), "engine_root returned after cancel");
    drop(ui);
    fl.finish();
}

/// tasklens-8q0.42: through the real sweep, a unit row whose unit cgroup's `cgroup.events` says
/// `frozen 1` is frozen, and so are its processes. No unit sample is involved.
#[test]
fn unit_row_frozen_from_sweep() {
    const SESSION: &str = "user.slice/user-1000.slice/session-2.scope";
    let root = FixtureRoot::merged();
    // TOKENPROC is in session-2.scope; a second process makes the unit fold into a row.
    root.clone_pid(TOKENPROC, 4300);
    let cgroup = root.root().join("sys/fs/cgroup").join(SESSION);
    fs::create_dir_all(&cgroup).expect("create cgroup dir");
    fs::write(cgroup.join("cgroup.events"), "populated 1\nfrozen 1\n").expect("write events");
    let mut fl = FakeLab::new();
    let (ep, ui) = fake_sources::root_endpoints(root.config(), vec![processes::spec()]);
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        engine_root(cx, ep).await;
    });
    ui.demand_tx
        .send(table_demand())
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));

    let snap = ui.latest.load_full();
    let unit = snap
        .table
        .groups
        .iter()
        .find(|g| g.kind == GroupRowKind::Unit && &*g.name == "session-2.scope")
        .expect("session-2.scope unit row");
    assert!(unit.frozen);
    assert_eq!(unit.members, 2);
    let procs = &snap.processes;
    for pid in [TOKENPROC, 4300] {
        let i = procs
            .records
            .iter()
            .position(|r| r.key.pid == pid)
            .unwrap_or_else(|| panic!("pid {pid} swept"));
        assert!(procs.extras[i].frozen, "pid {pid} frozen");
    }
    assert!(
        snap.table
            .groups
            .iter()
            .filter(|g| &*g.name != "session-2.scope")
            .all(|g| !g.frozen),
        "no other row is frozen"
    );

    handle.abort();
    fl.advance(Duration::from_millis(10));
    drop(ui);
    fl.finish();
}

/// tasklens-8q0.42 review: two other users' `pipewire.service`, one process each, are both
/// System and fold into one unit row. The row is frozen when both cgroups are, and
/// `wireplumber.service`, frozen for one user only, is not.
#[test]
fn unit_row_frozen_across_users() {
    let root = FixtureRoot::merged();
    let unit = |uid: u32, name: &str| {
        format!("user.slice/user-{uid}.slice/user@{uid}.service/session.slice/{name}")
    };
    let units = [
        (4330, unit(1001, "pipewire.service"), true),
        (4331, unit(1002, "pipewire.service"), true),
        (4340, unit(1001, "wireplumber.service"), false),
        (4341, unit(1002, "wireplumber.service"), true),
    ];
    for (pid, cgroup, frozen) in &units {
        root.clone_pid(TOKENPROC, *pid);
        root.write(*pid, "cgroup", format!("0::/{cgroup}\n"));
        let dir = root.root().join("sys/fs/cgroup").join(cgroup);
        fs::create_dir_all(&dir).expect("create cgroup dir");
        let events = format!("populated 1\nfrozen {}\n", u8::from(*frozen));
        fs::write(dir.join("cgroup.events"), events).expect("write events");
    }
    let mut fl = FakeLab::new();
    let (ep, ui) = fake_sources::root_endpoints(root.config(), vec![processes::spec()]);
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        engine_root(cx, ep).await;
    });
    ui.demand_tx
        .send(table_demand())
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(2), Duration::from_millis(10));

    let snap = ui.latest.load_full();
    let row = |name: &str| {
        snap.table
            .groups
            .iter()
            .find(|g| g.kind == GroupRowKind::Unit && &*g.name == name)
            .unwrap_or_else(|| panic!("{name} unit row"))
    };
    let pipewire = row("pipewire.service");
    assert_eq!(pipewire.members, 2);
    assert!(pipewire.frozen);
    let wireplumber = row("wireplumber.service");
    assert_eq!(wireplumber.members, 2);
    assert!(!wireplumber.frozen);

    handle.abort();
    fl.advance(Duration::from_millis(10));
    drop(ui);
    fl.finish();
}

#[test]
fn low_memory_mode_runs_processes_and_kwin() {
    // KWin runs for keep above only (R-152).
    let ids: Vec<SourceId> = workers_for(EngineMode::LowMemory)
        .iter()
        .map(|spec| spec.id)
        .collect();
    assert_eq!(ids, [SourceId::Processes, SourceId::KWin]);
}
