//! Engine configuration that `tl-app` fills once at start (ADR-001, ADR-004).
//!
//! `state_dir`, `history_dir` and `app_paths` hold user paths. Never log them above `debug`.

use std::path::PathBuf;

use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;

/// Which worker set the engine runs. Fixed at start (ADR-004).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EngineMode {
    /// Every worker.
    Full,
    /// The `Processes` worker with the stripped sweep, and the `KWin` worker for
    /// `KWinCommand::KeepAbove` only (R-152 OI-9).
    LowMemory,
}

/// Start settings for the engine. There is no `Default`: the state directory is mandatory.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub mode: EngineMode,
    /// `/proc`, `/sys` and `/run` roots. Tests pass a fixture (ADR-013).
    pub sys_root: SysRoot,
    /// `$XDG_STATE_HOME/tasklens`. Dumps live under it.
    pub state_dir: PathBuf,
    /// `tl-app` sets `$XDG_STATE_HOME/tasklens/history` (ADR-012, R-150 item 3).
    pub history_dir: Option<PathBuf>,
    /// `tl-app` sets `/usr/lib/tasklens/tl-helper` when the file exists (ADR-006, R-150 item 3).
    pub helper_path: Option<PathBuf>,
    /// Desktop files, icon themes and the icon cache for app grouping. `None` means the
    /// `Processes` worker reads them from the environment, with the icon cache at
    /// `$XDG_CACHE_HOME/tasklens/icons.toml`. Tests set it so they never write the real cache.
    pub app_paths: Option<AppPaths>,
}

impl EngineConfig {
    /// Host `SysRoot`, no history directory and no helper path.
    pub fn new(mode: EngineMode, state_dir: PathBuf) -> Self {
        Self {
            mode,
            sys_root: SysRoot::host(),
            state_dir,
            history_dir: None,
            helper_path: None,
            app_paths: None,
        }
    }

    pub fn with_sys_root(mut self, root: SysRoot) -> Self {
        self.sys_root = root;
        self
    }

    pub fn with_history_dir(mut self, dir: PathBuf) -> Self {
        self.history_dir = Some(dir);
        self
    }

    pub fn with_helper_path(mut self, path: PathBuf) -> Self {
        self.helper_path = Some(path);
        self
    }

    pub fn with_app_paths(mut self, paths: AppPaths) -> Self {
        self.app_paths = Some(paths);
        self
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn new_uses_real_sysroot() {
        let config = EngineConfig::new(EngineMode::Full, "/tmp/x".into());
        assert_eq!(config.mode, EngineMode::Full);
        assert_eq!(config.state_dir, Path::new("/tmp/x"));
        assert_eq!(config.sys_root, SysRoot::host());
    }

    #[test]
    fn with_sys_root_overrides() {
        let fixture = SysRoot::at(Path::new("/fixtures/step0"));
        let config = EngineConfig::new(EngineMode::LowMemory, "/tmp/x".into())
            .with_sys_root(fixture.clone());
        assert_eq!(config.sys_root.proc, Path::new("/fixtures/step0/proc"));
        assert_eq!(config.sys_root, fixture);
        assert_eq!(config.mode, EngineMode::LowMemory);
    }

    #[test]
    fn optional_paths_default_none() {
        let config = EngineConfig::new(EngineMode::Full, "/tmp/x".into());
        assert_eq!(config.history_dir, None);
        assert_eq!(config.helper_path, None);
        assert!(config.app_paths.is_none());

        let config = config
            .with_history_dir("/s/h".into())
            .with_helper_path("/usr/lib/tasklens/tl-helper".into());
        assert_eq!(config.history_dir.as_deref(), Some(Path::new("/s/h")));
        assert_eq!(
            config.helper_path.as_deref(),
            Some(Path::new("/usr/lib/tasklens/tl-helper"))
        );
    }
}
