//! Cgroup freeze, thaw and kill on a real transient user scope (ADR-006, ADR-013, E11-010).
//!
//! Run on the owner machine with
//! `heavy-build cargo test -p tl-engine --test real_cgroup -- --ignored`. Each test that writes
//! starts its own scope `tasklens-test-<pid>-<tag>.scope` with a `sleep` inside and stops it on
//! drop, also when the test fails. No other unit or cgroup is written to.
//!
//! Validation errors use tl-ops' real variants (leader note on i3o.11): a stale `cgroup_id` is
//! `Rejected(IdentityChanged)` and a path outside `app.slice` is `Rejected(NotOwner)`.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use asupersync::runtime::{Runtime, RuntimeBuilder};
use tl_engine::actions::{ActionCtx, RealOps, cgroup};
use tl_engine::dbus::ManagerConnSlot;
use tl_engine::{EngineConfig, EngineMode};
use tl_model::actions::{ActionKind, ActionState, CgroupRef, RejectReason};
use tl_model::time::BootId;

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
/// Upper bound for the scope to appear and for each action to finish.
const WAIT: Duration = Duration::from_secs(10);
/// The bead's limit for the `sleep` to be gone after `KillGroup`.
const KILL_LIMIT: Duration = Duration::from_secs(1);

/// A transient user scope running `sleep 60`. Stopped on drop.
struct Scope {
    unit: String,
    child: Child,
    cg: CgroupRef,
}

impl Scope {
    fn start(tag: &str) -> Self {
        let unit = format!("tasklens-test-{}-{tag}.scope", std::process::id());
        let child = Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", &format!("--unit={unit}")])
            .args(["sleep", "60"])
            .stdin(Stdio::null())
            .spawn()
            .expect("systemd-run runs");
        // From here the guard stops the unit, also when the wait below panics.
        let mut scope = Scope {
            unit,
            child,
            cg: CgroupRef {
                rel_path: Arc::from(""),
                cgroup_id: 0,
            },
        };
        let rel = scope.wait_in_scope();
        let ino = fs::metadata(Path::new(CGROUP_ROOT).join(&rel))
            .expect("scope cgroup exists")
            .ino();
        scope.cg = CgroupRef {
            rel_path: Arc::from(rel),
            cgroup_id: ino,
        };
        scope
    }

