//! KDE Plasma pre-startup and logout scripts as startup rows (SYS-019 sources 4 and 5).
//!
//! Plasma runs only executable scripts, so the execute bit is the enabled state (SYS-022).

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use smallvec::SmallVec;

use super::autostart::StartupEnv;
use super::{StartupKindRaw, StartupRaw, StartupStatusRaw};
use crate::error::CollectError;
use crate::sanitize::sanitize_text;

/// Pre-startup scripts, under `config_home`.
pub const PLASMA_ENV_DIR: &str = "plasma-workspace/env";
/// Logout scripts, under `config_home`.
pub const PLASMA_SHUTDOWN_DIR: &str = "plasma-workspace/shutdown";

/// Longest display name kept for a script, in bytes.
const MAX_NAME: usize = 4096;

/// Appends rows only when env.current_desktop contains "KDE".
///
/// Pre-startup rows are the `*.sh` regular files of [`PLASMA_ENV_DIR`]; logout rows are every
/// regular file of [`PLASMA_SHUTDOWN_DIR`]. Symlinks are followed. A missing directory is
/// skipped; an existing directory that cannot be listed returns the error.
pub fn scan_plasma_scripts(
    env: &StartupEnv,
    out: &mut Vec<StartupRaw>,
) -> Result<(), CollectError> {
    if !env.current_desktop.iter().any(|d| d == "KDE") || !env.config_home.is_absolute() {
        return Ok(());
    }
    scan_dir(
        &env.config_home.join(PLASMA_ENV_DIR),
        StartupKindRaw::PlasmaPreStartup,
        out,
    )?;
    scan_dir(
        &env.config_home.join(PLASMA_SHUTDOWN_DIR),
        StartupKindRaw::PlasmaLogout,
        out,
    )
}

fn scan_dir(
    dir: &Path,
    kind: StartupKindRaw,
    out: &mut Vec<StartupRaw>,
) -> Result<(), CollectError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    // Non-UTF-8 paths cannot be a row identity, so they are skipped.
    let Some(dir_str) = dir.to_str() else {
        return Ok(());
    };
    let location: Arc<str> = Arc::from(dir_str);
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| kind != StartupKindRaw::PlasmaPreStartup || name.ends_with(".sh"))
        .collect();
    names.sort_unstable();
    let mut text = String::new();
    for file_name in names {
        let path = dir.join(&file_name);
        // `metadata` follows symlinks, so a link to a regular file counts.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let Some(path_str) = path.to_str().filter(|_| meta.is_file()) else {
            continue;
        };
        let status = if meta.permissions().mode() & 0o111 != 0 {
            StartupStatusRaw::Enabled
        } else {
            StartupStatusRaw::Disabled
        };
        sanitize_text(file_name.as_bytes(), MAX_NAME, &mut text);
        let path_arc: Arc<str> = Arc::from(path_str);
        out.push(StartupRaw {
            kind,
            file_name: Arc::from(file_name),
            source_path: path_arc.clone(),
            location: location.clone(),
            name: Arc::from(text.as_str()),
            icon: None,
            command: path_arc.clone(),
            resolved_path: Some(path_arc),
            status,
            try_exec: None,
            only_show_in: SmallVec::new(),
            not_show_in: SmallVec::new(),
            kde_autostart_phase: None,
            generated_unit: None,
            overrides: None,
            overridden_by_user: false,
            tasklens_override: false,
            unit_file_state: None,
            service_type: None,
            can_toggle: true,
            can_delete: true,
            can_run: true,
            file_exists: true,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::Permissions;
    use std::path::PathBuf;

    fn kde_env(home: PathBuf) -> StartupEnv {
        StartupEnv {
            config_home: home,
            current_desktop: vec!["KDE".to_owned()],
            ..StartupEnv::default()
        }
    }

    fn write(path: &Path, mode: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    }

    fn scan(env: &StartupEnv) -> Vec<StartupRaw> {
        let mut out = Vec::new();
        scan_plasma_scripts(env, &mut out).unwrap();
        out
    }

    #[test]
    fn not_kde_no_rows() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("plasma-workspace/env/a.sh"), 0o755);
        let env = StartupEnv {
            current_desktop: vec!["GNOME".to_owned()],
            ..kde_env(dir.path().to_path_buf())
        };
        assert!(scan(&env).is_empty());
    }

    #[test]
    fn exec_bit_is_status() {
        let dir = tempfile::tempdir().unwrap();
        let env_dir = dir.path().join("plasma-workspace/env");
        write(&env_dir.join("b.sh"), 0o644);
        write(&env_dir.join("a.sh"), 0o755);
        write(&env_dir.join("readme.txt"), 0o755);
        std::fs::create_dir(env_dir.join("sub.sh")).unwrap();
        let rows = scan(&kde_env(dir.path().to_path_buf()));
        assert_eq!(rows.len(), 2);

        let a_path = env_dir.join("a.sh");
        let a = &rows[0];
        assert_eq!(a.kind, StartupKindRaw::PlasmaPreStartup);
        assert_eq!(&*a.file_name, "a.sh");
        assert_eq!(&*a.name, "a.sh");
        assert_eq!(a.status, StartupStatusRaw::Enabled);
        assert_eq!(Some(&*a.source_path), a_path.to_str());
        assert_eq!(&*a.command, &*a.source_path);
        assert_eq!(a.resolved_path.as_deref(), a_path.to_str());
        assert_eq!(Some(&*a.location), env_dir.to_str());
        assert!(a.can_toggle && a.can_delete && a.can_run && a.file_exists);

        assert_eq!(&*rows[1].file_name, "b.sh");
        assert_eq!(rows[1].status, StartupStatusRaw::Disabled);
    }

    #[test]
    fn logout_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(&home.join("plasma-workspace/shutdown/cleanup"), 0o700);
        write(&home.join("plasma-workspace/env/z.sh"), 0o755);
        // A symlink to a regular file is followed.
        write(&home.join("target-script"), 0o644);
        std::os::unix::fs::symlink(
            home.join("target-script"),
            home.join("plasma-workspace/shutdown/linked"),
        )
        .unwrap();
        let rows = scan(&kde_env(home.to_path_buf()));
        let kinds: Vec<_> = rows.iter().map(|r| (r.kind, &*r.file_name)).collect();
        assert_eq!(
            kinds,
            [
                (StartupKindRaw::PlasmaPreStartup, "z.sh"),
                (StartupKindRaw::PlasmaLogout, "cleanup"),
                (StartupKindRaw::PlasmaLogout, "linked"),
            ]
        );
        assert_eq!(rows[1].status, StartupStatusRaw::Enabled);
        assert_eq!(rows[2].status, StartupStatusRaw::Disabled);
    }

    #[test]
    fn missing_dirs_ok() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan(&kde_env(dir.path().to_path_buf())).is_empty());

        // An existing directory that cannot be listed is an error.
        let env_dir = dir.path().join("plasma-workspace/env");
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::set_permissions(&env_dir, Permissions::from_mode(0o000)).unwrap();
        // Root can list it anyway, so only check when the listing really fails.
        if std::fs::read_dir(&env_dir).is_err() {
            let mut out = Vec::new();
            let result = scan_plasma_scripts(&kde_env(dir.path().to_path_buf()), &mut out);
            assert!(matches!(result, Err(CollectError::PermissionDenied)));
        }
        std::fs::set_permissions(&env_dir, Permissions::from_mode(0o755)).unwrap();
    }
}
