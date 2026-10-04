//! Atomic file writes.
//!
//! `write_atomic` writes `<name>.tmp-<pid>` in the target's directory, syncs it, renames it over
//! the target and syncs the directory (ADR-011). A crash leaves either the old file or the new
//! one, never a partial file.

use std::ffi::OsString;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use crate::error::{OpsError, OpsResult};

/// The mode for settings, state and every other private file tasklens writes (ADR-011).
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// Replaces `path` with `bytes` atomically. The new file gets exactly `mode`, whatever the
/// umask. The parent directory must exist.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> OpsResult {
    let result = write_atomic_inner(path, bytes, mode);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    tracing::debug!(file = %name, ok = result.is_ok(), error = ?result.as_ref().err(), "atomic write");
    result
}

fn write_atomic_inner(path: &Path, bytes: &[u8], mode: u32) -> OpsResult {
    let tmp = temp_path(path)?;
    let mode = Mode::from_raw_mode(mode);
    let fd = create_temp(&tmp, mode)?;
    if let Err(e) = fill_and_rename(fd, &tmp, path, bytes, mode) {
        let _ = rustix::fs::unlink(&tmp);
        return Err(e);
    }
    // A bare file name has an empty parent, which means the current directory.
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let dir = rustix::fs::open(
        parent,
        OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    rustix::fs::fsync(&dir)?;
    Ok(())
}

/// Creates `tmp` exclusively. A leftover file from an earlier run with the same pid is removed
/// and the create is tried once more.
fn create_temp(tmp: &Path, mode: Mode) -> OpsResult<OwnedFd> {
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC;
    match rustix::fs::open(tmp, flags, mode) {
        Ok(fd) => Ok(fd),
        Err(Errno::EXIST) => {
            rustix::fs::unlink(tmp)?;
            Ok(rustix::fs::open(tmp, flags, mode)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Sets the mode, writes all of `bytes`, syncs and closes the file, then renames it over
/// `path`.
fn fill_and_rename(fd: OwnedFd, tmp: &Path, path: &Path, bytes: &[u8], mode: Mode) -> OpsResult {
    rustix::fs::fchmod(&fd, mode)?;
    let mut rest = bytes;
    while !rest.is_empty() {
        match rustix::io::write(&fd, rest) {
            Ok(0) => return Err(OpsError::Os(libc::EIO)),
            Ok(n) => rest = &rest[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    rustix::fs::fsync(&fd)?;
    drop(fd);
    rustix::fs::rename(tmp, path)?;
    Ok(())
}

/// `<parent>/<file name>.tmp-<pid>`.
pub(crate) fn temp_path(path: &Path) -> OpsResult<PathBuf> {
    let name = path
        .file_name()
        .ok_or(OpsError::InvalidInput("path has no file name"))?;
    let parent = path
        .parent()
        .ok_or(OpsError::InvalidInput("path has no parent"))?;
    let mut tmp_name = OsString::from(name);
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    Ok(parent.join(tmp_name))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("tl-ops-fs-test-{}-{n}", std::process::id()));
            std::fs::create_dir(&dir).expect("create test dir");
            TestDir(dir)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_files(dir: &Path) -> Vec<OsString> {
        std::fs::read_dir(dir)
            .expect("read test dir")
            .map(|e| e.expect("dir entry").file_name())
            .filter(|n| n.to_string_lossy().contains(".tmp-"))
            .collect()
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).expect("stat").permissions().mode() & 0o7777
    }

    #[test]
    fn temp_path_appends_pid() {
        let tmp = temp_path(Path::new("/a/settings.toml")).expect("temp path");
        let want = format!("/a/settings.toml.tmp-{}", std::process::id());
        assert_eq!(tmp, PathBuf::from(want));
    }

    #[test]
    fn temp_path_without_file_name_is_invalid() {
        assert_eq!(
            temp_path(Path::new("/")),
            Err(OpsError::InvalidInput("path has no file name"))
        );
        assert_eq!(
            temp_path(Path::new("/a/..")),
            Err(OpsError::InvalidInput("path has no file name"))
        );
    }

    #[test]
    fn write_atomic_creates_with_mode_and_no_temp_left() {
        let dir = TestDir::new();
        let target = dir.0.join("settings.toml");
        write_atomic(&target, b"a = 1\n", PRIVATE_FILE_MODE).expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), b"a = 1\n");
        assert_eq!(mode_of(&target), 0o600);
        assert!(temp_files(&dir.0).is_empty());
    }

    #[test]
    fn write_atomic_replaces_by_rename() {
        let dir = TestDir::new();
        let target = dir.0.join("settings.toml");
        std::fs::write(&target, b"old").expect("seed");
        let old_ino = std::fs::metadata(&target).expect("stat").ino();
        write_atomic(&target, b"new", PRIVATE_FILE_MODE).expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), b"new");
        assert_ne!(std::fs::metadata(&target).expect("stat").ino(), old_ino);
    }

    #[test]
    fn write_atomic_removes_stale_temp() {
        let dir = TestDir::new();
        let target = dir.0.join("settings.toml");
        let stale = temp_path(&target).expect("temp path");
        std::fs::write(&stale, b"stale").expect("seed stale");
        write_atomic(&target, b"a = 1\n", PRIVATE_FILE_MODE).expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), b"a = 1\n");
        assert!(!stale.exists());
        assert!(temp_files(&dir.0).is_empty());
    }

    #[test]
    fn write_atomic_missing_parent_is_enoent() {
        let dir = TestDir::new();
        let missing = dir.0.join("missing");
        let target = missing.join("settings.toml");
        assert_eq!(
            write_atomic(&target, b"x", PRIVATE_FILE_MODE),
            Err(OpsError::Os(libc::ENOENT))
        );
        assert!(!missing.exists());
        assert_eq!(std::fs::read_dir(&dir.0).expect("read dir").count(), 0);
    }

    #[test]
    fn write_atomic_onto_directory_cleans_temp() {
        let dir = TestDir::new();
        let target = dir.0.join("settings.toml");
        std::fs::create_dir(&target).expect("create target dir");
        assert_eq!(
            write_atomic(&target, b"x", PRIVATE_FILE_MODE),
            Err(OpsError::Os(libc::EISDIR))
        );
        assert!(target.is_dir());
        assert!(temp_files(&dir.0).is_empty());
    }

    #[test]
    fn write_atomic_mode_ignores_umask() {
        // The umask is process-wide, so the test does not change it. The `fchmod` step makes
        // the result exact whatever the umask is.
        let dir = TestDir::new();
        let target = dir.0.join("autostart.desktop");
        write_atomic(&target, b"[Desktop Entry]\n", 0o640).expect("write");
        assert_eq!(mode_of(&target), 0o640);
    }

    #[test]
    fn write_atomic_large_payload() {
        let dir = TestDir::new();
        let target = dir.0.join("themes.json");
        let payload: Vec<u8> = (0..4 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
        write_atomic(&target, &payload, PRIVATE_FILE_MODE).expect("write");
        assert_eq!(std::fs::read(&target).expect("read"), payload);
    }
}
