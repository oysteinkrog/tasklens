//! Tests for the `tasklens-lowmem.service` unit (E32-002) and the `tasklens --watch-memory`
//! arm, which execs `tasklens-watch` (tasklens-m1f.19).

#[path = "../src/atomic.rs"]
mod atomic;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/lowmem_unit.rs"]
mod lowmem_unit;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/paths.rs"]
mod paths;

// E32-002: the lowmem unit. Every automatic test works in a scratch folder and never talks to
// the user's systemd manager.

/// The environment `unit_path_from` sees: `HOME=/home/u` plus `xdg` as `XDG_CONFIG_HOME`.
fn env_with(xdg: Option<&str>) -> impl Fn(&str) -> Option<std::ffi::OsString> {
    move |name| match name {
        "HOME" => Some("/home/u".into()),
        "XDG_CONFIG_HOME" => xdg.map(Into::into),
        _ => None,
    }
}

#[test]
fn render_unit_replaces_placeholders() {
    let text = lowmem_unit::render_unit(std::path::Path::new("/usr/bin/tasklens-watch"), -900);
    assert!(
        text.lines()
            .any(|l| l == "ExecStart=/usr/bin/tasklens-watch")
    );
    assert!(text.lines().any(|l| l == "OOMScoreAdjust=-900"));
    assert!(!text.contains('@'), "placeholder left in:\n{text}");
    let fallback = lowmem_unit::render_unit(std::path::Path::new("/usr/bin/tasklens-watch"), 0);
    assert!(fallback.lines().any(|l| l == "OOMScoreAdjust=0"));
}

#[test]
fn render_unit_quotes_spaces() {
    let text = lowmem_unit::render_unit(
        std::path::Path::new("/home/u/my builds/tasklens-watch"),
        -900,
    );
    assert!(
        text.lines()
            .any(|l| l == r#"ExecStart="/home/u/my builds/tasklens-watch""#),
        "{text}"
    );
    // systemd expands % and $ in ExecStart, so both are doubled.
    let odd = lowmem_unit::render_unit(std::path::Path::new("/opt/a%b$c/tasklens-watch"), -900);
    assert!(
        odd.lines()
            .any(|l| l == "ExecStart=/opt/a%%b$$c/tasklens-watch")
    );
    let quote = lowmem_unit::render_unit(std::path::Path::new("/opt/a \"b/tasklens-watch"), -900);
    assert!(
        quote
            .lines()
            .any(|l| l == r#"ExecStart="/opt/a \"b/tasklens-watch""#),
        "{quote}"
    );
}

#[test]
fn unit_path_xdg_fallback() {
    let expected = std::path::PathBuf::from("/home/u/.config/systemd/user/tasklens-lowmem.service");
    for xdg in [None, Some(""), Some("relative/path")] {
        assert_eq!(
            lowmem_unit::unit_path_from(env_with(xdg)).unwrap(),
            expected,
            "{xdg:?}"
        );
    }
    assert_eq!(
        lowmem_unit::unit_path_from(env_with(Some("/cfg"))).unwrap(),
        std::path::PathBuf::from("/cfg/systemd/user/tasklens-lowmem.service")
    );
    assert!(lowmem_unit::unit_path_from(|_| None).is_err());
}

#[test]
fn unit_file_write_and_remove_in_scratch_dir() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch
        .path()
        .join("systemd/user")
        .join(lowmem_unit::UNIT_NAME);
    let exec = std::path::Path::new("/usr/bin/tasklens-watch");
    lowmem_unit::write_unit_file(&path, exec, -900).unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o644);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&scratch.path().join("systemd")), 0o700);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        lowmem_unit::render_unit(exec, -900)
    );
    // The fallback rewrite replaces the file in place.
    lowmem_unit::write_unit_file(&path, exec, 0).unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("OOMScoreAdjust=0\n")
    );
    lowmem_unit::remove_unit_file(&path).unwrap();
    assert!(!path.exists());
    lowmem_unit::remove_unit_file(&path).unwrap();
}

/// Owner machine only: installs, enables and starts the real unit in the user's own systemd
/// session, then removes it. Run by hand with `--ignored`; it prints the OOMScoreAdjust value
/// the user manager accepted (U-24).
#[test]
#[ignore = "changes the user's real systemd session"]
#[allow(clippy::print_stdout, reason = "prints the U-24 result for the owner")]
fn real_install_enable_remove() {
    let installed = lowmem_unit::install_and_enable().expect("install_and_enable");
    println!(
        "U-24: accepted OOMScoreAdjust={}",
        installed.oom_score_adjust
    );
    let conn = zbus::blocking::Connection::session().unwrap();
    let reply = conn
        .call_method(
            Some("org.freedesktop.systemd1"),
            "/org/freedesktop/systemd1",
            Some("org.freedesktop.systemd1.Manager"),
            "GetUnitFileState",
            &(lowmem_unit::UNIT_NAME,),
        )
        .unwrap();
    let state: String = reply.body().deserialize().unwrap();
    assert_eq!(state, "enabled");

    lowmem_unit::disable_and_remove().expect("first disable_and_remove");
    lowmem_unit::disable_and_remove().expect("second disable_and_remove");
    assert!(!lowmem_unit::unit_path().unwrap().exists());
}

/// With a fake owner of the bus name, `--watch-memory` does not forward to it and keeps running.
/// A live tasklens that already owns the name counts as the owner. The arm execs
/// `tasklens-watch` from the same folder, so build it first: `cargo build -p tl-watch`. The
/// child gets its own `XDG_RUNTIME_DIR`, so a real watcher on this machine does not hold the lock.
#[test]
#[ignore = "needs a session bus and a built tasklens-watch; starts the real binary"]
fn watch_memory_no_single_instance() {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
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
    let runtime_dir = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_tasklens"))
        .arg("--watch-memory")
        .env("XDG_RUNTIME_DIR", runtime_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let exited = child.try_wait();
    // Kill only the child this test started, before any assert can fail.
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(exited.unwrap(), None, "the watcher exited");
}
