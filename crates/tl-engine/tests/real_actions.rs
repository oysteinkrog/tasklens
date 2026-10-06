//! Process actions through `RealOps` on real child processes (ADR-006, ADR-013, E11-008).
//!
//! Run on the owner machine with
//! `heavy-build cargo test -p tl-engine --test real_actions -- --ignored`. Every process a test
//! acts on is one it spawned itself (`sleep`, `true`, `sh` and the `sleep`s that `sh` starts, or
//! this test binary re-run as a thread helper). Each test ends its processes in a `Drop` guard,
//! also when it fails: direct children through `Child::kill`, grandchildren through a pidfd
//! opened while they were known to be ours, so a reused PID is never hit.
//!
//! `thread_nice_only_that_tid` comes with thread targets (E11-006).

use std::fs;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use asupersync::runtime::{Runtime, RuntimeBuilder};
use rustix::process::{Pid, PidfdFlags, Signal as RxSignal};
use tl_engine::actions::{ActionCtx, ActionOps, RealOps, process};
use tl_engine::dbus::ManagerConnSlot;
use tl_engine::{EngineConfig, EngineMode};
use tl_model::actions::{ActionKind, ActionState, ActionTarget, RejectReason, Signal};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;
use tl_ops::OpsError;

/// The boot ID the keys and the `ActionCtx` share. tl-ops only compares the two.
const BOOT: BootId = BootId(1);
/// Upper bound for one action and for a setup step.
const WAIT: Duration = Duration::from_secs(10);
/// The bead's limit for a signalled child to be gone.
const GONE_LIMIT: Duration = Duration::from_millis(100);
/// Set in the environment of the re-run test binary that only holds threads.
const THREAD_HELPER_ENV: &str = "TASKLENS_REAL_ACTIONS_THREAD_HELPER";
/// Extra threads the helper starts.
const HELPER_THREADS: usize = 4;
/// How long `StopSettles` waits for a stopped target to show state `T`.
const STOP_SETTLE: Duration = Duration::from_millis(100);
/// How long `StopSettles` then keeps the target stopped, five polls of a 1 ms watcher.
const STOP_HOLD: Duration = Duration::from_millis(5);

/// The fields of `/proc/<pid>/stat` after the `(comm)` field. Index 0 is field 3 (state).
fn stat_fields(path: &str) -> Option<Vec<String>> {
    let text = fs::read_to_string(path).ok()?;
    let rest = &text[text.rfind(')')? + 1..];
    Some(rest.split_whitespace().map(str::to_owned).collect())
}

/// Field `n` (1-based, as in proc(5)) of a stat file.
fn stat_field(path: &str, n: usize) -> Option<String> {
    stat_fields(path)?.get(n.checked_sub(3)?).cloned()
}

/// The key of a live (or not yet reaped) process, from `/proc/<pid>/stat` field 22.
fn capture(pid: u32) -> ProcessKey {
    let ticks = stat_field(&format!("/proc/{pid}/stat"), 22)
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("/proc/{pid}/stat has a start time"));
    ProcessKey {
        start_ticks: ticks,
        boot_id: BOOT,
        pid,
    }
}

/// True while `key` is a process that has not exited: its stat file reads, the start time still
/// matches and the state is not zombie or dead.
fn alive(key: ProcessKey) -> bool {
    let Some(fields) = stat_fields(&format!("/proc/{}/stat", key.pid)) else {
        return false;
    };
    let same = fields.get(22 - 3).and_then(|t| t.parse::<u64>().ok()) == Some(key.start_ticks);
    let state = fields.first().map(String::as_str);
    same && !matches!(state, Some("Z" | "X" | "x"))
}

