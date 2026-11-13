//! The real helper path through pkexec and polkit on the owner machine (ADR-006, ADR-013,
//! R-003, R-117, APP-003, PROC-076, E29-011).
//!
//! Each test starts a real `Engine::start` with the default hooks, so `Command::StartHelperSession`
//! runs `pkexec /usr/lib/tasklens/tl-helper --session`, and drives it through `EngineClient` only.
//! The owner answers each polkit prompt by hand; no password is stored or scripted. Run them one
//! at a time, with the instructions visible:
//!
//! `heavy-build cargo test -p tl-engine --test real_helper -- --ignored --test-threads=1`
//!
//! The instructions go straight to stderr, so libtest's output capture does not hide them.
//! Without the installed package (helper, pkexec and the polkit policy) each test prints
//! "skipped: helper not installed" and passes.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode};
use tl_model::actions::{ActionId, ActionKind, ActionRequest, ActionState, ActionTarget, Signal};
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::demand::Demand;
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::ids::ProcessKey;

/// What the package installs. All three must exist, or the test skips.
const HELPER: &str = "/usr/lib/tasklens/tl-helper";
const PKEXEC: &str = "/usr/bin/pkexec";
const POLICY: &str = "/usr/share/polkit-1/actions/io.github.oysteinkrog.Tasklens.policy";
/// The skip line, as the bead words it.
const SKIPPED: &str = "skipped: helper not installed";

/// The system unit the owner starts before `unlock_and_signal_other_user`.
const NOBODY_UNIT: &str = "tasklens-test-nobody";
/// How long the test waits for the owner's unit to show up in the snapshot.
const UNIT_WAIT: Duration = Duration::from_secs(60);
/// How long the owner has to answer a polkit prompt.
const PROMPT_WAIT: Duration = Duration::from_secs(120);
/// How long a privileged action may take once the helper runs.
const ACTION_WAIT: Duration = Duration::from_secs(10);
/// The helper exits after 300 s without a request (R-117); the test waits 305 s.
const IDLE_WAIT: Duration = Duration::from_secs(305);
/// Slack after `IDLE_WAIT` for the engine to notice the exit and publish.
const IDLE_SLACK: Duration = Duration::from_secs(10);
/// The bead's limit for `EndHelperSession`.
const END_LIMIT: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(10);

/// One line to stderr, past libtest's capture.
fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "real_helper: {line}");
}

/// `None` when every path exists, else the skip line naming the first missing path.
fn skip_reason(paths: &[&Path]) -> Option<String> {
    paths
        .iter()
        .find(|p| !p.exists())
        .map(|p| format!("{SKIPPED} ({} missing)", p.display()))
}

/// True when the installed package is there. Prints the skip line when it is not.
fn installed() -> bool {
    match skip_reason(&[Path::new(HELPER), Path::new(PKEXEC), Path::new(POLICY)]) {
        Some(reason) => {
            say(&reason);
            false
        }
        None => true,
    }
}

/// A real engine on the live system. Drop ends the helper session and stops the engine, also
/// when a test fails, so no helper is left running.
struct Rig {
    engine: Option<EngineHandle>,
}

