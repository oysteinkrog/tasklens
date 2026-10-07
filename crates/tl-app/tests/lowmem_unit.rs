//! Tests for the `tasklens-lowmem.service` unit (E32-002), the persist wrapper that installs or
//! removes it (E32-004), and the `tasklens --watch-memory` arm, which execs `tasklens-watch`
//! (tasklens-m1f.19).

#[path = "../src/atomic.rs"]
mod atomic;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/lowmem_unit.rs"]
mod lowmem_unit;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/notify.rs"]
mod notify;
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

// E32-004: wrap_persist. The install and remove functions are fakes; nothing talks to systemd.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lowmem_unit::{Installed, LowmemUnitError, PersistFn, UnitOps};
use tl_model::settings::{Settings, UiState};

const WAIT: Duration = Duration::from_secs(5);

/// Counts calls to the fake install, remove and notification, and records what `inner` got.
#[derive(Default)]
struct Fake {
    installs: AtomicUsize,
    removes: AtomicUsize,
    notifications: AtomicUsize,
    saved: Mutex<Vec<(Settings, UiState)>>,
    /// While true, the fake install waits.
    hold: Mutex<bool>,
    released: Condvar,
}

impl Fake {
    fn installs(&self) -> usize {
        self.installs.load(Ordering::SeqCst)
    }

    fn removes(&self) -> usize {
        self.removes.load(Ordering::SeqCst)
    }

    fn release(&self) {
        *self.hold.lock().unwrap() = false;
        self.released.notify_all();
    }

    fn last_saved(&self) -> (Settings, UiState) {
        self.saved
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("inner was called")
    }

