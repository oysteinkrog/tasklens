//! Tests for loading `settings.toml` and `state.toml` (E33-006, ADR-011 Load).

#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/migrations.rs"]
mod migrations;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/settings_store.rs"]
mod settings_store;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use settings_store::{LoadNotice, load_settings, load_state};
use tempfile::TempDir;
use tl_model::settings::{Settings, UiState};

const NOW: u64 = 1_759_500_000;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `f` with a subscriber that records every event, and returns the log text.
fn capture_logs<R>(f: impl FnOnce() -> R) -> (R, String) {
    let out = Captured::default();
    let writer = out.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .without_time()
        .finish();
    let result = tracing::subscriber::with_default(subscriber, f);
    let logs = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    (result, logs)
}

fn write(dir: &TempDir, name: &str, text: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn missing_file_defaults_no_write() {
    let dir = TempDir::new().unwrap();

    let s = load_settings(&dir.path().join("settings.toml"), NOW);
    assert_eq!(s.value, Settings::default());
    assert!(!s.read_only);
    assert!(s.notices.is_empty());

    let st = load_state(&dir.path().join("state.toml"), NOW);
    assert_eq!(st.value, UiState::default());
    assert!(!st.read_only);
    assert!(st.notices.is_empty());

    assert!(names(dir.path()).is_empty());
}

#[test]
fn corrupt_file_renamed() {
    for name in ["settings.toml", "state.toml"] {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, name, "[general\nsecret = 1");
        let kept_as = format!("{name}.corrupt-{NOW}");

        let ((read_only, notices), logs) = capture_logs(|| {
            if name == "settings.toml" {
                let l = load_settings(&path, NOW);
                assert_eq!(l.value, Settings::default());
                (l.read_only, l.notices)
            } else {
                let l = load_state(&path, NOW);
                assert_eq!(l.value, UiState::default());
                (l.read_only, l.notices)
            }
        });

        assert!(!read_only);
        assert_eq!(names(dir.path()), [kept_as.clone()]);
        assert_eq!(
            notices,
            [LoadNotice::Corrupt {
                kept_as: kept_as.clone()
            }]
        );
        assert_eq!(
            notices[0].toast_text(),
            format!(
                "Settings file was unreadable and has been reset. The old file was kept as {kept_as}"
            )
        );
        assert!(!logs.contains("secret"), "file contents logged: {logs}");
    }
}

#[test]
fn corrupt_toast_text_exact() {
    let notice = LoadNotice::Corrupt {
        kept_as: "settings.toml.corrupt-1759500000".to_owned(),
    };
    assert_eq!(
        notice.toast_text(),
        "Settings file was unreadable and has been reset. The old file was kept as \
         settings.toml.corrupt-1759500000"
    );
}

#[test]
fn v0_without_version_loaded_as_v1() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "settings.toml", "[processes]\ninterval_ms = 2000\n");
    let s = load_settings(&path, NOW);
    assert_eq!(s.value.processes.interval_ms, 2000);
    assert_eq!(s.value.schema_version, 1);
    assert!(!s.read_only);
    assert!(s.notices.is_empty());

    let path = write(&dir, "state.toml", "[window]\nwidth = 1500\n");
    let st = load_state(&path, NOW);
    assert_eq!(st.value.window.width, 1500);
    assert_eq!(st.value.schema_version, 1);
    assert!(!st.read_only);
    assert!(st.notices.is_empty());
}

#[test]
fn newer_version_read_only() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "settings.toml",
        "schema_version = 2\n[processes]\ninterval_ms = 2000\n[future]\nthing = 1\n",
    );
    let before = std::fs::read(&path).unwrap();
    let s = load_settings(&path, NOW);
    assert!(s.read_only);
    assert_eq!(s.value.processes.interval_ms, 2000);
    assert_eq!(s.notices, [LoadNotice::Newer]);
    assert_eq!(
        s.notices[0].toast_text(),
        "Settings were written by a newer tasklens; changes will not be saved"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);

    // The state file has its own flag.
    let path = write(
        &dir,
        "state.toml",
        "schema_version = 7\n[window]\nwidth = 1500\n",
    );
    let st = load_state(&path, NOW);
    assert!(st.read_only);
    assert_eq!(st.value.window.width, 1500);
    assert_eq!(st.notices, [LoadNotice::Newer]);
    let path = write(&dir, "state2.toml", "[window]\nwidth = 1500\n");
    assert!(!load_state(&path, NOW).read_only);
}

