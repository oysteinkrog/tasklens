//! Tests for loading and saving `settings.toml` and `state.toml` (E33-006, E33-008, ADR-011).

#[path = "../src/atomic.rs"]
mod atomic;

#[path = "../src/autostart.rs"]
mod autostart;

#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/import_ts.rs"]
mod import_ts;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/migrations.rs"]
mod migrations;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/paths.rs"]
mod paths;
#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/settings_store.rs"]
mod settings_store;

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use paths::Paths;
use settings_store::{LoadNotice, RunHistoryFile, Store, load_settings, load_state};
use tempfile::TempDir;
use tl_model::settings::{Settings, UiState};
use tl_ui::{ImportSummary, PersistTheme, RunHistoryEntry, RunHistoryStore};

const NOW: u64 = 1_759_500_000;

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
    let logs = String::from_utf8(out.0.lock().expect("log buffer").clone()).expect("utf-8 logs");
    (result, logs)
}

fn write(dir: &TempDir, name: &str, text: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, text).expect("write test file");
    path
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
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
        assert_eq!(names(dir.path()), std::slice::from_ref(&kept_as));
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

/// Paths with every XDG root under `dir`.
fn paths_in(dir: &TempDir) -> Paths {
    let root = dir.path().to_path_buf();
    Paths::from_vars(|name| match name {
        "HOME" => Some(OsString::from(&root)),
        "XDG_CONFIG_HOME" => Some(root.join("config").into_os_string()),
        "XDG_STATE_HOME" => Some(root.join("state").into_os_string()),
        "XDG_CACHE_HOME" => Some(root.join("cache").into_os_string()),
        _ => None,
    })
    .expect("test paths")
}

fn defaults() -> (Settings, UiState) {
    (Settings::default(), UiState::default())
}

/// Settings that differ from the defaults by `interval_ms`.
fn settings_with(interval_ms: u32) -> Settings {
    let mut s = Settings::default();
    s.processes.interval_ms = interval_ms;
    s
}

/// The file's modification count stand-in: its inode changes on every atomic write.
fn inode(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.ino())
}

const SETTLE: Duration = Duration::from_millis(800);

#[test]
fn debounce_writes_once() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let store = Store::new(&paths, false, false, defaults());
    let persist = store.persist_callback();
    let state = UiState::default();

    let mut inodes = Vec::new();
    for ms in [1500, 2000, 2500] {
        persist(&settings_with(ms), &state);
        std::thread::sleep(Duration::from_millis(100));
        inodes.push(inode(&paths.settings_file()));
    }
    // Nothing is written while calls keep coming.
    assert_eq!(inodes, [None, None, None]);

    std::thread::sleep(SETTLE);
    let first = inode(&paths.settings_file()).expect("settings.toml written");
    let saved = load_settings(&paths.settings_file(), NOW);
    assert_eq!(saved.value.processes.interval_ms, 2500);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        inode(&paths.settings_file()),
        Some(first),
        "written more than once"
    );
    assert!(!paths.state_file().exists(), "unchanged state.toml written");
}

#[test]
fn only_changed_file_written() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let store = Store::new(&paths, false, false, defaults());
    let mut state = UiState::default();
    state.window.width = 1500;
    store.persist_callback()(&Settings::default(), &state);
    std::thread::sleep(SETTLE);

    assert!(
        !paths.settings_file().exists(),
        "unchanged settings.toml written"
    );
    assert_eq!(
        load_state(&paths.state_file(), NOW).value.window.width,
        1500
    );
}

#[test]
fn read_only_never_written() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    let text = "schema_version = 2\n[processes]\ninterval_ms = 2000\n";
    std::fs::write(paths.settings_file(), text).unwrap();
    let loaded = load_settings(&paths.settings_file(), NOW);
    assert!(loaded.read_only);

    let store = Store::new(&paths, true, false, (loaded.value, UiState::default()));
    let persist = store.persist_callback();
    persist(&settings_with(3000), &UiState::default());
    std::thread::sleep(SETTLE);
    persist(&settings_with(4000), &UiState::default());
    store.flush();
    assert!(store.write_settings_now(&settings_with(5000)).is_err());

    assert_eq!(
        std::fs::read_to_string(paths.settings_file()).unwrap(),
        text
    );
}

