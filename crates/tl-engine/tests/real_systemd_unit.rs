//! The systemd workers on the real system, through the engine's public API (ADR-013, E14-007,
//! SYS-001, SYS-005, UI-SYS-022).
//!
//! Run on the owner machine with
//! `heavy-build cargo test -p tl-engine --test real_systemd_unit -- --ignored`. The fixture
//! writes the user unit `tasklens-test.service` with `create_new`, so it never replaces a file
//! of the owner's, and refuses to run when the unit already exists. On drop it stops the unit,
//! removes the file and reloads the user manager, also when a test fails. Only the user manager
//! is written to (`systemctl --user`); the system manager is only read. Without a session bus
//! each test returns early.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command as Proc, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::demand::Demand;
use tl_model::health::ManagerState;
use tl_model::snapshot::{ServiceStatus, Snapshot, UnitRecord};
use tl_model::table::{
    Manager, ServiceColumn, ServiceFilter, SysColumnId, SysNodeId, SysSortSpec, SysTable,
    SysTableQuery, UnitRef,
};

const TEST_UNIT: &str = "tasklens-test.service";
/// The bead's bound for every wait.
const WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);

/// The tests share the user manager and the unit name, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// True when this process can reach a session bus, so the user manager is there.
fn have_session() -> bool {
    let env = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|v| !v.is_empty());
    let socket = std::env::var_os("XDG_RUNTIME_DIR")
        .is_some_and(|dir| PathBuf::from(dir).join("bus").exists());
    let ok = env || socket;
    if !ok {
        eprintln!("no session bus: skipped");
    }
    ok
}

fn systemctl(args: &[&str]) -> String {
    let out = Proc::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect("systemctl runs");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn load_state() -> String {
    systemctl(&["show", "-P", "LoadState", TEST_UNIT])
}

/// `$XDG_CONFIG_HOME`, or `$HOME/.config` when it is unset or not absolute.
fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".config")
        })
}

/// A file this test created with `create_new`. On drop it is removed, and so is its directory
/// when the test made that too.
struct OwnFile {
    path: PathBuf,
    made_dir: Option<PathBuf>,
}

impl OwnFile {
    fn create(path: PathBuf, text: &str) -> Self {
        let dir = path.parent().expect("has a parent").to_owned();
        let made_dir = (!dir.exists()).then(|| dir.clone());
        fs::create_dir_all(&dir).expect("create the directory");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("{} not created: {e}", path.display()));
        let own = OwnFile { path, made_dir };
        file.write_all(text.as_bytes()).expect("write");
        own
    }
}

impl Drop for OwnFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        if let Some(dir) = &self.made_dir {
            let _ = fs::remove_dir(dir);
        }
    }
}

/// `tasklens-test.service` in `$XDG_CONFIG_HOME/systemd/user`. On drop the unit is stopped,
/// the file removed and the user manager reloaded.
struct TestUnit {
    file: Option<OwnFile>,
}

impl TestUnit {
    fn install() -> Self {
        assert_eq!(
            load_state(),
            "not-found",
            "{TEST_UNIT} already exists; not touched"
        );
        let path = config_home().join("systemd/user").join(TEST_UNIT);
        let file = OwnFile::create(
            path,
            "[Unit]\nDescription=tasklens integration test unit\n\
             [Service]\nExecStart=/usr/bin/sleep 600\n",
        );
        // From here the guard cleans up, also when the check below panics.
        let unit = TestUnit { file: Some(file) };
        systemctl(&["daemon-reload"]);
        assert_eq!(
            load_state(),
            "loaded",
            "the user manager does not see {TEST_UNIT}; its XDG_CONFIG_HOME may differ"
        );
        unit
    }

    fn start(&self) {
        systemctl(&["start", TEST_UNIT]);
    }

    fn stop(&self) {
        systemctl(&["stop", TEST_UNIT]);
    }
}

impl Drop for TestUnit {
    fn drop(&mut self) {
        systemctl(&["stop", TEST_UNIT]);
        drop(self.file.take());
        systemctl(&["daemon-reload"]);
        systemctl(&["reset-failed", TEST_UNIT]);
    }
}

/// The engine, shut down on drop, also when a test fails.
struct RunningEngine(Option<EngineHandle>);

impl RunningEngine {
    fn start(name: &str) -> Self {
        let state_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-state"));
        let config = EngineConfig::new(EngineMode::Full, state_dir);
        let engine = Engine::start(config, EngineHooks::default()).expect("engine starts");
        RunningEngine(Some(engine))
    }

    fn get(&self) -> &EngineHandle {
        self.0.as_ref().expect("engine running")
    }

    /// Sets `demand` and the Services query with both groups and `extra` expanded.
    fn ask(&self, demand: Demand, filter: &str, extra: &[SysNodeId]) {
        self.get().set_demand(demand);
        let mut expanded = vec![
            SysNodeId::ServiceGroup(Manager::System),
            SysNodeId::ServiceGroup(Manager::User),
        ];
        expanded.extend_from_slice(extra);
        let query = SysTableQuery {
            table: SysTable::Services,
            generation: 1,
            sort: SysSortSpec {
                column: SysColumnId::Services(ServiceColumn::Name),
                ascending: true,
            },
            filter: Arc::from(filter),
            expanded: expanded.into(),
            services: Some(ServiceFilter::default()),
        };
        self.get()
            .send(Command::SetSysTableQuery(query))
            .expect("query sent");
    }