    /// Waits until `done` holds, or fails after 5 s.
    fn wait_for(&self, done: impl Fn(&Fake) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < WAIT, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Builds the wrapper over a recording `inner`. `install` gives the fake install's result.
fn wrap(
    initial: bool,
    file_exists: bool,
    install: fn() -> Result<Installed, LowmemUnitError>,
) -> (Arc<Fake>, PersistFn) {
    let fake = Arc::new(Fake::default());
    let (f1, f2, f3, f4) = (fake.clone(), fake.clone(), fake.clone(), fake.clone());
    let ops = UnitOps {
        install: Box::new(move || {
            let mut hold = f1.hold.lock().unwrap();
            while *hold {
                hold = f1.released.wait(hold).unwrap();
            }
            f1.installs.fetch_add(1, Ordering::SeqCst);
            install()
        }),
        remove: Box::new(move || {
            f2.removes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
        unit_file_exists: Box::new(move || file_exists),
        notify_failure: Box::new(move || {
            f3.notifications.fetch_add(1, Ordering::SeqCst);
        }),
    };
    let inner: PersistFn = Box::new(move |settings, state| {
        f4.saved
            .lock()
            .unwrap()
            .push((settings.clone(), state.clone()));
    });
    (fake, lowmem_unit::wrap_persist_with(inner, initial, ops))
}

fn installed_ok() -> Result<Installed, LowmemUnitError> {
    Ok(Installed {
        oom_score_adjust: -900,
    })
}

fn installed_fallback() -> Result<Installed, LowmemUnitError> {
    Ok(Installed {
        oom_score_adjust: 0,
    })
}

fn install_fails() -> Result<Installed, LowmemUnitError> {
    Err(LowmemUnitError::Bus("fake failure".into()))
}

fn with_lowmem(enabled: bool) -> Settings {
    let mut s = Settings::default();
    s.lowmem.enabled = enabled;
    s
}

/// Waits until the worker has saved again after the install or remove.
fn wait_idle(fake: &Fake, saves: usize) {
    fake.wait_for(|f| f.saved.lock().unwrap().len() >= saves);
    // Give a wrongly started second run time to show up.
    std::thread::sleep(Duration::from_millis(50));
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn wrap_persist_installs_on_enable() {
    let (fake, persist) = wrap(false, false, installed_ok);
    persist(&with_lowmem(true), &UiState::default());
    // One save from the callback, one from the worker after the install.
    wait_idle(&fake, 2);
    assert_eq!(fake.installs(), 1);
    assert_eq!(fake.removes(), 0);
    assert!(fake.last_saved().0.lowmem.enabled);
    // The callback's own save is the first one, made before the install ended.
    assert!(fake.saved.lock().unwrap()[0].0.lowmem.enabled);
}

#[test]
fn wrap_persist_no_op_on_same_value() {
    let (fake, persist) = wrap(true, true, installed_ok);
    persist(&with_lowmem(true), &UiState::default());
    persist(&with_lowmem(true), &UiState::default());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fake.installs(), 0);
    assert_eq!(fake.removes(), 0);
    assert_eq!(fake.saved.lock().unwrap().len(), 2);
}

#[test]
fn wrap_persist_removes_on_disable() {
    let (fake, persist) = wrap(true, true, installed_ok);
    persist(&with_lowmem(false), &UiState::default());
    wait_idle(&fake, 2);
    assert_eq!(fake.removes(), 1);
    assert_eq!(fake.installs(), 0);
    assert!(!fake.last_saved().0.lowmem.enabled);
}

#[test]
fn wrap_persist_coalesces_while_busy() {
    let (fake, persist) = wrap(false, false, installed_ok);
    *fake.hold.lock().unwrap() = true;
    persist(&with_lowmem(true), &UiState::default());
    persist(&with_lowmem(false), &UiState::default());
    persist(&with_lowmem(true), &UiState::default());
    fake.release();
    wait_idle(&fake, 4);
    assert_eq!(fake.installs(), 1);
    assert_eq!(fake.removes(), 0);
    assert_eq!(fake.saved.lock().unwrap().len(), 4);
}

#[test]
fn wrap_persist_inner_called_on_failure() {
    let out = Captured::default();
    let writer = out.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .without_time()
        .finish();
    let fake = tracing::subscriber::with_default(subscriber, || {
        let (fake, persist) = wrap(false, false, install_fails);
        persist(&with_lowmem(true), &UiState::default());
        wait_idle(&fake, 2);
        fake
    });
    assert_eq!(fake.installs(), 1);
    assert!(!fake.saved.lock().unwrap().is_empty());
    let logs = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let warn = logs
        .lines()
        .find(|l| l.contains("WARN"))
        .unwrap_or_else(|| panic!("no warn line:\n{logs}"));
    assert!(warn.contains("tasklens::engine"), "{warn}");
    assert!(warn.contains("fake failure"), "{warn}");
}

#[test]
fn failure_reverts_setting_and_notifies() {
    let (fake, persist) = wrap(false, false, install_fails);
    persist(&with_lowmem(true), &UiState::default());
    wait_idle(&fake, 2);
    fake.wait_for(|f| f.notifications.load(Ordering::SeqCst) > 0);
    assert!(!fake.last_saved().0.lowmem.enabled);
    assert_eq!(fake.notifications.load(Ordering::SeqCst), 1);
    // tl-ui still holds `true`: later saves stay reverted and nothing is retried.
    persist(&with_lowmem(true), &UiState::default());
    std::thread::sleep(Duration::from_millis(50));
    assert!(!fake.last_saved().0.lowmem.enabled);
    assert_eq!(fake.installs(), 1);
    // Turning the switch off and on again retries.
    persist(&with_lowmem(false), &UiState::default());
    persist(&with_lowmem(true), &UiState::default());
    fake.wait_for(|f| f.installs() == 2);
}

#[test]
fn oom_score_adjust_written_to_state() {
    let (fake, persist) = wrap(false, false, installed_fallback);
    persist(&with_lowmem(true), &UiState::default());
    wait_idle(&fake, 2);
    let state = fake.last_saved().1;
    assert_eq!(state.lowmem.oom_score_adjust, Some(0));
    let text = toml::to_string(&state).unwrap();
    assert!(text.contains("[lowmem]\noom_score_adjust = 0\n"), "{text}");
    // tl-ui's copy of the state does not have the value; later saves keep it.
    persist(&with_lowmem(true), &UiState::default());
    assert_eq!(fake.last_saved().1.lowmem.oom_score_adjust, Some(0));
}

#[test]
fn startup_reinstalls_missing_unit() {
    let (fake, _persist) = wrap(true, false, installed_ok);
    fake.wait_for(|f| f.installs() == 1);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fake.installs(), 1);
    // The setting is off and a file exists: nothing is done.
    let (fake, _persist) = wrap(false, true, installed_ok);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fake.installs() + fake.removes(), 0);
    // The setting is on and the file exists: nothing is done.
    let (fake, _persist) = wrap(true, true, installed_ok);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fake.installs() + fake.removes(), 0);
}
