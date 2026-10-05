//! Engine side of the GPU probe (ADR-007): where the probe binary is, its error type and the
//! deadlines every later GPU bead uses.
//!
//! A missing probe binary is the expected condition "GPU probe not installed", not a failure.
//! The engine never searches `PATH` for it.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustix::fs::{Access, access};
use tl_model::gpu_proto::InitFailure;

use crate::update::WorkerError;

pub mod client;
pub mod spawn;

/// Environment variable that names the probe binary explicitly.
pub const PROBE_ENV: &str = "TASKLENS_GPU_PROBE";
/// File name of the probe next to the running binary.
pub const PROBE_NAME: &str = "tl-gpu-probe";
/// Installed location, tried last.
pub const PROBE_SYSTEM_PATH: &str = "/usr/lib/tasklens/tl-gpu-probe";
pub const HELLO_DEADLINE: Duration = Duration::from_millis(3000);
pub const SAMPLE_DEADLINE: Duration = Duration::from_millis(500);
pub const SHUTDOWN_GRACE: Duration = Duration::from_millis(200);
pub const API_VERSIONS_DEADLINE: Duration = Duration::from_secs(5);
pub const NOT_INSTALLED: &str = "GPU probe not installed";
pub const VERSION_MISMATCH: &str = "probe version mismatch";

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("{NOT_INSTALLED}")]
    NotInstalled,
    #[error("spawn: {0}")]
    Spawn(Arc<str>),
    /// Holds the `WireError` text.
    #[error("protocol: {0}")]
    Protocol(Arc<str>),
    #[error("deadline {0:?} missed")]
    Deadline(Duration),
    /// For example "status 0" or "signal 11".
    #[error("probe exited: {0}")]
    Exited(Arc<str>),
    #[error("{VERSION_MISMATCH}: got {0}")]
    VersionMismatch(u16),
    #[error("init failed: {kind:?}: {message}")]
    InitFailed {
        kind: InitFailure,
        message: Arc<str>,
    },
}

impl From<ProbeError> for WorkerError {
    fn from(e: ProbeError) -> Self {
        WorkerError::Probe(e.to_string().into())
    }
}

/// Finds the probe binary. Lookup order:
/// 1. `env` set and non-empty: that path if it is a regular file, else `None`. A wrong explicit
///    override is not silently replaced by another binary.
/// 2. `tl-gpu-probe` next to `current_exe`, if it is an executable regular file.
/// 3. [`PROBE_SYSTEM_PATH`], if it is an executable regular file.
pub fn locate_probe(env: Option<&OsStr>, current_exe: Option<&Path>) -> Option<PathBuf> {
    locate_probe_in(env, current_exe, Path::new(PROBE_SYSTEM_PATH))
}

/// [`locate_probe`] with the environment and the running binary's path of this process.
pub fn locate() -> Option<PathBuf> {
    locate_probe(
        std::env::var_os(PROBE_ENV).as_deref(),
        std::env::current_exe().ok().as_deref(),
    )
}

fn locate_probe_in(
    env: Option<&OsStr>,
    current_exe: Option<&Path>,
    system: &Path,
) -> Option<PathBuf> {
    if let Some(env) = env.filter(|e| !e.is_empty()) {
        let path = PathBuf::from(env);
        let found = is_regular_file(&path);
        tracing::debug!(path = %path.display(), found, "GPU probe from {PROBE_ENV}");
        return found.then_some(path);
    }
    let sibling = current_exe
        .and_then(Path::parent)
        .map(|dir| dir.join(PROBE_NAME))
        .filter(|p| is_executable_file(p));
    let found = sibling.or_else(|| is_executable_file(system).then(|| system.to_path_buf()));
    tracing::debug!(path = ?found, "GPU probe lookup");
    found
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

fn is_executable_file(path: &Path) -> bool {
    is_regular_file(path) && access(path, Access::EXEC_OK).is_ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tl-engine-gpu-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn file(&self, name: &str, mode: u32) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"#!/bin/sh\n").expect("write file");
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn locate_env_override_wins_and_is_not_replaced() {
        let dir = TempDir::new("env");
        let probe = dir.file(PROBE_NAME, 0o755);
        let system = dir.file("system-probe", 0o755);
        let exe = dir.0.join("tasklens");

        let found = locate_probe_in(Some(probe.as_os_str()), Some(&exe), &system);
        assert_eq!(found, Some(probe));

        let missing = dir.0.join("missing").join(PROBE_NAME);
        let found = locate_probe_in(Some(missing.as_os_str()), Some(&exe), &system);
        assert_eq!(found, None);
    }

    #[test]
    fn locate_sibling_of_current_exe() {
        let dir = TempDir::new("sibling");
        let probe = dir.file(PROBE_NAME, 0o755);
        let exe = dir.0.join("tasklens");
        let system = dir.0.join("no-system-probe");

        assert_eq!(locate_probe_in(None, Some(&exe), &system), Some(probe));
        // An empty override counts as unset.
        assert_eq!(
            locate_probe_in(Some(OsStr::new("")), Some(&exe), &system),
            Some(dir.0.join(PROBE_NAME))
        );
    }

    #[test]
    fn locate_none_when_absent() {
        let dir = TempDir::new("absent");
        let exe = dir.0.join("tasklens");
        let system = dir.0.join("no-system-probe");

        assert_eq!(locate_probe_in(None, Some(&exe), &system), None);
        assert_eq!(locate_probe_in(None, None, &system), None);
    }

    #[test]
    fn locate_skips_non_executable() {
        let dir = TempDir::new("noexec");
        dir.file(PROBE_NAME, 0o644);
        let exe = dir.0.join("tasklens");

        let no_system = dir.0.join("no-system-probe");
        assert_eq!(locate_probe_in(None, Some(&exe), &no_system), None);

        let system = dir.file("system-probe", 0o755);
        assert_eq!(locate_probe_in(None, Some(&exe), &system), Some(system));
    }

    #[test]
    fn probe_error_to_worker_error_text() {
        let e: WorkerError = ProbeError::NotInstalled.into();
        assert!(matches!(&e, WorkerError::Probe(text) if &**text == NOT_INSTALLED));
        assert_eq!(NOT_INSTALLED, "GPU probe not installed");
    }
}
