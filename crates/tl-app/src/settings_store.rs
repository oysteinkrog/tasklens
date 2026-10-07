//! Loads and saves `settings.toml` and `state.toml` (ADR-011 Load and Save, UI-SHL-110 step 5).
//!
//! Load: both files go through the same steps: a missing file gives defaults, an unreadable one
//! is renamed and reset, an older one is migrated, a newer one is read-only, a section with a bad
//! value takes its defaults, and out-of-range values are clamped. The notices become toasts after
//! the first frame (UI-SHL-110 step 11).
//!
//! Save: `Store` writes a file 500 ms after the last change and once on exit, through
//! `write_atomic`, on the `tasklens-save` thread. It also writes `themes.json` and
//! `run-history.json`. A read-only file is never written. When `[startup]` changes, it also
//! writes or removes the autostart entry (E33-014).
//!
//! Logs name files, sections and keys, never their values.

use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tl_model::ids::ColumnId;
use tl_model::settings::{ColumnState, SCHEMA_VERSION, Settings, StartupSettings, UiState};
use tl_model::table::{SysColumnId, SysTable};
use tl_ui::theme::user_store::serialize_user_themes;
use tl_ui::{PersistTheme, RunHistoryEntry, RunHistoryStore};
use tracing::{info, warn};

use crate::paths::{FILE_MODE, Paths, ensure_dir};
use crate::{atomic, autostart, migrations};

/// Which of the two files is being loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Settings,
    State,
}

impl FileKind {
    fn label(self) -> &'static str {
        match self {
            FileKind::Settings => "settings.toml",
            FileKind::State => "state.toml",
        }
    }
}

/// A loaded file. `read_only` is set when the file was written by a newer tasklens; the save
/// half (E33-008) never writes such a file.
#[derive(Debug)]
pub struct Loaded<T> {
    pub value: T,
    pub read_only: bool,
    pub notices: Vec<LoadNotice>,
}

/// Something the user is told after the first frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadNotice {
    /// The file could not be read or parsed and was renamed. `kept_as` is the new file name
    /// only, for example "settings.toml.corrupt-1759500000".
    Corrupt { kept_as: String },
    /// The file has a higher `schema_version` than this build.
    Newer,
}

impl LoadNotice {
    pub fn toast_text(&self) -> String {
        match self {
            LoadNotice::Corrupt { kept_as } => format!(
                "Settings file was unreadable and has been reset. The old file was kept as {kept_as}"
            ),
            LoadNotice::Newer => {
                "Settings were written by a newer tasklens; changes will not be saved".to_owned()
            }
        }
    }
}

pub fn load_settings(path: &Path, now_unix: u64) -> Loaded<Settings> {
    load(FileKind::Settings, path, now_unix)
}

pub fn load_state(path: &Path, now_unix: u64) -> Loaded<UiState> {
    load(FileKind::State, path, now_unix)
}

/// What steps 1 to 3 leave for deserializing.
enum LoadStep {
    /// Missing or unreadable: use defaults.
    Defaults { notices: Vec<LoadNotice> },
    /// Parsed and, if older, migrated. `version` is the schema the table now has.
    Table {
        table: toml::Table,
        version: u32,
        read_only: bool,
        notices: Vec<LoadNotice>,
    },
}

/// The two file types, so `load` can stay generic.
trait Stored: DeserializeOwned + Default {
    fn clamp(&mut self) -> Vec<&'static str>;
    fn set_version(&mut self, version: u32);
    /// Drops what clamping does not cover. Runs after `clamp`.
    fn drop_unknown(&mut self) {}
}

impl Stored for Settings {
    fn clamp(&mut self) -> Vec<&'static str> {
        Settings::clamp(self)
    }

    fn set_version(&mut self, version: u32) {
        self.schema_version = version;
    }
}

impl Stored for UiState {
    fn clamp(&mut self) -> Vec<&'static str> {
        UiState::clamp(self)
    }

    fn set_version(&mut self, version: u32) {
        self.schema_version = version;
    }

    fn drop_unknown(&mut self) {
        let c = &mut self.columns;
        retain_known(&mut c.processes, "processes", |s| {
            ColumnId::from_str(s).is_some()
        });
        retain_known(&mut c.history, "history", |s| {
            ColumnId::from_str(s).is_some()
        });
        retain_known(&mut c.services, "services", |s| {
            SysColumnId::from_str(SysTable::Services, s).is_some()
        });
        retain_known(&mut c.startup, "startup", |s| {
            SysColumnId::from_str(SysTable::Startup, s).is_some()
        });
        retain_known(&mut c.connections, "connections", |s| {
            SysColumnId::from_str(SysTable::Connections, s).is_some()
        });
    }
}