impl Rig {
    fn start(tag: &str, demand: Demand) -> Rig {
        let state = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("real_helper-{tag}"));
        let config = EngineConfig::new(EngineMode::Full, state);
        let engine = Engine::start(config, EngineHooks::default()).expect("engine starts");
        engine.set_demand(demand);
        Rig {
            engine: Some(engine),
        }
    }

    fn engine(&self) -> &EngineHandle {
        self.engine.as_ref().expect("engine runs")
    }

    fn send(&self, cmd: Command) {
        self.engine().send(cmd).expect("command queued");
    }

    fn helper(&self) -> HelperState {
        self.engine().latest().health.helper
    }

    /// Polls the snapshot until `done` holds or `limit` runs out. True when it held.
    fn wait_for(&self, limit: Duration, mut done: impl FnMut(&Rig) -> bool) -> bool {
        let begin = Instant::now();
        loop {
            if done(self) {
                return true;
            }
            if begin.elapsed() > limit {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Asks for the helper and waits for the owner to authenticate.
    fn unlock(&self) {
        let before = self.helper();
        assert_eq!(before, HelperState::Locked, "helper before the start");
        say("a polkit prompt opens now: authenticate as an administrator");
        self.send(Command::StartHelperSession);
        assert!(
            self.wait_for(PROMPT_WAIT, |r| r.helper() == HelperState::Unlocked),
            "helper not Unlocked within {PROMPT_WAIT:?}: {:?}, start {:?}",
            self.helper(),
            self.engine().latest().health.helper_start,
        );
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let Some(engine) = self.engine.take() else {
            return;
        };
        if engine.latest().health.helper != HelperState::Locked {
            let _ = engine.send(Command::EndHelperSession);
            let begin = Instant::now();
            while engine.latest().health.helper != HelperState::Locked
                && begin.elapsed() < END_LIMIT
            {
                std::thread::sleep(POLL);
            }
        }
        let report = engine.shutdown();
        if !report.clean() {
            say(&format!("engine shutdown was not clean: {report:?}"));
        }
    }
}

/// The fields of `/proc/<pid>/stat` after the `(comm)` field. Index 0 is field 3 (state).
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &text[text.rfind(')')? + 1..];
    Some(rest.split_whitespace().map(str::to_owned).collect())
}

/// True while `pid` is a process that has not exited (zombies count as gone).
fn running(pid: u32) -> bool {
    stat_fields(pid)
        .and_then(|f| f.first().cloned())
        .is_some_and(|state| !matches!(state.as_str(), "Z" | "X" | "x"))
}

/// The running `tl-helper` processes this test process started. pkexec execs the helper in
/// the child the engine spawned, so the helper's parent is this process.
fn own_helpers() -> Vec<u32> {
    let me = std::process::id().to_string();
    let Ok(dir) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| {
            let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
            comm.trim() == "tl-helper"
                && stat_fields(pid).is_some_and(|f| f.get(1) == Some(&me))
                && running(pid)
        })
        .collect()
}