    /// Polls `latest()` every 100 ms until `check` returns `Some`, for at most 5 s.
    fn wait_for<T>(&self, what: &str, mut check: impl FnMut(&Snapshot) -> Option<T>) -> T {
        let begin = Instant::now();
        loop {
            let snap = self.get().latest();
            if let Some(found) = check(&snap) {
                return found;
            }
            assert!(
                begin.elapsed() < WAIT,
                "{what}: not within {WAIT:?}; managers {:?}, rows {:?}",
                snap.services.managers,
                snap.services.rows,
            );
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for RunningEngine {
    fn drop(&mut self) {
        if let Some(engine) = self.0.take() {
            let report = engine.shutdown();
            // Not a shutdown test, so `torn_down`, not `drained` (tasklens-cr0.30). A test that
            // already failed keeps its own message; a second panic would abort.
            if !std::thread::panicking() {
                assert!(report.torn_down, "engine shutdown: {report:?}");
            }
        }
    }
}

fn test_unit_ref() -> UnitRef {
    UnitRef {
        manager: Manager::User,
        name: Arc::from(TEST_UNIT),
    }
}

/// The test unit's record when its row is in the published rows.
fn test_unit(snap: &Snapshot) -> Option<UnitRecord> {
    let me = test_unit_ref();
    let services = &snap.services;
    let row = services
        .rows
        .iter()
        .find(|r| r.node == SysNodeId::Unit(me.clone()))?;
    let unit = services.units.get(row.record? as usize)?;
    (unit.unit == me).then(|| unit.clone())
}

fn services_demand() -> Demand {
    Demand {
        any_surface_visible: true,
        services: true,
        ..Demand::default()
    }
}

#[test]
#[ignore = "owner machine: writes and starts the user unit tasklens-test.service"]
fn user_unit_listed_and_tracks_start_stop() {
    let _serial = serial();
    if !have_session() {
        return;
    }
    let unit = TestUnit::install();
    let engine = RunningEngine::start("real_systemd_unit-start_stop");
    engine.ask(services_demand(), "tasklens-test", &[]);

    engine.wait_for("listed as Stopped", |s| {
        test_unit(s).filter(|u| u.status == ServiceStatus::Stopped)
    });

    unit.start();
    let running = engine.wait_for("Running with a MainPID", |s| {
        test_unit(s).filter(|u| u.status == ServiceStatus::Running && u.main_pid != 0)
    });
    eprintln!("{TEST_UNIT} running as pid {}", running.main_pid);

    unit.stop();
    engine.wait_for("Stopped again", |s| {
        test_unit(s).filter(|u| u.status == ServiceStatus::Stopped)
    });

    drop(engine);
    drop(unit);
    assert_eq!(load_state(), "not-found", "{TEST_UNIT} is gone");
}

#[test]
#[ignore = "owner machine: reads the real system manager"]
fn system_manager_lists_units() {
    let _serial = serial();
    if !have_session() {
        return;
    }
    let engine = RunningEngine::start("real_systemd_unit-system");
    engine.ask(services_demand(), "", &[]);

    let n = engine.wait_for("System group with units", |s| {
        let services = &s.services;
        let n = services
            .rows
            .iter()
            .filter(|r| matches!(&r.node, SysNodeId::Unit(u) if u.manager == Manager::System))
            .count();
        (n > 0 && services.managers[0] == ManagerState::Connected).then_some(n)
    });
    eprintln!("{n} system units listed");
}

#[test]
#[ignore = "owner machine: writes and starts the user unit tasklens-test.service"]
fn unit_cgroup_children_listed() {
    let _serial = serial();
    if !have_session() {
        return;
    }
    let unit = TestUnit::install();
    unit.start();
    let engine = RunningEngine::start("real_systemd_unit-children");
    let demand = Demand {
        process_table: true,
        ..services_demand()
    };
    engine.ask(demand, "tasklens-test", &[SysNodeId::Unit(test_unit_ref())]);

    let main_pid = engine.wait_for("Running with a MainPID", |s| {
        test_unit(s)
            .filter(|u| u.status == ServiceStatus::Running && u.main_pid != 0)
            .map(|u| u.main_pid)
    });
    let me = test_unit_ref();
    let key = engine.wait_for("child row for the MainPID", |s| {
        s.services.rows.iter().find_map(|r| match &r.node {
            SysNodeId::UnitProcess(u, key) if *u == me && key.pid == main_pid => Some(*key),
            _ => None,
        })
    });
    eprintln!("child row {key:?}");

    drop(engine);
    drop(unit);
    assert_eq!(load_state(), "not-found", "{TEST_UNIT} is gone");
}
