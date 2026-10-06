//! Loads `settings.toml` and `state.toml` (ADR-011 Load, UI-SHL-110 step 5).
//!
//! Both files go through the same steps: a missing file gives defaults, an unreadable one is
//! renamed and reset, an older one is migrated, a newer one is read-only, a section with a bad
//! value takes its defaults, and out-of-range values are clamped. The notices become toasts after
//! the first frame (UI-SHL-110 step 11). Logs name files, sections and keys, never their values.

use std::io::ErrorKind;
use std::path::Path;

use serde::de::DeserializeOwned;
use tl_model::ids::ColumnId;
use tl_model::settings::{ColumnState, SCHEMA_VERSION, Settings, UiState};
use tl_model::table::{SysColumnId, SysTable};
use tracing::{info, warn};

use crate::migrations;

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
