//! The "Start at login" XDG autostart entry (E33-014, APP-048).
//!
//! `apply` writes `$XDG_CONFIG_HOME/autostart/io.github.oysteinkrog.Tasklens.desktop` when the
//! setting is on and removes it when it is off. The settings saver calls it when
//! `startup.start_at_login` or `startup.start_minimized` changes; it is not called at start.

use std::io::{self, ErrorKind};
use std::path::Path;

use crate::atomic;

/// The entry is a normal desktop file, readable by the session manager.
const ENTRY_MODE: u32 = 0o644;

/// What `apply` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Written,
    Removed,
    Unchanged,
}

/// The entry text. `start_minimized` adds `--start-minimized` to `Exec`.
pub fn entry_text(start_minimized: bool) -> String {
    let exec = if start_minimized {
        "tasklens --start-minimized"
    } else {
        "tasklens"
    };
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=tasklens\n\
         Exec={exec}\n\
         Icon=io.github.oysteinkrog.Tasklens\n\
         X-KDE-autostart-phase=2\n"
    )
}

/// Writes the entry at `path` when `start_at_login` is on and the file is missing or differs.
/// Removes it when the setting is off. The `autostart` directory is shared with other apps, so
/// it is created when missing but its mode is never changed.
pub fn apply(path: &Path, start_at_login: bool, start_minimized: bool) -> io::Result<Change> {
    if !start_at_login {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(Change::Removed),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Change::Unchanged),
            Err(e) => Err(e),
        };
    }
    let text = entry_text(start_minimized);
    match std::fs::read(path) {
        Ok(current) if current == text.as_bytes() => return Ok(Change::Unchanged),
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    atomic::write_atomic(path, text.as_bytes(), ENTRY_MODE).map_err(io::Error::other)?;
    Ok(Change::Written)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;

    fn entry_path(dir: &TempDir) -> PathBuf {
        dir.path()
            .join("autostart")
            .join("io.github.oysteinkrog.Tasklens.desktop")
    }

    #[test]
    fn entry_text_plain() {
        assert_eq!(
            entry_text(false),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=tasklens\n\
             Exec=tasklens\n\
             Icon=io.github.oysteinkrog.Tasklens\n\
             X-KDE-autostart-phase=2\n"
        );
    }

    #[test]
    fn entry_text_minimized() {
        assert_eq!(
            entry_text(true),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=tasklens\n\
             Exec=tasklens --start-minimized\n\
             Icon=io.github.oysteinkrog.Tasklens\n\
             X-KDE-autostart-phase=2\n"
        );
    }

    #[test]
    fn apply_writes() {
        let dir = TempDir::new().unwrap();
        let path = entry_path(&dir);

        assert_eq!(apply(&path, true, false).unwrap(), Change::Written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), entry_text(false));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);

        // A changed `start_minimized` rewrites the file.
        assert_eq!(apply(&path, true, true).unwrap(), Change::Written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), entry_text(true));
    }

    #[test]
    fn apply_removes() {
        let dir = TempDir::new().unwrap();
        let path = entry_path(&dir);
        apply(&path, true, false).unwrap();

        assert_eq!(apply(&path, false, false).unwrap(), Change::Removed);
        assert!(!path.exists());
        assert_eq!(apply(&path, false, true).unwrap(), Change::Unchanged);
    }

    #[test]
    fn apply_unchanged_no_rewrite() {
        let dir = TempDir::new().unwrap();
        let path = entry_path(&dir);
        apply(&path, true, true).unwrap();
        let before = std::fs::metadata(&path).unwrap();

        assert_eq!(apply(&path, true, true).unwrap(), Change::Unchanged);
        let after = std::fs::metadata(&path).unwrap();
        // An atomic write replaces the inode, so the same inode and mtime mean no write.
        assert_eq!(after.ino(), before.ino());
        assert_eq!(after.mtime_nsec(), before.mtime_nsec());
        assert_eq!(after.mtime(), before.mtime());
    }
}
