//! Tests for the panic hook and crash report files (E33-005).

#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/crash.rs"]
mod crash;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/logging.rs"]
mod logging;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/paths.rs"]
mod paths;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tracing_subscriber::fmt::MakeWriter;

use crash::{KEEP_REPORTS, ReportFields, format_report, install_hook, prune};
use logging::RecentLines;

/// The panic hook is global, so tests that set it run one at a time.
static HOOK_LOCK: Mutex<()> = Mutex::new(());

/// Runs `install`, panics with `message` on a thread named `thread`, then restores the hook
/// that was set before.
fn panic_with_hook(install: impl FnOnce(), thread: &str, message: &'static str) {
    let _guard = HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let saved = std::panic::take_hook();
    install();
    let joined = std::thread::Builder::new()
        .name(thread.to_owned())
        .spawn(move || std::panic::panic_any(message))
        .expect("spawn")
        .join();
    let _ = std::panic::take_hook();
    std::panic::set_hook(saved);
    assert!(joined.is_err(), "the thread must still panic");
}

fn noop() {}

fn reports(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read crash dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
        .collect();
    paths.sort();
    paths
}

fn only_report(dir: &Path) -> String {
    let found = reports(dir);
    assert_eq!(found.len(), 1, "{found:?}");
    std::fs::read_to_string(&found[0]).expect("read report")
}

fn recent_with(lines: &[&str]) -> Arc<OnceLock<RecentLines>> {
    let ring = RecentLines::default();
    for line in lines {
        ring.make_writer()
            .write_all(format!("{line}\n").as_bytes())
            .expect("write ring");
    }
    let recent = Arc::new(OnceLock::new());
    let _ = recent.set(ring);
    recent
}

/// Returns the lines of the section that starts with `header`, up to the next empty line.
fn section<'a>(text: &'a str, header: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|line| *line != header)
        .skip(1)
        .take_while(|line| !line.is_empty())
        .collect()
}

#[test]
fn report_fields_in_order() {
    let fields = ReportFields {
        version: "1.2.3",
        git_sha: "abcdef123456",
        thread: "worker",
        message: "boom",
        location: "src/x.rs:1:2",
        backtrace: "frame 0\nframe 1\n",
    };
    let recent = vec!["one".to_owned(), "two".to_owned()];
    let text = format_report(&fields, &recent);
    let expected = "tasklens 1.2.3 (abcdef123456)\n\
                    thread: worker\n\
                    message: boom\n\
                    location: src/x.rs:1:2\n\
                    \n\
                    backtrace:\n\
                    frame 0\n\
                    frame 1\n\
                    \n\
                    last log lines:\n\
                    one\n\
                    two\n";
    assert_eq!(text, expected);
}

#[test]
fn report_file_written() {
    let dir = tempfile::tempdir().expect("temp dir");
    let crash_dir = dir.path().join("state/crash");
    let recent = recent_with(&["first", "second", "third"]);
    let hook_dir = crash_dir.clone();
    panic_with_hook(|| install_hook(hook_dir, recent, noop), "worker", "boom");

    let found = reports(&crash_dir);
    assert_eq!(found.len(), 1, "{found:?}");
    let name = found[0]
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    let time = name
        .strip_prefix("crash-")
        .and_then(|rest| rest.strip_suffix(".txt"))
        .expect("crash-<t>.txt");
    assert!(time.parse::<u64>().is_ok(), "{name}");

    let text = std::fs::read_to_string(&found[0]).expect("read report");
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("tasklens "), "{text}");
    assert_eq!(lines[1], "thread: worker");
    assert_eq!(lines[2], "message: boom");
    assert!(lines[3].starts_with("location: "), "{text}");
    assert!(lines[3].contains("crash_report.rs:"), "{text}");
    assert!(lines.contains(&"backtrace:"));
    assert!(!section(&text, "backtrace:").is_empty());

    let logged = section(&text, "last log lines:");
    assert_eq!(logged.len(), 3, "{logged:?}");
    assert!(logged[0].ends_with("first"));
    assert!(logged[1].ends_with("second"));
    assert!(logged[2].ends_with("third"));

    let mode = |path: &Path| {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode(&found[0]), 0o600);
    assert_eq!(mode(&crash_dir), 0o700);
}

#[test]
fn hook_before_logging_has_no_log_lines() {
    let dir = tempfile::tempdir().expect("temp dir");
    let crash_dir = dir.path().join("crash");
    let hook_dir = crash_dir.clone();
    panic_with_hook(
        || install_hook(hook_dir, Arc::new(OnceLock::new()), noop),
        "worker",
        "early",
    );
    let text = only_report(&crash_dir);
    assert!(text.ends_with("last log lines:\n"), "{text}");
}

