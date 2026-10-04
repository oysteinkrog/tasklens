//! Tests for XDG path resolution and directory modes (E33-002).

#[allow(dead_code, reason = "the tests use only part of the module")]
#[path = "../src/paths.rs"]
mod paths;

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use paths::{DIR_MODE, Paths, ensure_dir};

fn resolve(vars: &[(&str, &str)]) -> anyhow::Result<Paths> {
    let vars: Vec<(String, OsString)> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
        .collect();
    Paths::from_vars(|name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn config_home_set() {
    let p = resolve(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/x/cfg")]).unwrap();
    assert_eq!(
        p.settings_file(),
        Path::new("/x/cfg/tasklens/settings.toml")
    );
    assert_eq!(p.themes_file(), Path::new("/x/cfg/tasklens/themes.json"));
}

#[test]
fn state_home_default() {
    let p = resolve(&[("HOME", "/home/u")]).unwrap();
    assert_eq!(
        p.state_file(),
        Path::new("/home/u/.local/state/tasklens/state.toml")
    );
    assert_eq!(p.home, Path::new("/home/u"));
}

#[test]
fn cache_home_empty_falls_back() {
    let p = resolve(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "")]).unwrap();
    assert_eq!(
        p.icon_cache_file(),
        Path::new("/home/u/.cache/tasklens/icons.toml")
    );
}

#[test]
fn relative_xdg_ignored() {
    let p = resolve(&[
        ("HOME", "/home/u"),
        ("XDG_CONFIG_HOME", "relative/dir"),
        ("XDG_STATE_HOME", "rel"),
        ("XDG_CACHE_HOME", "./c"),
    ])
    .unwrap();
    assert_eq!(p.config_dir, Path::new("/home/u/.config/tasklens"));
    assert_eq!(p.state_dir, Path::new("/home/u/.local/state/tasklens"));
    assert_eq!(p.cache_dir, Path::new("/home/u/.cache/tasklens"));
}

#[test]
fn home_missing_is_error() {
    for vars in [
        &[][..],
        &[("HOME", "")][..],
        &[("HOME", "home/u"), ("XDG_CONFIG_HOME", "/x/cfg")][..],
    ] {
        let err = resolve(vars).unwrap_err();
        assert_eq!(err.to_string(), "HOME is not set to an absolute path");
    }
}

#[test]
fn autostart_path() {
    let p = resolve(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/x/cfg")]).unwrap();
    assert_eq!(
        p.autostart_file(),
        Path::new("/x/cfg/autostart/io.github.oysteinkrog.Tasklens.desktop")
    );
    let p = resolve(&[("HOME", "/home/u")]).unwrap();
    assert_eq!(
        p.autostart_file(),
        Path::new("/home/u/.config/autostart/io.github.oysteinkrog.Tasklens.desktop")
    );
}

#[test]
fn ensure_dir_creates_0700() {
    let tmp = tempfile::TempDir::new().unwrap();
    let a = tmp.path().join("a");
    let b = a.join("b");
    let c = b.join("c");
    ensure_dir(&c).unwrap();
    for dir in [&a, &b, &c] {
        assert!(dir.is_dir());
        assert_eq!(mode(dir), DIR_MODE, "{}", dir.display());
    }
    // A second call on existing directories succeeds and changes nothing.
    ensure_dir(&c).unwrap();
    assert_eq!(mode(&c), DIR_MODE);
}

#[test]
fn ensure_dir_keeps_existing_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let existing = tmp.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
    let child = existing.join("child");
    ensure_dir(&child).unwrap();
    assert_eq!(mode(&existing), 0o755);
    assert_eq!(mode(&child), DIR_MODE);
    ensure_dir(&existing).unwrap();
    assert_eq!(mode(&existing), 0o755);
}

#[test]
fn all_file_names() {
    let p = resolve(&[("HOME", "/home/u")]).unwrap();
    let cases = [
        (p.settings_file(), "settings.toml"),
        (p.themes_file(), "themes.json"),
        (p.state_file(), "state.toml"),
        (p.logs_dir(), "logs"),
        (p.crash_dir(), "crash"),
        (p.history_dir(), "history"),
        (p.dumps_dir(), "dumps"),
        (p.run_history_file(), "run-history.json"),
        (p.icon_cache_file(), "icons.toml"),
        (p.autostart_file(), "io.github.oysteinkrog.Tasklens.desktop"),
    ];
    for (path, name) in cases {
        assert_eq!(path.file_name().unwrap(), name, "{}", path.display());
    }
}