/// Keeps the known column ids of one table, in their saved order.
fn retain_known(state: &mut ColumnState, table: &str, known: impl Fn(&str) -> bool) {
    let before = state.order.len() + state.visible.len() + state.widths.len();
    state.order.retain(|id| known(id));
    state.visible.retain(|id| known(id));
    state.widths.retain(|id, _| known(id));
    let dropped = before - (state.order.len() + state.visible.len() + state.widths.len());
    if dropped > 0 {
        // The ids come from the file, so only the count is logged.
        info!(
            target: "tasklens::settings",
            "dropped {dropped} unknown column id entries from columns.{table}"
        );
    }
}

fn load<T: Stored>(kind: FileKind, path: &Path, now_unix: u64) -> Loaded<T> {
    let (table, version, read_only, notices) = match load_table(path, now_unix) {
        LoadStep::Defaults { notices } => {
            return Loaded {
                value: T::default(),
                read_only: false,
                notices,
            };
        }
        LoadStep::Table {
            table,
            version,
            read_only,
            notices,
        } => (table, version, read_only, notices),
    };
    let mut value: T = deserialize_by_section(kind, table);
    value.set_version(version);
    for key in value.clamp() {
        info!(
            target: "tasklens::settings",
            "{}: {key} was out of range and has been clamped",
            kind.label()
        );
    }
    value.drop_unknown();
    Loaded {
        value,
        read_only,
        notices,
    }
}

/// Load steps 1 to 3: read, parse, rename a corrupt file, migrate an older one.
fn load_table(path: &Path, now_unix: u64) -> LoadStep {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return LoadStep::Defaults {
                notices: Vec::new(),
            };
        }
        Err(e) => {
            warn!(
                target: "tasklens::settings",
                "{}: could not read: {}",
                path.display(),
                e.kind()
            );
            None
        }
    };
    let parsed = text.and_then(|text| match text.parse::<toml::Table>() {
        Ok(table) => Some(table),
        Err(_) => {
            // The parse error quotes the file, so it is not logged.
            warn!(target: "tasklens::settings", "{}: not valid TOML", path.display());
            None
        }
    });
    let Some(mut table) = parsed else {
        return LoadStep::Defaults {
            notices: vec![LoadNotice::Corrupt {
                kept_as: keep_corrupt(path, now_unix),
            }],
        };
    };

    let from = migrations::version_of(&table);
    if from > SCHEMA_VERSION {
        warn!(
            target: "tasklens::settings",
            "{}: schema_version {from} is newer than {SCHEMA_VERSION}; read-only",
            path.display()
        );
        return LoadStep::Table {
            table,
            version: from,
            read_only: true,
            notices: vec![LoadNotice::Newer],
        };
    }
    let version = migrations::migrate(&mut table, from);
    LoadStep::Table {
        table,
        version,
        read_only: false,
        notices: Vec::new(),
    }
}

/// Renames a corrupt file to `<name>.corrupt-<now_unix>` and returns the new name. A failed
/// rename is logged; the name is returned anyway.
fn keep_corrupt(path: &Path, now_unix: u64) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let kept_as = format!("{name}.corrupt-{now_unix}");
    if let Err(e) = std::fs::rename(path, path.with_file_name(&kept_as)) {
        warn!(
            target: "tasklens::settings",
            "{}: could not rename the corrupt file: {}",
            path.display(),
            e.kind()
        );
    }
    kept_as
}