    /// The child's cgroup once `systemd-run` has moved it into the scope, relative to the root.
    fn wait_in_scope(&mut self) -> String {
        let path = format!("/proc/{}/cgroup", self.child.id());
        let begin = Instant::now();
        loop {
            let text = fs::read_to_string(&path).unwrap_or_default();
            if let Some(rel) = text.lines().find_map(|l| l.strip_prefix("0::/"))
                && rel.ends_with(&format!("/{}", self.unit))
            {
                return rel.to_owned();
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("systemd-run exited before the scope started: {status}");
            }
            assert!(begin.elapsed() < WAIT, "no scope after {WAIT:?}: {text:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn dir(&self) -> PathBuf {
        Path::new(CGROUP_ROOT).join(&*self.cg.rel_path)
    }

    /// The `frozen` value in the scope's `cgroup.events`.
    fn frozen(&self) -> Option<u8> {
        let text = fs::read_to_string(self.dir().join("cgroup.events")).ok()?;
        text.lines()
            .find_map(|l| l.strip_prefix("frozen "))?
            .parse()
            .ok()
    }

    /// The scope's `cgroup.procs`. A removed scope counts as empty.
    fn procs(&self) -> String {
        fs::read_to_string(self.dir().join("cgroup.procs")).unwrap_or_default()
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        for verb in ["thaw", "stop", "reset-failed"] {
            let _ = Command::new("systemctl")
                .args(["--user", verb, &self.unit])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        // The child is this test's own `sleep`; reap it so it never lingers.
        let _ = self.child.kill();
        let _ = self.child.wait();
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

/// Runs one `cgroup::run_target` with `RealOps` on the real runtime and returns its state.
fn run(rt: &Runtime, kind: ActionKind, cg: &CgroupRef) -> ActionState {
    let (events, _events_rx) = async_channel::bounded(16);
    let (msgs, _msgs_rx) = asupersync::channel::mpsc::channel(16);
    let actx = Arc::new(ActionCtx {
        events,
        msgs,
        ops: Arc::new(RealOps),
        boot_id: BootId(1),
        uid: uid(),
        config: Arc::new(EngineConfig::new(
            EngineMode::Full,
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("real_cgroup-state"),
        )),
        systemd_system: ManagerConnSlot::new(),
        systemd_user: ManagerConnSlot::new(),
        openers: None,
        helper: None,
        destroy_support_sent: std::sync::atomic::AtomicBool::new(false),
        unit_bus_override: None,
    });
    let cg = cg.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    rt.handle()
        .try_spawn_with_cx(move |cx| async move {
            let state = cgroup::run_target(&cx, &actx, &kind, &cg).await;
            let _ = tx.send(state);
        })
        .expect("task spawns");
    rx.recv_timeout(WAIT).expect("run_target finished")
}

#[test]
#[ignore = "owner machine: starts a transient user scope"]
fn freeze_thaw_kill_transient_scope() {
    let mut scope = Scope::start("ftk");
    let rt = runtime();
    assert_eq!(scope.frozen(), Some(0));

    assert_eq!(
        run(&rt, ActionKind::FreezeGroup, &scope.cg),
        ActionState::Succeeded
    );
    assert_eq!(scope.frozen(), Some(1));

    assert_eq!(
        run(&rt, ActionKind::ThawGroup, &scope.cg),
        ActionState::Succeeded
    );
    let begin = Instant::now();
    while scope.frozen() != Some(0) && begin.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(scope.frozen(), Some(0));

    assert_eq!(
        run(&rt, ActionKind::KillGroup, &scope.cg),
        ActionState::Succeeded
    );
    let begin = Instant::now();
    loop {
        let gone = scope.child.try_wait().expect("try_wait").is_some();
        let procs = scope.procs();
        if gone && procs.trim().is_empty() {
            break;
        }
        assert!(
            begin.elapsed() < KILL_LIMIT,
            "after {KILL_LIMIT:?}: sleep gone {gone}, cgroup.procs {procs:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "owner machine: starts a transient user scope"]
fn stale_cgroup_id_rejected() {
    let scope = Scope::start("stale");
    let rt = runtime();
    let stale = CgroupRef {
        rel_path: Arc::clone(&scope.cg.rel_path),
        cgroup_id: scope.cg.cgroup_id + 1,
    };
    for kind in [
        ActionKind::FreezeGroup,
        ActionKind::ThawGroup,
        ActionKind::KillGroup,
    ] {
        assert_eq!(
            run(&rt, kind, &stale),
            ActionState::Rejected(RejectReason::IdentityChanged)
        );
    }
    // Nothing was written: the scope is not frozen and the sleep still runs.
    assert_eq!(scope.frozen(), Some(0));
    assert!(!scope.procs().trim().is_empty());
}

#[test]
#[ignore = "owner machine: reads the live cgroup tree"]
fn session_scope_rejected() {
    let uid = uid();
    let slice = format!("user.slice/user-{uid}.slice");
    let session = fs::read_dir(Path::new(CGROUP_ROOT).join(&slice))
        .expect("user slice exists")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with("session-") && n.ends_with(".scope"));
    let Some(session) = session else {
        eprintln!("session_scope_rejected: skipped, no session-N.scope under {slice}");
        return;
    };
    let rel = format!("{slice}/{session}");
    let ino = fs::metadata(Path::new(CGROUP_ROOT).join(&rel))
        .expect("session scope exists")
        .ino();
    let cg = CgroupRef {
        rel_path: Arc::from(rel),
        cgroup_id: ino,
    };
    // Validation refuses the path before it opens anything, so the session is never touched.
    assert_eq!(
        run(&runtime(), ActionKind::FreezeGroup, &cg),
        ActionState::Rejected(RejectReason::NotOwner)
    );
}

#[test]
#[ignore = "owner machine: reads the live cgroup tree"]
fn own_cgroup_protected() {
    let own = tl_ops::cgroup::own_cgroup().expect("own cgroup reads");
    let prefix = format!(
        "user.slice/user-{0}.slice/user@{0}.service/app.slice/",
        uid()
    );
    let app_unit = own.ends_with(".scope") || own.ends_with(".service");
    if !own.starts_with(&prefix) || !app_unit {
        eprintln!("own_cgroup_protected: skipped, test process cgroup {own:?} is not an app unit");
        return;
    }
    let ino = fs::metadata(Path::new(CGROUP_ROOT).join(&own))
        .expect("own cgroup exists")
        .ino();
    let cg = CgroupRef {
        rel_path: Arc::from(own),
        cgroup_id: ino,
    };
    // `Protected` is checked before any write, so this never freezes the test itself.
    assert_eq!(
        run(&runtime(), ActionKind::FreezeGroup, &cg),
        ActionState::Rejected(RejectReason::Protected)
    );
}