/// `systemctl show -P <property> <unit>` on the system manager. Reading needs no root.
fn unit_property(property: &str) -> String {
    let out = Proc::new("systemctl")
        .args(["show", "-P", property, NOBODY_UNIT])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect("systemctl runs");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Checks on drop that the owner's `nobody` unit is gone, and prints the command that stops it
/// when it is not.
struct NobodyUnit;

impl Drop for NobodyUnit {
    fn drop(&mut self) {
        if unit_property("ActiveState") == "active" {
            say(&format!(
                "{NOBODY_UNIT} still runs; stop it with: sudo systemctl stop {NOBODY_UNIT}"
            ));
        }
    }
}

fn process_demand() -> Demand {
    Demand {
        any_surface_visible: true,
        process_table: true,
        ..Demand::default()
    }
}

/// Waits for the first final state of action `id`.
fn final_state(rig: &Rig, id: ActionId) -> ActionState {
    let events = rig.engine().action_events();
    let begin = Instant::now();
    loop {
        while let Ok(ev) = events.try_recv() {
            if ev.id == id && matches!(ev.target, None | Some(0)) && ev.state.is_final() {
                return ev.state;
            }
        }
        assert!(
            begin.elapsed() < ACTION_WAIT,
            "action {id:?} had no final state within {ACTION_WAIT:?}"
        );
        std::thread::sleep(POLL);
    }
}

#[test]
#[ignore = "owner machine: polkit prompt, signals the owner's nobody unit as root"]
fn unlock_and_signal_other_user() {
    if !installed() {
        return;
    }
    let _unit = NobodyUnit;
    let rig = Rig::start("signal", process_demand());
    say(&format!(
        "run now, within {} s: sudo systemd-run --uid=nobody --unit={NOBODY_UNIT} sleep 300",
        UNIT_WAIT.as_secs()
    ));
    let mut key: Option<ProcessKey> = None;
    let found = rig.wait_for(UNIT_WAIT, |r| {
        let Ok(pid) = unit_property("MainPID").parse::<u32>() else {
            return false;
        };
        if pid == 0 {
            return false;
        }
        let snap = r.engine().latest();
        key = snap
            .processes
            .records
            .iter()
            .find(|rec| rec.key.pid == pid)
            .map(|rec| rec.key);
        key.is_some()
    });
    assert!(
        found,
        "{NOBODY_UNIT} not in the snapshot within {UNIT_WAIT:?}"
    );
    let key = key.expect("found");

    rig.unlock();
    let id = ActionId(1);
    rig.send(Command::Action(ActionRequest {
        id,
        kind: ActionKind::Signal(Signal::Term),
        targets: std::iter::once(ActionTarget::Process(key)).collect(),
        privileged: true,
    }));
    assert_eq!(final_state(&rig, id), ActionState::Succeeded);
    assert!(
        rig.wait_for(ACTION_WAIT, |_| !running(key.pid)),
        "pid {} still runs after Term",
        key.pid
    );
}

#[test]
#[ignore = "owner machine: polkit prompt the owner cancels"]
fn dismissed_prompt_locks_again() {
    if !installed() {
        return;
    }
    let rig = Rig::start("dismiss", Demand::default());
    assert_eq!(rig.helper(), HelperState::Locked, "helper before the start");
    say("a polkit prompt opens now: press Cancel");
    rig.send(Command::StartHelperSession);
    assert!(
        rig.wait_for(PROMPT_WAIT, |r| r.helper() == HelperState::Starting),
        "helper never Starting: {:?}",
        rig.helper()
    );
    assert!(
        rig.wait_for(PROMPT_WAIT, |r| r.helper() == HelperState::Locked),
        "helper not Locked again within {PROMPT_WAIT:?}: {:?}",
        rig.helper()
    );
    let start = rig.engine().latest().health.helper_start.clone();
    assert!(
        matches!(start, Some((_, HelperStartOutcome::Cancelled))),
        "helper_start: {start:?}"
    );
}

#[test]
#[ignore = "owner machine: polkit prompt, then 305 s idle"]
fn idle_exit_relocks() {
    if !installed() {
        return;
    }
    // No demand, so nothing sends the helper a request while it idles.
    let rig = Rig::start("idle", Demand::default());
    rig.unlock();
    say(&format!(
        "unlocked; waiting {} s with no requests",
        IDLE_WAIT.as_secs()
    ));
    std::thread::sleep(IDLE_WAIT);
    assert!(
        rig.wait_for(IDLE_SLACK, |r| r.helper() == HelperState::Locked),
        "helper not Locked after {:?} idle: {:?}",
        IDLE_WAIT + IDLE_SLACK,
        rig.helper()
    );
    assert!(own_helpers().is_empty(), "helper still runs");
}

#[test]
#[ignore = "owner machine: polkit prompt, then ends the session"]
fn end_session_locks() {
    if !installed() {
        return;
    }
    let rig = Rig::start("end", Demand::default());
    rig.unlock();
    let helpers = own_helpers();
    assert!(!helpers.is_empty(), "no tl-helper child after the unlock");
    let begin = Instant::now();
    rig.send(Command::EndHelperSession);
    assert!(
        rig.wait_for(END_LIMIT, |r| r.helper() == HelperState::Locked),
        "helper not Locked within {END_LIMIT:?}: {:?}",
        rig.helper()
    );
    let left = END_LIMIT.saturating_sub(begin.elapsed());
    assert!(
        rig.wait_for(left, |_| helpers.iter().all(|&p| !running(p))),
        "helper pids {helpers:?} still run after {END_LIMIT:?}"
    );
}

/// The skip path, shown without the package: on the owner machine it is installed, so the four
/// tests above cannot show it without a prompt.
#[test]
fn skip_reason_names_missing_install() {
    let missing = Path::new("/nonexistent/tasklens/tl-helper");
    let reason = skip_reason(&[missing, Path::new("/nonexistent/pkexec")]).expect("skips");
    assert!(reason.starts_with(SKIPPED), "{reason}");
    assert!(
        reason.contains("/nonexistent/tasklens/tl-helper"),
        "{reason}"
    );
    assert_eq!(skip_reason(&[Path::new("/")]), None);
}