/// Load step 4. A section with a value of the wrong type takes its defaults; the other sections
/// keep their file values.
fn deserialize_by_section<T: Stored>(kind: FileKind, mut table: toml::Table) -> T {
    if let Ok(value) = table.clone().try_into() {
        return value;
    }
    let bad: Vec<String> = table
        .iter()
        .filter(|(key, value)| {
            let mut single = toml::Table::new();
            single.insert((*key).clone(), (*value).clone());
            single.try_into::<T>().is_err()
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in &bad {
        warn!(
            target: "tasklens::settings",
            "{}: section {key} has a bad value; using its defaults",
            kind.label()
        );
        table.remove(key);
    }
    table.try_into().unwrap_or_else(|_| {
        warn!(
            target: "tasklens::settings",
            "{}: could not be read; using defaults",
            kind.label()
        );
        T::default()
    })
}

/// How long the store waits after the last change before it saves (ADR-011 Save).
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// The most `run-history.json` bytes `RunHistoryFile::load` reads.
const RUN_HISTORY_MAX_BYTES: u64 = 1024 * 1024;

/// The ADR-011 persist callback. It is `Send` and `Sync`, so it also fits `tl_ui::app::PersistFn`.
pub type PersistCallback = Box<dyn Fn(&Settings, &UiState) + Send + Sync>;

/// Saves settings, state and themes for the life of the app (ADR-011 Save).
///
/// The persist callbacks only clone their arguments and wake the `tasklens-save` thread, so they
/// never block the GPUI main thread. That thread serializes and writes.
pub struct Store {
    inner: Arc<Inner>,
    saver: Mutex<Option<JoinHandle<()>>>,
}

struct Inner {
    settings: Target,
    state: Target,
    themes_file: PathBuf,
    autostart_file: PathBuf,
    /// The `--app-id` value, written into the autostart entry's `Exec`.
    app_id: Option<String>,
    shared: Mutex<Shared>,
    wake: Condvar,
    /// The last bytes loaded or written per file. Held during every write, so the saver thread
    /// and `flush` never write at the same time.
    written: Mutex<Written>,
}

struct Target {
    kind: FileKind,
    path: PathBuf,
    read_only: bool,
}

struct Written {
    settings: String,
    state: String,
    /// The `[startup]` values the autostart entry last followed. Starts as the loaded values,
    /// so the entry is only touched once the user changes them.
    startup: StartupSettings,
}

/// What the callbacks hand to the saver thread.
struct Shared {
    /// The newest value from the persist callback, not yet saved.
    pending: Option<(Settings, UiState)>,
    /// A value whose save failed. The next change or `flush` retries it.
    failed: Option<(Settings, UiState)>,
    /// When `pending` was last set.
    changed_at: Instant,
    themes: Vec<PersistTheme>,
    /// Set by `flush` and on drop. Later calls are ignored.
    stopped: bool,
}

impl Store {
    /// Starts the `tasklens-save` thread. `initial` is what was loaded; a file is only written
    /// when its serialized text differs from the text of the last value loaded or written.
    pub fn new(
        paths: &Paths,
        settings_ro: bool,
        state_ro: bool,
        initial: (Settings, UiState),
    ) -> Store {
        let inner = Arc::new(Inner {
            settings: Target {
                kind: FileKind::Settings,
                path: paths.settings_file(),
                read_only: settings_ro,
            },
            state: Target {
                kind: FileKind::State,
                path: paths.state_file(),
                read_only: state_ro,
            },
            themes_file: paths.themes_file(),
            autostart_file: paths.autostart_file(),
            app_id: paths.app_id.clone(),
            shared: Mutex::new(Shared {
                pending: None,
                failed: None,
                changed_at: Instant::now(),
                themes: Vec::new(),
                stopped: false,
            }),
            wake: Condvar::new(),
            written: Mutex::new(Written {
                settings: serialize_settings(&initial.0),
                state: serialize_state(&initial.1),
                startup: initial.0.startup.clone(),
            }),
        });
        // The saver thread logs through the subscriber of the thread that made the store.
        let dispatch = tracing::dispatcher::get_default(|d| d.clone());
        let thread_inner = inner.clone();
        let saver = std::thread::Builder::new()
            .name("tasklens-save".to_owned())
            .spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || run_saver(&thread_inner));
            })
            .map_err(|e| {
                warn!(target: "tasklens::settings", "could not start the save thread: {e}");
            })
            .ok();
        Store {
            inner,
            saver: Mutex::new(saver),
        }
    }

    /// The ADR-011 persist callback for `UiInit.persist`. tl-ui calls it on every change; the
    /// store debounces by `SAVE_DEBOUNCE`.
    pub fn persist_callback(&self) -> PersistCallback {
        let inner = self.inner.clone();
        Box::new(move |settings, state| {
            let mut shared = lock(&inner.shared);
            if shared.stopped {
                return;
            }
            shared.pending = Some((settings.clone(), state.clone()));
            shared.failed = None;
            shared.changed_at = Instant::now();
            inner.wake.notify_one();
        })
    }

    /// The callback for `UiInit.persist_themes`. Requests run in order on `tasklens-save`.
    pub fn persist_themes_callback(&self) -> Box<dyn Fn(PersistTheme)> {
        let inner = self.inner.clone();
        Box::new(move |req| {
            let mut shared = lock(&inner.shared);
            if shared.stopped {
                return;
            }
            shared.themes.push(req);
            inner.wake.notify_one();
        })
    }

    /// Writes `settings.toml` now, on the calling thread, for Reset settings
    /// (`AppEnv.write_settings_now`). The error text is shown to the user.
    pub fn write_settings_now(&self, s: &Settings) -> Result<(), String> {
        let target = &self.inner.settings;
        if target.read_only {
            return Err(LoadNotice::Newer.toast_text());
        }
        let mut written = lock(&self.inner.written);
        save_file(target, serialize_settings(s), &mut written.settings)
    }

    /// Writes any pending change now, on the calling thread, and stops the saver thread. Called
    /// on exit. Calls to the callbacks after this are ignored.
    pub fn flush(&self) {
        lock(&self.inner.shared).stopped = true;
        self.inner.wake.notify_all();
        if let Some(saver) = lock(&self.saver).take()
            && saver.join().is_err()
        {
            warn!(target: "tasklens::settings", "the save thread panicked");
        }
        let (pending, themes) = {
            let mut shared = lock(&self.inner.shared);
            let pending = shared.pending.take().or_else(|| shared.failed.take());
            (pending, std::mem::take(&mut shared.themes))
        };
        for req in themes {
            self.inner.persist_theme(req);
        }
        if let Some((settings, state)) = pending {
            self.inner.save(&settings, &state);
        }
    }
}

