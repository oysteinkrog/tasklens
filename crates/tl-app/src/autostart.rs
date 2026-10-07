//! The "Start at login" XDG autostart entry (E33-014, APP-048).
//!
//! `apply` writes `$XDG_CONFIG_HOME/autostart/io.github.oysteinkrog.Tasklens.desktop` (or
//! `<app_id>.desktop` with `--app-id`, see `Paths::autostart_file`) when the setting is on and
//! removes it when it is off. The settings saver calls it when
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

/// The entry text. `start_minimized` adds `--start-minimized` to `Exec`, and `app_id` adds
/// `--app-id <id>` so the entry starts the same instance. The id is validated, so it needs no
/// quoting.
pub fn entry_text(start_minimized: bool, app_id: Option<&str>) -> String {
    let mut exec = String::from("tasklens");
    if start_minimized {
        exec.push_str(" --start-minimized");
    }
    if let Some(id) = app_id {
        exec.push_str(" --app-id ");
        exec.push_str(id);
    }
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
pub fn apply(
    path: &Path,
    start_at_login: bool,
    start_minimized: bool,
    app_id: Option<&str>,
) -> io::Result<Change> {
    if !start_at_login {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(Change::Removed),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Change::Unchanged),
            Err(e) => Err(e),
        };
    }
    let text = entry_text(start_minimized, app_id);
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
            entry_text(false, None),
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
            entry_text(true, None),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=tasklens\n\
             Exec=tasklens --start-minimized\n\
             Icon=io.github.oysteinkrog.Tasklens\n\
             X-KDE-autostart-phase=2\n"
        );
    }

    #[test]
    fn entry_text_app_id() {
        assert_eq!(
            entry_text(true, Some("io.example.Dev")),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=tasklens\n\
             Exec=tasklens --start-minimized --app-id io.example.Dev\n\
             Icon=io.github.oysteinkrog.Tasklens\n\
             X-KDE-autostart-phase=2\n"
        );
        assert!(
            entry_text(false, Some("io.example.Dev"))
                .contains("\nExec=tasklens --app-id io.example.Dev\n")
        );
    }

    #[test]
    fn app_id_entry_leaves_real_entry() {
        let dir = TempDir::new().unwrap();
        let real = entry_path(&dir);
        apply(&real, true, false, None).unwrap();
        let dev = dir.path().join("autostart").join("io.example.Dev.desktop");

        assert_eq!(
            apply(&dev, true, false, Some("io.example.Dev")).unwrap(),
            Change::Written
        );
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            entry_text(false, None)
        );
        assert_eq!(
            apply(&dev, false, false, Some("io.example.Dev")).unwrap(),
            Change::Removed
        );
        assert!(real.exists());
    }

    #[test]
    fn apply_writes() {
        let dir = TempDir::new().unwrap();
        let path = entry_path(&dir);

        assert_eq!(apply(&path, true, false, None).unwrap(), Change::Written);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            entry_text(false, None)
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);

        // A changed `start_minimized` rewrites the file.
        assert_eq!(apply(&path, true, true, None).unwrap(), Change::Written);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            entry_text(true, None)
        );
    }

    #[test]
    fn apply_removes() {
        let dir = TempDir::new().unwrap();
        let path = entry_path(&dir);
        apply(&path, true, false, None).unwrap();

        assert_eq!(apply(&path, false, false, None).unwrap(), Change::Removed);
        assert!(!path.exists());
        assert_eq!(apply(&path, false, true, None).unwrap(), Change::Unchanged);
    }

    #[test]
    fn apply_unchanged_no_rewrite() {
        let dir = TempDir::new().unwrap();
        let path = entry_path(&dir);
        apply(&path, true, true, None).unwrap();
        let before = std::fs::metadata(&path).unwrap();

        assert_eq!(apply(&path, true, true, None).unwrap(), Change::Unchanged);
        let after = std::fs::metadata(&path).unwrap();
        // An atomic write replaces the inode, so the same inode and mtime mean no write.
        assert_eq!(after.ino(), before.ino());
        assert_eq!(after.mtime_nsec(), before.mtime_nsec());
        assert_eq!(after.mtime(), before.mtime());
    }
}