#[test]
fn same_second_suffix() {
    let dir = tempfile::tempdir().expect("temp dir");
    let crash_dir = dir.path().to_path_buf();
    let written: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let hook_written = Arc::clone(&written);
    let hook_dir = crash_dir.clone();
    panic_with_hook(
        move || {
            std::panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
                let mut out = hook_written.lock().unwrap_or_else(PoisonError::into_inner);
                for _ in 0..2 {
                    out.push(
                        crash::write_report(&hook_dir, info, &[], 1_700_000_000).expect("write"),
                    );
                }
            }));
        },
        "worker",
        "twice",
    );
    let written = written.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        *written,
        vec![
            crash_dir.join("crash-1700000000.txt"),
            crash_dir.join("crash-1700000000-1.txt"),
        ]
    );
}

fn make_reports(dir: &Path, count: u64) {
    for i in 0..count {
        let t = 1_700_000_000 + i * 10;
        std::fs::write(dir.join(format!("crash-{t}.txt")), b"report").expect("write report");
        std::fs::write(dir.join(format!("crash-{t}.seen")), b"").expect("write marker");
    }
}

#[test]
fn prune_keeps_newest_20() {
    let dir = tempfile::tempdir().expect("temp dir");
    make_reports(dir.path(), 25);
    std::fs::write(dir.path().join("notes.txt"), b"other").expect("write");
    prune(dir.path()).expect("prune");

    let kept: Vec<PathBuf> = reports(dir.path())
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name != "notes.txt"))
        .collect();
    assert_eq!(kept.len(), KEEP_REPORTS);
    let expected: Vec<PathBuf> = (5..25)
        .map(|i| {
            dir.path()
                .join(format!("crash-{}.txt", 1_700_000_000 + i * 10))
        })
        .collect();
    assert_eq!(kept, expected);
    assert!(dir.path().join("notes.txt").exists());
}

#[test]
fn prune_removes_seen_markers() {
    let dir = tempfile::tempdir().expect("temp dir");
    make_reports(dir.path(), 25);
    prune(dir.path()).expect("prune");
    for i in 0..25u64 {
        let marker = dir
            .path()
            .join(format!("crash-{}.seen", 1_700_000_000 + i * 10));
        assert_eq!(marker.exists(), i >= 5, "{}", marker.display());
    }
}

#[test]
fn prune_orders_by_number_not_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    // 9 sorts after 10 as text, but is older.
    std::fs::write(dir.path().join("crash-9.txt"), b"").expect("write");
    for t in 10..30u64 {
        std::fs::write(dir.path().join(format!("crash-{t}.txt")), b"").expect("write");
    }
    std::fs::write(dir.path().join("crash-29-1.txt"), b"").expect("write");
    prune(dir.path()).expect("prune");
    assert!(!dir.path().join("crash-9.txt").exists());
    assert!(!dir.path().join("crash-10.txt").exists());
    assert!(dir.path().join("crash-29-1.txt").exists());
    assert_eq!(reports(dir.path()).len(), KEEP_REPORTS);
}

#[test]
fn no_cmdline_or_env_in_report() {
    let dir = tempfile::tempdir().expect("temp dir");
    let crash_dir = dir.path().join("crash");
    let hook_dir = crash_dir.clone();
    panic_with_hook(
        || install_hook(hook_dir, recent_with(&["a line"]), noop),
        "worker",
        "private",
    );
    let text = only_report(&crash_dir);

    let args: Vec<String> = std::env::args().collect();
    let cmdline = args.join(" ");
    assert!(!text.contains(&cmdline), "{text}");
    assert!(!text.contains(&args[0]), "{text}");
    assert!(!text.contains("HOME="), "{text}");
}

static PREVIOUS_RAN: AtomicBool = AtomicBool::new(false);

#[test]
fn unwritable_dir_falls_through() {
    let dir = tempfile::tempdir().expect("temp dir");
    let read_only = dir.path().join("ro");
    std::fs::create_dir(&read_only).expect("create");
    std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let crash_dir = read_only.join("crash");
    let hook_dir = crash_dir.clone();

    PREVIOUS_RAN.store(false, Ordering::SeqCst);
    panic_with_hook(
        || {
            std::panic::set_hook(Box::new(|_| PREVIOUS_RAN.store(true, Ordering::SeqCst)));
            install_hook(hook_dir, recent_with(&["a line"]), noop);
        },
        "worker",
        "no room",
    );
    assert!(
        PREVIOUS_RAN.load(Ordering::SeqCst),
        "the previous hook must run"
    );
    // Root can write anyway, so only check the folder when the mode was enforced.
    if std::fs::create_dir(read_only.join("probe")).is_err() {
        assert!(!crash_dir.exists());
    }
    std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o700)).expect("chmod");
}
