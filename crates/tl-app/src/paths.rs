//! XDG path resolution (ADR-011, table (c)). Paths are resolved by hand, without the
//! `directories` crate (ADR-002).

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Mode for every file tasklens writes.
pub const FILE_MODE: u32 = 0o600;
/// Mode for every directory tasklens creates.
pub const DIR_MODE: u32 = 0o700;

const APP_DIR: &str = "tasklens";
const AUTOSTART_NAME: &str = "io.github.oysteinkrog.Tasklens.desktop";

/// The resolved roots for every tasklens file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// `$XDG_CONFIG_HOME/tasklens`
    pub config_dir: PathBuf,
    /// `$XDG_STATE_HOME/tasklens`
    pub state_dir: PathBuf,
    /// `$XDG_CACHE_HOME/tasklens`
    pub cache_dir: PathBuf,
    pub home: PathBuf,
    /// The resolved `$XDG_CONFIG_HOME` root, used for the autostart entry.
    config_root: PathBuf,
}

impl Paths {
    /// Resolves the paths from the process environment.
    pub fn from_env() -> anyhow::Result<Paths> {
        Self::from_vars(|name| std::env::var_os(name))
    }

    /// Resolves the paths from `get`, which returns the value of an environment variable.
    pub fn from_vars(get: impl Fn(&str) -> Option<OsString>) -> anyhow::Result<Paths> {
        let home = absolute(get("HOME"))
            .ok_or_else(|| anyhow::anyhow!("HOME is not set to an absolute path"))?;
        let root =
            |var: &str, default: &str| absolute(get(var)).unwrap_or_else(|| home.join(default));
        let config_root = root("XDG_CONFIG_HOME", ".config");
        let state_root = root("XDG_STATE_HOME", ".local/state");
        let cache_root = root("XDG_CACHE_HOME", ".cache");
        Ok(Paths {
            config_dir: config_root.join(APP_DIR),
            state_dir: state_root.join(APP_DIR),
            cache_dir: cache_root.join(APP_DIR),
            home,
            config_root,
        })
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }

    pub fn themes_file(&self) -> PathBuf {
        self.config_dir.join("themes.json")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.toml")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }

    pub fn crash_dir(&self) -> PathBuf {
        self.state_dir.join("crash")
    }

    pub fn history_dir(&self) -> PathBuf {
        self.state_dir.join("history")
    }

    pub fn dumps_dir(&self) -> PathBuf {
        self.state_dir.join("dumps")
    }

    pub fn run_history_file(&self) -> PathBuf {
        self.state_dir.join("run-history.json")
    }

    pub fn icon_cache_file(&self) -> PathBuf {
        self.cache_dir.join("icons.toml")
    }

    /// `$XDG_CONFIG_HOME/autostart/io.github.oysteinkrog.Tasklens.desktop`
    pub fn autostart_file(&self) -> PathBuf {
        self.config_root.join("autostart").join(AUTOSTART_NAME)
    }
}

/// Returns the value as a path when it is set, not empty and absolute.
fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    path.is_absolute().then_some(path)
}

/// Creates `path` and any missing parents, then sets mode 0700 on each directory it created.
/// A directory that already existed keeps its mode.
pub fn ensure_dir(path: &Path) -> std::io::Result<()> {
    let mut missing = Vec::new();
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir.as_os_str().is_empty() || dir.exists() {
            break;
        }
        missing.push(dir);
        current = dir.parent();
    }
    std::fs::create_dir_all(path)?;
    for dir in missing {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(DIR_MODE))?;
    }
    Ok(())
}