/// Waits up to `limit` for `key` to stop being alive.
fn gone_within(key: ProcessKey, limit: Duration) -> bool {
    let begin = Instant::now();
    loop {
        if !alive(key) {
            return true;
        }
        if begin.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// This test's own children and grandchildren. Drop kills and reaps the children and kills the
/// grandchildren through their pidfds.
#[derive(Default)]
struct Guard {
    children: Vec<Child>,
    pidfds: Vec<OwnedFd>,
}

impl Guard {
    fn spawn(&mut self, cmd: &mut Command) -> u32 {
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child spawns");
        let pid = child.id();
        self.children.push(child);
        pid
    }

    /// Holds a pidfd for a grandchild so Drop can end it without a PID lookup.
    fn hold(&mut self, pid: u32) {
        let raw = Pid::from_raw(i32::try_from(pid).expect("pid fits")).expect("pid is not 0");
        let fd = rustix::process::pidfd_open(raw, PidfdFlags::empty()).expect("pidfd opens");
        self.pidfds.push(fd);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        for fd in &self.pidfds {
            let _ = rustix::process::pidfd_send_signal(fd, RxSignal::KILL);
        }
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn uid() -> u32 {
    fs::metadata("/proc/self").expect("/proc/self").uid()
}

fn runtime() -> Runtime {
    RuntimeBuilder::new()
        .worker_threads(1)
        .blocking_threads(1, 2)
        .build()
        .expect("runtime builds")
}

fn actx(ops: Arc<dyn ActionOps>) -> Arc<ActionCtx> {
    let (events, _events_rx) = async_channel::bounded(16);
    let (msgs, _msgs_rx) = asupersync::channel::mpsc::channel(16);
    Arc::new(ActionCtx {
        events,
        msgs,
        ops,
        boot_id: BOOT,
        uid: uid(),
        config: Arc::new(EngineConfig::new(
            EngineMode::Full,
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("real_actions-state"),
        )),
        systemd_system: ManagerConnSlot::new(),
        systemd_user: ManagerConnSlot::new(),
        openers: None,
        helper: None,
        destroy_support_sent: AtomicBool::new(false),
        unit_bus_override: None,
        startup_paths: tl_engine::actions::startup::StartupPaths::from_env(),
    })
}

/// Runs one `process::run_target` with `RealOps` on the real runtime.
fn run(kind: ActionKind, key: ProcessKey) -> ActionState {
    let rt = runtime();
    let actx = actx(Arc::new(RealOps));
    let (tx, rx) = std::sync::mpsc::channel();
    rt.handle()
        .try_spawn_with_cx(move |cx| async move {
            let target = ActionTarget::Process(key);
            let state = process::run_target(&cx, &actx, &kind, &target).await;
            let _ = tx.send(state);
        })
        .expect("task spawns");
    rx.recv_timeout(WAIT).expect("run_target finished")
}

/// `RealOps` that, after each SIGSTOP, waits up to `STOP_SETTLE` for the target to show state
/// `T`, then `STOP_HOLD` more, before it returns. The STOP and KILL passes of `kill_tree` are microseconds apart, so
/// without this a watcher polling `/proc` almost never sees the stop. Every call still goes to
/// `RealOps`; the wait only widens the gap between the passes, it never reorders them.
struct StopSettles;

impl ActionOps for StopSettles {
    fn open_verified(&self, key: ProcessKey, boot: BootId) -> Result<OwnedFd, OpsError> {
        RealOps.open_verified(key, boot)
    }

    fn send_signal(&self, pidfd: &OwnedFd, sig: Signal) -> Result<(), OpsError> {
        RealOps.send_signal(pidfd, sig)?;
        if matches!(sig, Signal::Stop)
            && let Some(pid) = pidfd_pid(pidfd)
        {
            let path = format!("/proc/{pid}/stat");
            let begin = Instant::now();
            while stat_field(&path, 3).as_deref() != Some("T") && begin.elapsed() < STOP_SETTLE {
                std::thread::sleep(Duration::from_millis(1));
            }
            std::thread::sleep(STOP_HOLD);
        }
        Ok(())
    }
}

/// The PID behind a pidfd, from the `Pid:` line of its fdinfo.
fn pidfd_pid(pidfd: &OwnedFd) -> Option<u32> {
    let info = fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd())).ok()?;
    info.lines()
        .find_map(|l| l.strip_prefix("Pid:"))?
        .trim()
        .parse()
        .ok()
}

/// Runs one `process::kill_tree` through `StopSettles` on the real runtime.
fn run_tree(keys: &[ProcessKey]) -> Vec<ActionState> {
    let rt = runtime();
    let actx = actx(Arc::new(StopSettles));
    let targets: Vec<ActionTarget> = keys.iter().map(|k| ActionTarget::Process(*k)).collect();
    let (tx, rx) = std::sync::mpsc::channel();
    rt.handle()
        .try_spawn_with_cx(move |cx| async move {
            let states = process::kill_tree(&cx, &actx, &targets).await;
            let _ = tx.send(states);
        })
        .expect("task spawns");
    rx.recv_timeout(WAIT).expect("kill_tree finished")
}

fn sleep_60(guard: &mut Guard) -> ProcessKey {
    capture(guard.spawn(Command::new("sleep").arg("60")))
}

/// The TIDs in `/proc/<pid>/task/`.
fn tids(pid: u32) -> Vec<u32> {
    fs::read_dir(format!("/proc/{pid}/task"))
        .map(|dir| {
            dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The PIDs in `/proc/<pid>/task/<pid>/children`.
fn children_of(pid: u32) -> Vec<u32> {
    fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .map(|text| {
            text.split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
#[ignore = "owner machine: signals a spawned sleep"]
fn signal_term_real_child() {
    let mut guard = Guard::default();
    let key = sleep_60(&mut guard);
    assert_eq!(
        run(ActionKind::Signal(Signal::Term), key),
        ActionState::Succeeded
    );
    // `alive` treats the unreaped zombie as gone.
    assert!(
        gone_within(key, GONE_LIMIT),
        "sleep still runs after {GONE_LIMIT:?}"
    );
}

#[test]
#[ignore = "owner machine: signals a spawned sleep"]
fn changed_start_ticks_is_identity_changed() {
    let mut guard = Guard::default();
    let key = sleep_60(&mut guard);
    let stale = ProcessKey {
        start_ticks: key.start_ticks + 1,
        ..key
    };
    assert_eq!(
        run(ActionKind::Signal(Signal::Term), stale),
        ActionState::Rejected(RejectReason::IdentityChanged)
    );
    std::thread::sleep(Duration::from_millis(50));
    assert!(alive(key), "sleep was signalled through a stale key");
    assert_eq!(
        guard.children[0].try_wait().expect("try_wait"),
        None,
        "sleep exited"
    );
}

/// Not a test of its own: when `THREAD_HELPER_ENV` is set, this re-run of the test binary starts
/// `HELPER_THREADS` threads and sleeps, so `set_nice_real_child_all_tids` has a many-thread child.
#[test]
#[ignore = "helper process for set_nice_real_child_all_tids"]
fn thread_helper() {
    if std::env::var_os(THREAD_HELPER_ENV).is_none() {
        return;
    }
    for _ in 0..HELPER_THREADS {
        std::thread::spawn(|| std::thread::sleep(Duration::from_secs(60)));
    }
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
#[ignore = "owner machine: renices a spawned copy of this test binary"]
fn set_nice_real_child_all_tids() {
    let mut guard = Guard::default();
    let exe = std::env::current_exe().expect("test binary path");
    let pid = guard.spawn(
        Command::new(exe)
            .args(["--exact", "thread_helper", "--ignored", "--test-threads=1"])
            .env(THREAD_HELPER_ENV, "1"),
    );
    // The main thread plus the helper threads, and libtest's own test thread when it uses one.
    let begin = Instant::now();
    while tids(pid).len() < HELPER_THREADS + 1 {
        assert!(begin.elapsed() < WAIT, "helper threads did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let key = capture(pid);
    assert_eq!(run(ActionKind::SetNice(5), key), ActionState::Succeeded);

    let tids = tids(pid);
    assert!(tids.len() > HELPER_THREADS, "threads: {tids:?}");
    for tid in tids {
        let nice = stat_field(&format!("/proc/{pid}/task/{tid}/stat"), 19);
        assert_eq!(nice.as_deref(), Some("5"), "nice of TID {tid}");
    }
}

#[test]
#[ignore = "owner machine: renices a spawned true"]
fn exiting_child_never_succeeds() {
    let mut guard = Guard::default();
    let pid = guard.spawn(&mut Command::new("true"));
    // Not reaped yet, so the stat file reads even when `true` already exited.
    let key = capture(pid);
    let state = run(ActionKind::SetNice(5), key);
    assert!(
        matches!(
            state,
            ActionState::Rejected(RejectReason::IdentityChanged) | ActionState::OutcomeUnknown(_)
        ),
        "got {state:?}"
    );
}

#[test]
#[ignore = "owner machine: ends a spawned sh and its two sleeps"]
fn kill_tree_real_parent_two_children() {
    let mut guard = Guard::default();
    let sh = guard.spawn(Command::new("sh").args(["-c", "sleep 60 & sleep 60 & wait"]));
    let begin = Instant::now();
    let kids = loop {
        let kids = children_of(sh);
        if kids.len() == 2 {
            break kids;
        }
        assert!(begin.elapsed() < WAIT, "sh children: {kids:?}");
        std::thread::sleep(Duration::from_millis(10));
    };
    for &kid in &kids {
        guard.hold(kid);
    }
    let parent = capture(sh);
    let children: Vec<ProcessKey> = kids.iter().map(|&p| capture(p)).collect();

    // One watcher per child records whether it saw state `T` before the child vanished.
    let stop = Arc::new(AtomicBool::new(false));
    let watchers: Vec<_> = children
        .iter()
        .map(|&key| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let path = format!("/proc/{}/stat", key.pid);
                let mut saw_stopped = false;
                while !stop.load(Ordering::Relaxed) && alive(key) {
                    saw_stopped |= stat_field(&path, 3).as_deref() == Some("T");
                    std::thread::sleep(Duration::from_millis(1));
                }
                saw_stopped
            })
        })
        .collect();

    let mut keys = vec![parent];
    keys.extend(&children);
    let states = run_tree(&keys);
    assert_eq!(states, vec![ActionState::Succeeded; 3]);

    for key in &keys {
        assert!(gone_within(*key, WAIT), "pid {} survived", key.pid);
    }
    stop.store(true, Ordering::Relaxed);
    for (watcher, key) in watchers.into_iter().zip(&children) {
        let saw = watcher.join().expect("watcher ran");
        assert!(saw, "pid {} was never seen stopped", key.pid);
    }
}