impl Drop for Store {
    /// Stops the saver thread without saving. `flush` is the exit path.
    fn drop(&mut self) {
        lock(&self.inner.shared).stopped = true;
        self.inner.wake.notify_all();
    }
}

impl Inner {
    /// Writes each file whose text changed. Returns false when a write failed.
    fn save(&self, settings: &Settings, state: &UiState) -> bool {
        let mut written = lock(&self.written);
        let settings_ok = save_file(
            &self.settings,
            serialize_settings(settings),
            &mut written.settings,
        )
        .is_ok();
        let state_ok = save_file(&self.state, serialize_state(state), &mut written.state).is_ok();
        // A read-only settings file does not block this; the entry is not the settings file.
        self.apply_autostart(&settings.startup, &mut written.startup);
        settings_ok && state_ok
    }

    /// Writes or removes the autostart entry when `[startup]` differs from `last`. A failure is
    /// a warning only, and `last` is kept so the next save retries.
    fn apply_autostart(&self, startup: &StartupSettings, last: &mut StartupSettings) {
        if startup == last {
            return;
        }
        match autostart::apply(
            &self.autostart_file,
            startup.start_at_login,
            startup.start_minimized,
            self.app_id.as_deref(),
        ) {
            Ok(_) => *last = startup.clone(),
            Err(error) => warn!(
                target: "tasklens::settings",
                "Could not update the autostart entry: {error}"
            ),
        }
    }

    fn persist_theme(&self, req: PersistTheme) {
        match req {
            PersistTheme::Save(themes) => {
                let text = serialize_user_themes(&themes);
                if let Err(e) = write_file(&self.themes_file, text.as_bytes()) {
                    warn!(target: "tasklens::settings", "Could not save themes.json: {e}");
                }
            }
            PersistTheme::RenameCorrupt { unix_seconds } => {
                let to = self
                    .themes_file
                    .with_file_name(format!("themes.json.corrupt-{unix_seconds}"));
                if let Err(e) = std::fs::rename(&self.themes_file, to) {
                    warn!(
                        target: "tasklens::settings",
                        "Could not rename the corrupt themes.json: {}",
                        e.kind()
                    );
                }
            }
        }
    }
}