#[test]
fn flush_writes_now() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let store = Store::new(&paths, false, false, defaults());
    let persist = store.persist_callback();
    let mut state = UiState::default();
    state.window.width = 1500;
    persist(&settings_with(2000), &state);
    store.flush();

    assert_eq!(
        load_settings(&paths.settings_file(), NOW)
            .value
            .processes
            .interval_ms,
        2000
    );
    assert_eq!(
        load_state(&paths.state_file(), NOW).value.window.width,
        1500
    );

    // Calls after flush are ignored.
    persist(&settings_with(3000), &state);
    store.flush();
    assert_eq!(
        load_settings(&paths.settings_file(), NOW)
            .value
            .processes
            .interval_ms,
        2000
    );
}

#[test]
fn write_error_logged_and_retried() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    // A file where the config dir should be makes every settings write fail.
    std::fs::write(dir.path().join("config"), "").unwrap();

    let (store, logs) = capture_logs(|| {
        let store = Store::new(&paths, false, false, defaults());
        store.persist_callback()(&settings_with(2000), &UiState::default());
        std::thread::sleep(SETTLE);
        store
    });
    assert!(logs.contains("WARN"), "{logs}");
    assert!(logs.contains("Could not save settings.toml"), "{logs}");
    assert!(!logs.contains("2000"), "setting value logged: {logs}");

    // The failed value is kept, so flush retries it once the dir can be made.
    std::fs::remove_file(dir.path().join("config")).unwrap();
    store.flush();
    assert_eq!(
        load_settings(&paths.settings_file(), NOW)
            .value
            .processes
            .interval_ms,
        2000
    );
}

#[test]
fn crash_between_write_and_rename_leaves_intact_file() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let store = Store::new(&paths, false, false, defaults());
    store.persist_callback()(&settings_with(2000), &UiState::default());
    store.flush();

    // A crash after the temp write but before the rename leaves this file behind.
    let tmp = paths
        .config_dir
        .join(format!("settings.toml.tmp-{}", std::process::id()));
    std::fs::write(&tmp, "schema_version = 1\n[processes]\ninterval_").unwrap();

    let loaded = load_settings(&paths.settings_file(), NOW);
    assert_eq!(loaded.value.processes.interval_ms, 2000);
    assert!(loaded.notices.is_empty());

    // The next save replaces the leftover and the target holds the new value.
    let store = Store::new(&paths, false, false, (loaded.value, UiState::default()));
    store.persist_callback()(&settings_with(3000), &UiState::default());
    store.flush();
    assert_eq!(
        load_settings(&paths.settings_file(), NOW)
            .value
            .processes
            .interval_ms,
        3000
    );
    assert!(!tmp.exists());
}

#[test]
fn unknown_keys_absent_after_save() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(
        paths.settings_file(),
        "colour = 1\n[general]\nshade = 3\n[processes]\ninterval_ms = 2000\n",
    )
    .unwrap();
    let loaded = load_settings(&paths.settings_file(), NOW);
    let store = Store::new(&paths, false, false, (loaded.value, UiState::default()));
    store.persist_callback()(&settings_with(3000), &UiState::default());
    store.flush();

    let text = std::fs::read_to_string(paths.settings_file()).unwrap();
    assert!(text.contains("interval_ms = 3000"), "{text}");
    assert!(!text.contains("colour"), "{text}");
    assert!(!text.contains("shade"), "{text}");
}

#[test]
fn saved_file_mode_and_header() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let store = Store::new(&paths, false, false, defaults());
    let mut state = UiState::default();
    state.window.width = 1500;
    store.persist_callback()(&settings_with(2000), &state);
    store.flush();

    for path in [paths.settings_file(), paths.state_file()] {
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", path.display());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().next(), Some("schema_version = 1"), "{text}");
    }
    let dir_mode = std::fs::metadata(&paths.config_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
}