#[test]
fn out_of_range_clamped() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "settings.toml", "[appearance]\nui_scale_pct = 500\n");
    let (s, logs) = capture_logs(|| load_settings(&path, NOW));
    assert_eq!(s.value.appearance.ui_scale_pct, 200);
    assert!(logs.contains("INFO"), "{logs}");
    assert!(logs.contains("appearance.ui_scale_pct"), "{logs}");
    assert!(!logs.contains("500"), "setting value logged: {logs}");

    let path = write(&dir, "state.toml", "[window]\nwidth = 10\n");
    let (st, logs) = capture_logs(|| load_state(&path, NOW));
    assert_eq!(st.value.window.width, 800);
    assert!(logs.contains("window.width"), "{logs}");
}

#[test]
fn bad_section_keeps_others() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "settings.toml",
        "[processes]\ninterval_ms = \"fast\"\n[appearance]\nui_scale_pct = 150\n",
    );
    let (s, logs) = capture_logs(|| load_settings(&path, NOW));
    assert_eq!(s.value.processes, Settings::default().processes);
    assert_eq!(s.value.appearance.ui_scale_pct, 150);
    assert!(s.notices.is_empty());
    assert!(logs.contains("WARN"), "{logs}");
    assert!(logs.contains("processes"), "{logs}");
    assert!(!logs.contains("fast"), "setting value logged: {logs}");

    let path = write(
        &dir,
        "state.toml",
        "[window]\nwidth = \"wide\"\n[performance]\nnav_width = 300\n",
    );
    let st = load_state(&path, NOW);
    assert_eq!(st.value.window, UiState::default().window);
    assert_eq!(st.value.performance.nav_width, 300);
}

#[test]
fn bad_schema_version_type_loads() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "settings.toml",
        "schema_version = \"x\"\n[processes]\ninterval_ms = 2000\n",
    );
    let s = load_settings(&path, NOW);
    assert_eq!(s.value.processes.interval_ms, 2000);
    assert_eq!(s.value.schema_version, 1);
    assert!(!s.read_only);
}

#[test]
fn unknown_keys_ignored() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "settings.toml",
        "colour = 1\n[general]\ncolour = 3\n[processes]\ninterval_ms = 2000\n",
    );
    let s = load_settings(&path, NOW);
    assert_eq!(s.value.processes.interval_ms, 2000);
    assert!(s.notices.is_empty());

    let path = write(&dir, "state.toml", "[window]\ncolour = 3\nwidth = 1500\n");
    let st = load_state(&path, NOW);
    assert_eq!(st.value.window.width, 1500);
    assert!(st.notices.is_empty());
}

#[test]
fn unknown_column_id_dropped_order_kept() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "state.toml",
        "[columns.processes]\n\
         visible = [\"name\", \"bogus\", \"pid\"]\n\
         order = [\"pid\", \"bogus\", \"name\"]\n\
         widths = { name = 200, bogus = 50 }\n\
         [columns.services]\n\
         visible = [\"status\", \"cpu\", \"nope\"]\n",
    );
    let (st, logs) = capture_logs(|| load_state(&path, NOW));
    let c = &st.value.columns;
    assert_eq!(c.processes.visible, ["name", "pid"]);
    assert_eq!(c.processes.order, ["pid", "name"]);
    assert_eq!(c.processes.widths.keys().collect::<Vec<_>>(), ["name"]);
    assert_eq!(c.services.visible, ["status", "cpu"]);
    assert!(logs.contains("columns.processes"), "{logs}");
    assert!(!logs.contains("bogus"), "column id logged: {logs}");
}