/// The `tasklens-save` loop: theme requests run at once; a settings change runs once
/// `SAVE_DEBOUNCE` has passed with no newer change.
fn run_saver(inner: &Inner) {
    let mut shared = lock(&inner.shared);
    loop {
        if shared.stopped {
            return;
        }
        if !shared.themes.is_empty() {
            let reqs = std::mem::take(&mut shared.themes);
            drop(shared);
            for req in reqs {
                inner.persist_theme(req);
            }
            shared = lock(&inner.shared);
            continue;
        }
        if shared.pending.is_none() {
            shared = inner
                .wake
                .wait(shared)
                .unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        let due = shared.changed_at + SAVE_DEBOUNCE;
        let now = Instant::now();
        if now < due {
            shared = inner
                .wake
                .wait_timeout(shared, due - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
            continue;
        }
        let Some((settings, state)) = shared.pending.take() else {
            continue;
        };
        drop(shared);
        let ok = inner.save(&settings, &state);
        shared = lock(&inner.shared);
        // A newer change replaces the failed value; it is saved in full on its own turn.
        if !ok && shared.pending.is_none() {
            shared.failed = Some((settings, state));
        }
    }
}

/// Writes `text` to the target when it is not read-only and differs from `last`. A write error
/// is logged by file name and kind only.
fn save_file(target: &Target, text: String, last: &mut String) -> Result<(), String> {
    if target.read_only || text == *last {
        return Ok(());
    }
    match write_file(&target.path, text.as_bytes()) {
        Ok(()) => {
            *last = text;
            Ok(())
        }
        Err(e) => {
            let message = format!("Could not save {}: {e}", target.kind.label());
            warn!(target: "tasklens::settings", "{message}");
            Err(message)
        }
    }
}

/// Creates the parent directory (mode 0700) and writes `bytes` atomically with mode 0600.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        ensure_dir(dir).map_err(|e| e.to_string())?;
    }
    atomic::write_atomic(path, bytes, FILE_MODE).map_err(|e| e.to_string())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The `settings.toml` text. `schema_version = 1` is the first line; keys the model does not
/// know are not written (ADR-011 Save step 5).
fn serialize_settings(s: &Settings) -> String {
    let mut s = s.clone();
    s.schema_version = SCHEMA_VERSION;
    toml::to_string_pretty(&s).expect("settings always serialize to TOML")
}

/// The `state.toml` text, with `schema_version = 1` first.
fn serialize_state(s: &UiState) -> String {
    let mut s = s.clone();
    s.schema_version = SCHEMA_VERSION;
    toml::to_string_pretty(&s).expect("state always serializes to TOML")
}

/// The Run dialog history file `$XDG_STATE_HOME/tasklens/run-history.json`, a pretty-printed
/// JSON array.
pub struct RunHistoryFile {
    path: PathBuf,
}

/// One `run-history.json` record. `RunHistoryEntry` lives in tl-ui and has no serde derives.
#[derive(Serialize, Deserialize)]
struct RunHistoryRecord {
    command: String,
    executed_at: String,
    elevated: bool,
}

impl RunHistoryFile {
    pub fn new(paths: &Paths) -> RunHistoryFile {
        RunHistoryFile {
            path: paths.run_history_file(),
        }
    }
}

impl RunHistoryStore for RunHistoryFile {
    /// Reads at most 1 MiB. Any error gives an empty list.
    fn load(&self) -> Vec<RunHistoryEntry> {
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&self.path)
            .and_then(|f| f.take(RUN_HISTORY_MAX_BYTES).read_to_end(&mut bytes));
        if let Err(e) = read {
            if e.kind() != ErrorKind::NotFound {
                warn!(target: "tasklens::settings", "run-history.json: could not read: {}", e.kind());
            }
            return Vec::new();
        }
        match serde_json::from_slice::<Vec<RunHistoryRecord>>(&bytes) {
            Ok(records) => records
                .into_iter()
                .map(|r| RunHistoryEntry {
                    command: r.command,
                    executed_at: r.executed_at,
                    elevated: r.elevated,
                })
                .collect(),
            Err(_) => {
                // The parse error quotes the file, so it is not logged.
                warn!(target: "tasklens::settings", "run-history.json: not valid JSON");
                Vec::new()
            }
        }
    }

    fn save(&self, entries: &[RunHistoryEntry]) -> Result<(), String> {
        let records: Vec<RunHistoryRecord> = entries
            .iter()
            .map(|e| RunHistoryRecord {
                command: e.command.clone(),
                executed_at: e.executed_at.clone(),
                elevated: e.elevated,
            })
            .collect();
        let bytes = serde_json::to_vec_pretty(&records).map_err(|e| e.to_string())?;
        write_file(&self.path, &bytes).map_err(|e| {
            let message = format!("Could not save run-history.json: {e}");
            warn!(target: "tasklens::settings", "{message}");
            message
        })
    }
}