#[test]
fn themes_saved_atomic() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let store = Store::new(&paths, false, false, defaults());
    let themes: Vec<_> = tl_ui::theme::builtin::load()
        .iter()
        .take(2)
        .cloned()
        .collect();
    assert_eq!(themes.len(), 2);
    let want: Vec<String> = themes.iter().map(|t| t.name.to_string()).collect();

    store.persist_themes_callback()(PersistTheme::Save(themes));
    std::thread::sleep(SETTLE);
    let first = inode(&paths.themes_file()).expect("themes.json written");
    store.flush();
    assert_eq!(
        inode(&paths.themes_file()),
        Some(first),
        "written more than once"
    );

    let mode = std::fs::metadata(paths.themes_file())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let text = std::fs::read_to_string(paths.themes_file()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let saved: Vec<&str> = json["themes"]
        .as_array()
        .expect("themes array")
        .iter()
        .map(|t| t["name"].as_str().expect("theme name"))
        .collect();
    assert_eq!(saved, want);
    assert_eq!(names(&paths.config_dir), ["themes.json"]);
}

#[test]
fn themes_rename_corrupt() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(paths.themes_file(), "{not json").unwrap();
    let store = Store::new(&paths, false, false, defaults());

    store.persist_themes_callback()(PersistTheme::RenameCorrupt {
        unix_seconds: 1_700_000_000,
    });
    store.flush();

    assert_eq!(names(&paths.config_dir), ["themes.json.corrupt-1700000000"]);
    assert_eq!(
        std::fs::read_to_string(paths.config_dir.join("themes.json.corrupt-1700000000")).unwrap(),
        "{not json"
    );
}

#[test]
fn run_history_round_trip() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let file = RunHistoryFile::new(&paths);
    assert!(file.load().is_empty(), "a missing file loads as empty");

    let entries = vec![
        RunHistoryEntry {
            command: "konsole".to_owned(),
            executed_at: "2026-10-07T10:00:00Z".to_owned(),
            elevated: false,
        },
        RunHistoryEntry {
            command: "systemctl restart sshd".to_owned(),
            executed_at: "2026-10-07T10:05:00Z".to_owned(),
            elevated: true,
        },
    ];
    file.save(&entries).unwrap();
    assert_eq!(file.load(), entries);

    let mode = std::fs::metadata(paths.run_history_file())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(paths.run_history_file()).unwrap()).unwrap();
    assert!(json.is_array());

    std::fs::write(paths.run_history_file(), "[{").unwrap();
    assert!(file.load().is_empty(), "a bad file loads as empty");
}

/// Default settings with the given `[startup]` values.
fn startup_settings(start_at_login: bool, start_minimized: bool) -> Settings {
    let mut s = Settings::default();
    s.startup.start_at_login = start_at_login;
    s.startup.start_minimized = start_minimized;
    s
}

#[test]
fn autostart_follows_setting() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let entry = paths.autostart_file();
    assert!(entry.starts_with(dir.path()));
    let store = Store::new(&paths, false, false, defaults());
    let persist = store.persist_callback();
    let state = UiState::default();

    // On.
    persist(&startup_settings(true, false), &state);
    std::thread::sleep(SETTLE);
    let text = std::fs::read_to_string(&entry).expect("autostart entry written");
    assert!(text.contains("\nExec=tasklens\n"), "{text}");
    assert!(text.contains("\nX-KDE-autostart-phase=2\n"), "{text}");

    // Both on.
    persist(&startup_settings(true, true), &state);
    std::thread::sleep(SETTLE);
    let text = std::fs::read_to_string(&entry).unwrap();
    assert!(
        text.contains("\nExec=tasklens --start-minimized\n"),
        "{text}"
    );

    // Another setting changes: the entry is not rewritten.
    let first = inode(&entry).expect("entry exists");
    let mut other = startup_settings(true, true);
    other.processes.interval_ms = 2000;
    persist(&other, &state);
    std::thread::sleep(SETTLE);
    assert_eq!(
        load_settings(&paths.settings_file(), NOW)
            .value
            .processes
            .interval_ms,
        2000
    );
    assert_eq!(inode(&entry), Some(first), "unchanged entry rewritten");

    // Off.
    persist(&startup_settings(false, true), &state);
    store.flush();
    assert!(!entry.exists(), "autostart entry not removed");
}

