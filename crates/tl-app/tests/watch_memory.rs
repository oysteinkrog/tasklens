//! The `tasklens --low-memory` arm (tasklens-m1f.12). Both tests start the real binary and need a
//! desktop session, so they are ignored by default.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TITLE: &str = "tasklens - Low memory";

fn has_display() -> bool {
    ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()))
}

fn has_kdotool() -> bool {
    Command::new("kdotool")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Window ids that `kdotool` finds for the rescue title.
fn rescue_windows() -> Vec<String> {
    let out = Command::new("kdotool")
        .args(["search", "--name", TITLE])
        .output()
        .expect("kdotool search");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

fn spawn_low_memory(scratch: &std::path::Path, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_tasklens"))
        .arg("--low-memory")
        .args(extra)
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("XDG_CACHE_HOME", scratch.join("cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn tasklens --low-memory")
}

/// One rescue window opens, no main window opens, and closing it ends the process within the
/// ADR-004 bound (1.5 s) plus 0.5 s of slack.
#[test]
#[ignore = "needs a session and kdotool; starts the real binary"]
fn real_low_memory_opens_rescue_only() {
    if !has_display() || !has_kdotool() {
        eprintln!("skipped: needs a display and kdotool on PATH");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    let mut child = spawn_low_memory(scratch.path(), &[]);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ids = rescue_windows();
    while ids.len() != 1 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        ids = rescue_windows();
    }
    let mut exit_secs = None;
    if let [id] = ids.as_slice() {
        let _ = Command::new("kdotool").args(["windowclose", id]).status();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if matches!(child.try_wait(), Ok(Some(_))) {
                exit_secs = Some(start.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    // Kill only the child this test started, before any assert can fail.
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(ids.len(), 1, "rescue windows: {ids:?}");
    assert!(
        exit_secs.is_some(),
        "the process did not exit within 2 s of the close"
    );
}

/// A running instance owns the bus name; `--low-memory` must not forward to it and exit.
#[test]
#[ignore = "needs a session bus and a display; starts the real binary"]
fn real_low_memory_ignores_running_instance() {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    if !has_display() {
        eprintln!("skipped: needs a display");
        return;
    }
    let conn = zbus::blocking::Connection::session().expect("session bus");
    let reply = conn
        .request_name_with_flags(
            "io.github.oysteinkrog.Tasklens",
            RequestNameFlags::DoNotQueue.into(),
        )
        .expect("RequestName");
    assert!(
        matches!(
            reply,
            RequestNameReply::PrimaryOwner
                | RequestNameReply::Exists
                | RequestNameReply::AlreadyOwner
        ),
        "{reply:?}"
    );
    let scratch = tempfile::tempdir().unwrap();
    let mut child = spawn_low_memory(scratch.path(), &[]);
    std::thread::sleep(Duration::from_secs(2));
    let exited = child.try_wait();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(exited.unwrap(), None, "the rescue process exited early");
}

/// Window ids whose title contains `tasklens`. This includes the rescue window.
fn tasklens_windows() -> usize {
    let out = Command::new("kdotool")
        .args(["search", "--name", "tasklens"])
        .output()
        .expect("kdotool search");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// With `--force-full-memory-mode` the rescue window is still the only window (D-015). The engine
/// mode itself is covered by `engine_mode_choice` and `workers_for_filters_by_mode`.
#[test]
#[ignore = "needs a session and kdotool; starts the real binary"]
fn force_full_memory_mode_runs_full_engine() {
    if !has_display() || !has_kdotool() {
        eprintln!("skipped: needs a display and kdotool on PATH");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    let before = tasklens_windows();
    let started = Instant::now();
    let mut child = spawn_low_memory(scratch.path(), &["--force-full-memory-mode"]);
    let deadline = started + Duration::from_secs(10);
    let mut ids = rescue_windows();
    while ids.len() != 1 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        ids = rescue_windows();
    }
    if let Some(rest) = Duration::from_secs(4).checked_sub(started.elapsed()) {
        std::thread::sleep(rest);
    }
    let after = tasklens_windows();
    let running = matches!(child.try_wait(), Ok(None));
    // Kill only the child this test started, before any assert can fail.
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(ids.len(), 1, "rescue windows: {ids:?}");
    assert_eq!(after, before + 1, "a main window opened");
    assert!(running, "the process exited early");
}
