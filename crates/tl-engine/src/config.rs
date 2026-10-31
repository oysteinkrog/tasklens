//! Engine configuration that `tl-app` fills once at start (ADR-001, ADR-004).
//!
//! `state_dir`, `history_dir` and `app_paths` hold user paths. Never log them above `debug`.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use tl_collect::SysRoot;
use tl_collect::apps::AppPaths;
use tl_collect::pkg::{PackageIndex, Pacman, PkgBackend};

use crate::desktop::{DesktopSession, DesktopSource};

/// Which worker set the engine runs. Fixed at start (ADR-004).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EngineMode {
    /// Every worker.
    Full,
    /// The `Processes` worker with the stripped sweep, and the `KWin` worker for
    /// `KWinCommand::KeepAbove` only (R-152 OI-9).
    LowMemory,
}

/// Makes a package owner backend, once per session.
pub type PkgBackendFn = Arc<dyn Fn() -> Box<dyn PkgBackend> + Send + Sync>;

/// The package owner backend of the shared package index (R-163 item 1).
#[derive(Clone, Default)]
pub enum PkgSource {
    /// `pacman -Qo` and the pacman local database.
    #[default]
    Pacman,
    /// No backend: the package fields are `Unsupported`.
    Disabled,
    /// A backend from tests.
    Custom(PkgBackendFn),
}

impl PkgSource {
    /// A new index over this backend, or `None` for `Disabled`.
    pub fn index(&self) -> Option<PackageIndex> {
        match self {
            PkgSource::Pacman => Some(PackageIndex::new(Box::new(Pacman::default()))),
            PkgSource::Disabled => None,
            PkgSource::Custom(make) => Some(PackageIndex::new(make())),
        }
    }
}

impl fmt::Debug for PkgSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PkgSource::Pacman => "Pacman",
            PkgSource::Disabled => "Disabled",
            PkgSource::Custom(_) => "Custom",
        })
    }
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
    /// The backend of the package index the `Processes` and `ProcessDetail` workers share.
    /// Production uses pacman; tests inject a fake.
    pub pkg_backend: PkgSource,
    /// The host's desktop session (ADR-018). `DesktopSource::Process` from `new`; the engine
    /// service sets `UserManager`.
    pub desktop: DesktopSession,
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
            pkg_backend: PkgSource::Pacman,
            desktop: DesktopSession::new(DesktopSource::Process),
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

    pub fn with_pkg_backend(mut self, backend: PkgSource) -> Self {
        self.pkg_backend = backend;
        self
    }

    /// The desktop session comes from `source` (ADR-018).
    pub fn with_desktop(mut self, source: DesktopSource) -> Self {
        self.desktop = DesktopSession::new(source);
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
        assert!(matches!(config.pkg_backend, PkgSource::Pacman));
        assert!(config.pkg_backend.index().is_some());
        let off = config.clone().with_pkg_backend(PkgSource::Disabled);
        assert!(off.pkg_backend.index().is_none());

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
