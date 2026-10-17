//! LabRuntime tests for the `Processes` worker on fixture roots (ADR-013, ADR-008, ADR-016).
//!
//! Each test copies `owner-desktop` with `process-edge` laid over it into the target tmpdir, so
//! it can add pid directories and rewrite files between sweeps. `source::run` drives a real
//! `ProcessesSource` in a `FakeLab`; the sampler reads the lab's virtual clock.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::{mpsc, watch};
use asupersync::runtime::TaskHandle;
use asupersync::time::{TimeSource, VirtualClock};
use fake_sources::{FakeLab, channels, drain, io};
use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::session::engine_root;
use tl_engine::source::{self, WorkerControl};
use tl_engine::update::{EngineMsg, ProcessSweep, SourceUpdate};
use tl_engine::workers::processes::{self, ProcessesSource};
use tl_engine::workers::workers_for;
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::process::{GroupRowKind, ProcessRecord};
use tl_model::settings::EngineSettings;
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
        let mut fl = FakeLab::new();
        CLOCK.with(|c| *c.borrow_mut() = Some(Arc::clone(&fl.clock)));
        let (updates, rx, demand_tx, settings_tx, control) = channels();
        demand_tx.send(demand).expect("watch has receivers");
        settings_tx
            .send(EngineSettings {
                interval_ms,
                ..EngineSettings::default()
            })
            .expect("watch has receivers");
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
