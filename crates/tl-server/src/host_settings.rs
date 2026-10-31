//! Host settings in `$XDG_CONFIG_HOME/tasklens/engine.toml` (ADR-018 Settings and the
//! Implementation contract "Settings migration").
//!
//! The engine that holds the engine lock owns this file: it loads it at start and writes it on
//! every change a client makes with `SetHostSettings`. Every change needs the current revision;
//! a write from an older one is a `Conflict` that carries the current settings. The revision is
//! stored in the file, so it survives restarts. A stored revision is at least 1, so a client
//! that never read the settings (base revision 0) can never write them.
//!
//! Load follows ADR-011: a missing file is created, once, from the host keys in `settings.toml`
//! (`processes.interval_ms` and `[history]`); an unreadable one is renamed to
//! `engine.toml.corrupt-<unix seconds>` and replaced with defaults, without a second import; a
//! newer `schema_version` is read-only, so it is never written; out-of-range values are clamped.
//! Writes are atomic with mode 0600 (`tl_ops::write_atomic`).
//!
//! Logs name files and keys, never their values.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use tl_model::remote_proto::{
    HostSettings, HostSettingsChanges, HostSettingsValues, SetHostSettingsReply,
};
use tl_model::settings::HistorySettings;
use tracing::{info, warn};

/// The host settings file, in the config directory.
pub const ENGINE_FILE: &str = "engine.toml";
/// The window's settings file, the source of the one-time import.
pub const SETTINGS_FILE: &str = "settings.toml";
/// The `engine.toml` schema this build writes.
pub const SCHEMA_VERSION: u32 = 1;

const VERSION_KEY: &str = "schema_version";
const REV_KEY: &str = "rev";
const REMOTE_ADMIN_KEY: &str = "remote_admin";
const LOG: &str = "tasklens::settings";

/// Where the settings at start came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// `engine.toml` was read.
    Loaded,
    /// `engine.toml` was missing and was created from the host keys in `settings.toml`.
    Imported,
    /// Neither file existed: defaults, written to a new `engine.toml`.
    Created,
    /// `engine.toml` was unreadable and was renamed to `kept_as` (a file name only). Defaults
    /// are written to a new file; `settings.toml` is not imported again.
    Corrupt { kept_as: String },
}

/// `engine.toml` and the settings it holds.
#[derive(Debug)]
pub struct HostSettingsFile {
    path: PathBuf,
    current: HostSettings,
}

impl HostSettingsFile {
    /// Loads `dir/engine.toml`, or creates it as the module docs say. `dir` must exist.
    pub fn open(dir: &Path, now_unix: u64) -> (Self, Origin) {
        let path = dir.join(ENGINE_FILE);
        let (current, origin) = match read(&path) {
            Read::Missing => match import(&dir.join(SETTINGS_FILE)) {
                Some(values) => (fresh(values), Origin::Imported),
                None => (fresh(HostSettingsValues::default()), Origin::Created),
            },
            Read::Unreadable => {
                let kept_as = keep_corrupt(&path, now_unix);
                let origin = Origin::Corrupt { kept_as };
                (fresh(HostSettingsValues::default()), origin)
            }
            Read::Table(table) => (from_table(&table), Origin::Loaded),
        };
        let file = HostSettingsFile { path, current };
        if origin != Origin::Loaded {
            file.save();
        }
        (file, origin)
    }

    pub fn current(&self) -> &HostSettings {
        &self.current
    }

    /// Applies `changes` when `base_rev` is the current revision and saves the file, unless it
    /// is read-only. Changes that leave every value as it is keep the revision. A failed write
    /// is logged; the new values still apply until the engine stops.
    pub fn set(&mut self, base_rev: u64, changes: &HostSettingsChanges) -> SetHostSettingsReply {
        if base_rev != self.current.rev {
            return SetHostSettingsReply::Conflict {
                current: self.current.clone(),
            };
        }
        let next = self.current.values.with_changes(changes);
        if next != self.current.values {
            self.current.values = next;
            self.current.rev += 1;
            self.save();
        }
        SetHostSettingsReply::Ack {
            rev: self.current.rev,
        }
    }

    /// Writes the file, unless it is read-only. A failure is logged.
    fn save(&self) {
        if self.current.read_only {
            return;
        }
        let text = to_toml(&self.current);
        if let Err(e) = tl_ops::write_atomic(&self.path, text.as_bytes(), tl_ops::PRIVATE_FILE_MODE)
        {
            warn!(target: LOG, "{}: could not be written: {e}", self.path.display());
        }
    }
}