#[test]
fn autostart_error_logged_settings_saved() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    // A file where the autostart dir should be makes every entry write fail.
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config").join("autostart"), "").unwrap();

    let (store, logs) = capture_logs(|| {
        let store = Store::new(&paths, false, false, defaults());
        store.persist_callback()(&startup_settings(true, false), &UiState::default());
        std::thread::sleep(SETTLE);
        store
    });
    assert!(logs.contains("WARN"), "{logs}");
    assert!(
        logs.contains("Could not update the autostart entry"),
        "{logs}"
    );
    assert!(
        load_settings(&paths.settings_file(), NOW)
            .value
            .startup
            .start_at_login
    );
    store.flush();
}

#[test]
fn autostart_read_only_settings_still_applied() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let store = Store::new(&paths, true, false, defaults());
    store.persist_callback()(&startup_settings(true, false), &UiState::default());
    store.flush();
    assert!(!paths.settings_file().exists());
    assert!(paths.autostart_file().exists());
}

/// A TaskSlinger fixture: two booleans, an integer, a window key and one unknown key.
const TS_INI: &str = "[Settings]\r\nAnimationsEnabled=false\r\nCloseToTray=true\r\n\
                      RefreshIntervalMs=1000\r\nUnknownKey=5\r\n[Window]\r\nWidth=1600\r\n";

/// Runs the import callback on `bytes`, then persists the result the way tl-ui's
/// `TlSettings::update` does, and returns what landed on disk.
fn import_and_persist(bytes: &[u8]) -> (ImportSummary, Settings, UiState) {
    let dir = TempDir::new().expect("tempdir");
    let paths = paths_in(&dir);
    let ini = dir.path().join("UserSettings.ini");
    std::fs::write(&ini, bytes).expect("write fixture");
    let store = Store::new(&paths, false, false, defaults());
    let imported =
        import_ts::import_file(&ini, &Settings::default(), &UiState::default()).expect("import");
    store.persist_callback()(&imported.settings, &imported.state);
    store.flush();
    (
        imported.summary,
        load_settings(&paths.settings_file(), NOW).value,
        load_state(&paths.state_file(), NOW).value,
    )
}

#[test]
fn taskslinger_import_persisted() {
    let (summary, settings, state) = import_and_persist(TS_INI.as_bytes());
    assert_eq!(
        summary,
        ImportSummary {
            settings: 4,
            themes: 0
        }
    );
    assert!(!settings.motion.animations);
    assert!(settings.tray.close_to_tray);
    assert_eq!(settings.processes.interval_ms, 1000);
    assert_eq!(state.window.width, 1600);
}

#[test]
fn taskslinger_import_utf16_matches_utf8() {
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in TS_INI.encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    assert_eq!(
        import_and_persist(&utf16),
        import_and_persist(TS_INI.as_bytes())
    );
}

#[test]
fn taskslinger_import_bad_file_changes_nothing() {
    let dir = TempDir::new().unwrap();
    let paths = paths_in(&dir);
    let store = Store::new(&paths, false, false, defaults());
    let (settings, state) = defaults();

    let missing = dir.path().join("missing.ini");
    let err = import_ts::import_file(&missing, &settings, &state).expect_err("missing");
    assert!(
        err.starts_with("Could not read the TaskSlinger settings file"),
        "{err}"
    );

    let big = dir.path().join("big.ini");
    let mut text = TS_INI.to_owned();
    text.push_str(&"a".repeat(import_ts::MAX_INI_BYTES));
    std::fs::write(&big, text).unwrap();
    let err = import_ts::import_file(&big, &settings, &state).expect_err("too large");
    assert_eq!(err, import_ts::TOO_LARGE);

    // An error gives the UI nothing to apply, so the store writes nothing.
    store.flush();
    assert!(!paths.settings_file().exists());
    assert!(!paths.state_file().exists());
}
