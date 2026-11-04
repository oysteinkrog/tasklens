//! Shutdown while the device history journal fetch is still running (tasklens-gd3ky).
//!
//! Runs the real engine on the real runtime. A fake `journalctl` first on `PATH` never exits on
//! its own, so the `MetricHistory` worker's first journal fetch is still waiting on it when
//! shutdown begins. Shutdown must stop the fetch, kill the child and stay within the ADR-004
//! budgets. One test in its own binary: it changes `PATH` for the whole process.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tl_collect::sysroot::SysRoot;
use tl_engine::bridge::{DRAIN_BUDGET, Engine, EngineHooks, TEARDOWN_BUDGET};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::desktop::DesktopSource;
use tl_model::client::EngineClient;
use tl_model::demand::Demand;

/// How long the test waits for the worker to start the fake `journalctl`.
const START_WAIT: Duration = Duration::from_secs(10);
/// How long the test waits for the killed fake to be gone after shutdown.
const GONE_WAIT: Duration = Duration::from_secs(2);

/// Writes a fake `journalctl` into `dir`. It writes its pid to `dir/pid`, then becomes
/// `sleep 30`, so the fetch only ends when the worker kills it.
fn fake_journalctl(dir: &Path) {
    let script = dir.join("journalctl");
    std::fs::write(
        &script,
        "#!/bin/sh\nd=\"$(dirname \"$0\")\"\necho $$ > \"$d/pid.tmp\"\nmv \"$d/pid.tmp\" \"$d/pid\"\n\
         exec sleep 30\n",
    )
    .expect("write fake journalctl");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("make fake journalctl executable");
}

/// The pid the fake wrote, once it has started.
fn wait_for_pid(dir: &Path) -> u32 {
    let begin = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(dir.join("pid"))
            && let Ok(pid) = text.trim().parse()
        {
            return pid;
        }
        assert!(
            begin.elapsed() < START_WAIT,
            "the fake journalctl did not start within {START_WAIT:?}. The worker skips the \
             journal when the user is in none of the groups wheel, adm or systemd-journal."
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// True while `pid` is a process that has not exited yet (a zombie counts as gone).
fn alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state is the field after the `(comm)` part.
        Ok(stat) => stat
            .rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z')),
        Err(_) => false,
    }
}

#[test]
fn shutdown_stops_a_running_journal_fetch() {
    let scratch = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("scratch dir");
    let bin: PathBuf = scratch.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    fake_journalctl(&bin);
    let path = match std::env::var_os("PATH") {
        Some(old) => {
            let mut dirs = vec![bin.clone()];
            dirs.extend(std::env::split_paths(&old));
            std::env::join_paths(dirs).expect("join PATH")
        }
        None => bin.clone().into_os_string(),
    };
    // SAFETY: the only test in this binary, and no engine thread runs yet.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("PATH", path);
    }

    let base =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/minimal-vm");
    let config = EngineConfig::new(EngineMode::Full, scratch.path().join("state"))
        .with_sys_root(SysRoot::at(&base))
        .with_desktop(DesktopSource::None);
    let handle = Engine::start(config, EngineHooks::default()).expect("engine starts");
    // `history.device_metrics` is on by default; the demand turns the worker's store on, and
    // its first journal fetch starts at once.
    handle.set_demand(Demand {
        device_history: true,
        ..Demand::default()
    });
    let pid = wait_for_pid(&bin);
    assert!(alive(pid), "the fake journalctl exited on its own");

    let report = handle.shutdown();
    assert!(report.clean(), "{report:?}");
    assert!(
        report.elapsed < DRAIN_BUDGET + TEARDOWN_BUDGET,
        "{report:?}"
    );
    // The fetch killed its child rather than leave it running.
    let begin = Instant::now();
    while alive(pid) {
        assert!(
            begin.elapsed() < GONE_WAIT,
            "the fake journalctl (pid {pid}) still runs {GONE_WAIT:?} after shutdown"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