/// The settings file shared by every session of the engine. `apply` hands new values to the
/// engine; it runs under the lock, so the engine sees changes in revision order.
pub struct SharedHostSettings {
    file: Mutex<HostSettingsFile>,
    apply: Box<dyn Fn(&HostSettingsValues) + Send + Sync>,
}

impl SharedHostSettings {
    pub fn new(
        file: HostSettingsFile,
        apply: Box<dyn Fn(&HostSettingsValues) + Send + Sync>,
    ) -> Self {
        Self {
            file: Mutex::new(file),
            apply,
        }
    }

    pub fn get(&self) -> HostSettings {
        self.lock().current().clone()
    }

    pub fn set(&self, base_rev: u64, changes: &HostSettingsChanges) -> SetHostSettingsReply {
        let mut file = self.lock();
        let rev = file.current().rev;
        let reply = file.set(base_rev, changes);
        if file.current().rev != rev {
            (self.apply)(&file.current().values);
        }
        reply
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HostSettingsFile> {
        self.file.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// New settings at the first stored revision.
fn fresh(values: HostSettingsValues) -> HostSettings {
    HostSettings {
        rev: 1,
        values,
        read_only: false,
    }
}

enum Read {
    Missing,
    /// Could not be read or is not valid TOML.
    Unreadable,
    Table(toml::Table),
}

fn read(path: &Path) -> Read {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Read::Missing,
        Err(e) => {
            warn!(target: LOG, "{}: could not read: {}", path.display(), e.kind());
            return Read::Unreadable;
        }
    };
    match text.parse::<toml::Table>() {
        Ok(table) => Read::Table(table),
        Err(_) => {
            // The parse error quotes the file, so it is not logged.
            warn!(target: LOG, "{}: not valid TOML", path.display());
            Read::Unreadable
        }
    }
}

/// The host keys of `settings.toml`, or `None` when it is missing or unreadable. The window
/// owns that file, so it is never renamed or written here.
fn import(path: &Path) -> Option<HostSettingsValues> {
    match read(path) {
        Read::Table(table) => {
            let values = host_keys(&table, path);
            info!(target: LOG, "{ENGINE_FILE}: host settings imported from {SETTINGS_FILE}");
            Some(values)
        }
        Read::Missing => None,
        Read::Unreadable => {
            warn!(target: LOG, "{ENGINE_FILE}: {SETTINGS_FILE} unreadable; host settings start from defaults");
            None
        }
    }
}

/// `processes.interval_ms` and `[history]` from `table`, clamped. A key with a value of the
/// wrong type keeps its default.
fn host_keys(table: &toml::Table, path: &Path) -> HostSettingsValues {
    let mut values = HostSettingsValues::default();
    if let Some(v) = table.get("processes").and_then(|p| p.get("interval_ms")) {
        match v.as_integer().and_then(|i| u32::try_from(i).ok()) {
            Some(ms) => values.interval_ms = ms,
            None => bad_value(path, "processes.interval_ms"),
        }
    }
    if let Some(v) = table.get("history") {
        match v.clone().try_into::<HistorySettings>() {
            Ok(history) => values.history = history,
            Err(_) => bad_value(path, "history"),
        }
    }
    for key in values.clamp() {
        info!(target: LOG, "{}: {key} was out of range and has been clamped", path.display());
    }
    values
}

fn bad_value(path: &Path, key: &str) {
    warn!(target: LOG, "{}: {key} has a bad value; using its default", path.display());
}

/// The settings in a parsed `engine.toml`. A newer schema is read-only (ADR-011 load step 3).
fn from_table(table: &toml::Table) -> HostSettings {
    let path = Path::new(ENGINE_FILE);
    let version = match table.get(VERSION_KEY) {
        None => 1,
        Some(v) => v
            .as_integer()
            .and_then(|i| u32::try_from(i).ok())
            .filter(|&v| v >= 1)
            .unwrap_or_else(|| {
                bad_value(path, VERSION_KEY);
                1
            }),
    };
    let read_only = version > SCHEMA_VERSION;
    if read_only {
        warn!(target: LOG, "{ENGINE_FILE}: schema_version {version} is newer than {SCHEMA_VERSION}; read-only");
    }
    let mut values = host_keys(table, path);
    if let Some(v) = table.get(REMOTE_ADMIN_KEY) {
        match v.as_bool() {
            Some(on) => values.remote_admin = on,
            None => bad_value(path, REMOTE_ADMIN_KEY),
        }
    }
    let rev = match table.get(REV_KEY) {
        None => 1,
        Some(v) => v
            .as_integer()
            .and_then(|i| u64::try_from(i).ok())
            .unwrap_or_else(|| {
                bad_value(path, REV_KEY);
                1
            }),
    };
    HostSettings {
        rev: rev.max(1),
        values,
        read_only,
    }
}

fn to_toml(s: &HostSettings) -> String {
    let mut table = toml::Table::new();
    table.insert(VERSION_KEY.into(), i64::from(SCHEMA_VERSION).into());
    // A revision past i64::MAX cannot happen: it goes up by one per change.
    table.insert(
        REV_KEY.into(),
        i64::try_from(s.rev).unwrap_or(i64::MAX).into(),
    );
    table.insert(REMOTE_ADMIN_KEY.into(), s.values.remote_admin.into());
    let mut processes = toml::Table::new();
    processes.insert("interval_ms".into(), i64::from(s.values.interval_ms).into());
    table.insert("processes".into(), processes.into());
    match toml::Value::try_from(&s.values.history) {
        Ok(history) => {
            table.insert("history".into(), history);
        }
        // HistorySettings has only integers and booleans, so this cannot fail.
        Err(e) => warn!(target: LOG, "{ENGINE_FILE}: [history] not serialized: {e}"),
    }
    table.to_string()
}

/// Renames a corrupt file to `<name>.corrupt-<now_unix>` and returns the new name. A failed
/// rename is logged; the name is returned anyway.
fn keep_corrupt(path: &Path, now_unix: u64) -> String {
    let kept_as = format!("{ENGINE_FILE}.corrupt-{now_unix}");
    if let Err(e) = std::fs::rename(path, path.with_file_name(&kept_as)) {
        warn!(target: LOG, "{}: could not rename the corrupt file: {}", path.display(), e.kind());
    }
    kept_as
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use super::*;

    const NOW: u64 = 1_790_000_000;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn engine_text(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(ENGINE_FILE)).unwrap()
    }

    fn interval(ms: u32) -> HostSettingsChanges {
        HostSettingsChanges {
            interval_ms: Some(ms),
            ..HostSettingsChanges::default()
        }
    }

    const SETTINGS: &str = r#"
schema_version = 1

[units]
network = "bytes"

[processes]
interval_ms = 2000
hide_system = true

[history]
enabled = true
retention_h = 3
persist = false
max_disk_mb = 100
device_metrics = false
device_metrics_max_mb = 32
"#;

    fn imported() -> HostSettingsValues {
        HostSettingsValues {
            interval_ms: 2000,
            history: HistorySettings {
                enabled: true,
                retention_h: 3,
                persist: false,
                max_disk_mb: 100,
                device_metrics: false,
                device_metrics_max_mb: 32,
            },
            remote_admin: false,
        }
    }

    #[test]
    fn migration_keeps_values_and_happens_once() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), SETTINGS_FILE, SETTINGS);
        let (file, origin) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(origin, Origin::Imported);
        assert_eq!(file.current().values, imported());
        assert_eq!(file.current().rev, 1);
        assert!(!file.current().read_only);
        let mode = std::fs::metadata(tmp.path().join(ENGINE_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // The window's file is left as it was.
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(SETTINGS_FILE)).unwrap(),
            SETTINGS
        );

        // A later change to settings.toml is not imported again.
        write(
            tmp.path(),
            SETTINGS_FILE,
            "[processes]\ninterval_ms = 5000\n",
        );
        let (again, origin) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(origin, Origin::Loaded);
        assert_eq!(again.current(), file.current());
    }

    #[test]
    fn a_fresh_install_writes_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let (file, origin) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(origin, Origin::Created);
        assert_eq!(file.current().values, HostSettingsValues::default());
        assert_eq!(file.current().rev, 1);
        let (again, origin) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(origin, Origin::Loaded);
        assert_eq!(again.current(), file.current());
    }

    #[test]
    fn a_bad_import_value_keeps_its_default_and_out_of_range_is_clamped() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            SETTINGS_FILE,
            "[processes]\ninterval_ms = 50\n[history]\nenabled = \"yes\"\n",
        );
        let (file, _) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(file.current().values.interval_ms, 250);
        assert_eq!(file.current().values.history, HistorySettings::default());
    }

    #[test]
    fn conflicting_writes_get_the_current_settings_and_the_revision_survives_a_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut file, _) = HostSettingsFile::open(tmp.path(), NOW);
        // Two clients read revision 1; the first write wins.
        assert_eq!(
            file.set(1, &interval(500)),
            SetHostSettingsReply::Ack { rev: 2 }
        );
        let SetHostSettingsReply::Conflict { current } = file.set(1, &interval(5000)) else {
            panic!("the second write from revision 1 is a conflict");
        };
        assert_eq!((current.rev, current.values.interval_ms), (2, 500));

        let (mut again, _) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(again.current(), &current);
        assert_eq!(
            again.set(2, &interval(5000)),
            SetHostSettingsReply::Ack { rev: 3 }
        );
        // A change to the same values keeps the revision and does not write.
        let before = engine_text(tmp.path());
        assert_eq!(
            again.set(3, &interval(5000)),
            SetHostSettingsReply::Ack { rev: 3 }
        );
        assert_eq!(engine_text(tmp.path()), before);
    }

    #[test]
    fn a_client_that_never_read_cannot_write_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), SETTINGS_FILE, SETTINGS);
        let (mut file, _) = HostSettingsFile::open(tmp.path(), NOW);
        let before = engine_text(tmp.path());
        // A default `HostSettings` has revision 0, which is never stored.
        let unread = HostSettings::default();
        let changes = HostSettingsChanges {
            interval_ms: Some(unread.values.interval_ms),
            history: Some(unread.values.history.clone()),
            remote_admin: Some(unread.values.remote_admin),
        };
        assert!(matches!(
            file.set(unread.rev, &changes),
            SetHostSettingsReply::Conflict { current } if current.values == imported()
        ));
        assert_eq!(engine_text(tmp.path()), before);
    }

    #[test]
    fn a_newer_schema_is_read_only_and_never_written() {
        let tmp = tempfile::tempdir().unwrap();
        let newer = "schema_version = 99\nrev = 7\nremote_admin = true\nnew_key = 1\n\
                     [processes]\ninterval_ms = 2000\n";
        write(tmp.path(), ENGINE_FILE, newer);
        let (mut file, origin) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(origin, Origin::Loaded);
        let current = file.current().clone();
        assert!(current.read_only);
        assert_eq!(current.rev, 7);
        assert_eq!(current.values.interval_ms, 2000);
        assert!(current.values.remote_admin);
        // A change applies in memory only.
        assert_eq!(
            file.set(7, &interval(500)),
            SetHostSettingsReply::Ack { rev: 8 }
        );
        assert_eq!(file.current().values.interval_ms, 500);
        assert_eq!(engine_text(tmp.path()), newer);
    }

    #[test]
    fn a_corrupt_file_is_kept_aside_and_settings_are_not_imported_again() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), SETTINGS_FILE, SETTINGS);
        write(tmp.path(), ENGINE_FILE, "rev = [[[");
        let (file, origin) = HostSettingsFile::open(tmp.path(), NOW);
        let kept_as = format!("{ENGINE_FILE}.corrupt-{NOW}");
        assert_eq!(
            origin,
            Origin::Corrupt {
                kept_as: kept_as.clone()
            }
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(kept_as)).unwrap(),
            "rev = [[["
        );
        assert_eq!(file.current().values, HostSettingsValues::default());
        let (again, origin) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(origin, Origin::Loaded);
        assert_eq!(again.current(), file.current());
    }

    #[test]
    fn a_hand_edited_file_is_clamped_and_its_revision_is_at_least_one() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ENGINE_FILE,
            "rev = 0\n[processes]\ninterval_ms = 999999\n[history]\nretention_h = 0\n",
        );
        let (file, _) = HostSettingsFile::open(tmp.path(), NOW);
        assert_eq!(file.current().rev, 1);
        assert_eq!(file.current().values.interval_ms, 10_000);
        assert_eq!(file.current().values.history.retention_h, 1);
    }

    #[test]
    fn shared_settings_hand_only_real_changes_to_the_engine() {
        let tmp = tempfile::tempdir().unwrap();
        let (file, _) = HostSettingsFile::open(tmp.path(), NOW);
        let applied = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&applied);
        let shared = SharedHostSettings::new(
            file,
            Box::new(move |v: &HostSettingsValues| sink.lock().unwrap().push(v.interval_ms)),
        );
        assert!(matches!(
            shared.set(1, &interval(500)),
            SetHostSettingsReply::Ack { rev: 2 }
        ));
        assert!(matches!(
            shared.set(1, &interval(2000)),
            SetHostSettingsReply::Conflict { .. }
        ));
        assert!(matches!(
            shared.set(2, &interval(500)),
            SetHostSettingsReply::Ack { rev: 2 }
        ));
        assert_eq!(*applied.lock().unwrap(), vec![500]);
        assert_eq!(shared.get().values.interval_ms, 500);
    }
}
