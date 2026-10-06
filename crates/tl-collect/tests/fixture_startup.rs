//! Startup scan tests on the `autostart-kde` fixture (E06-A2).

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use smallvec::SmallVec;
use tl_collect::startup::{
    StartupEnv, StartupKindRaw, StartupRaw, StartupStatusRaw, UnitFileListing, scan_startup,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/autostart-kde")
}

fn fixture_path(rel: &str) -> String {
    root().join(rel).to_str().unwrap().to_owned()
}

fn fixture_env() -> StartupEnv {
    let root = root();
    StartupEnv {
        config_home: root.join("home/.config"),
        config_dirs: vec![root.join("etc/xdg"), root.join("etc/xdg2")],
        current_desktop: vec!["KDE".to_owned()],
        path_dirs: vec![root.join("usr/bin")],
        locales: SmallVec::new(),
    }
}

/// The unit listing the user manager would return for the fixture.
fn unit_paths() -> [(String, &'static str); 2] {
    [
        (
            fixture_path("home/.config/systemd/user/sync.service"),
            "enabled",
        ),
        (fixture_path("usr/lib/systemd/user/pkg.service"), "disabled"),
    ]
}

fn scan_into(out: &mut Vec<StartupRaw>) {
    let paths = unit_paths();
    let units: Vec<UnitFileListing<'_>> = paths
        .iter()
        .map(|(path, state)| UnitFileListing { path, state })
        .collect();
    scan_startup(&fixture_env(), &units, out).unwrap();
}

fn scan() -> Vec<StartupRaw> {
    let mut rows = Vec::new();
    scan_into(&mut rows);
    rows
}

/// `path` relative to the fixture root, so the snapshot does not depend on the checkout path.
fn relative(path: &str) -> String {
    Path::new(path)
        .strip_prefix(root())
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

fn render(rows: &[StartupRaw]) -> String {
    let mut text = String::new();
    for r in rows {
        writeln!(
            text,
            "{:?} {} name={:?} status={:?} overrides={:?} overridden_by_user={} \
             generated_unit={:?} can_toggle={} can_delete={}",
            r.kind,
            r.file_name,
            &*r.name,
            r.status,
            r.overrides.as_deref().map(relative),
            r.overridden_by_user,
            r.generated_unit.as_deref(),
            r.can_toggle,
            r.can_delete,
        )
        .unwrap();
    }
    text
}

#[test]
fn fixture_autostart_kde_snapshot() {
    insta::assert_snapshot!("autostart_kde", render(&scan()));
}

#[test]
fn shadowed_system_dir_dropped() {
    let rows = scan();
    let shadowed = fixture_path("etc/xdg2/autostart/org.kde.kalendarac.desktop");
    assert!(rows.iter().all(|r| *r.source_path != *shadowed));
    let kalendarac: Vec<_> = rows
        .iter()
        .filter(|r| &*r.file_name == "org.kde.kalendarac.desktop")
        .collect();
    assert_eq!(kalendarac.len(), 1);
    assert_eq!(
        &*kalendarac[0].source_path,
        fixture_path("etc/xdg/autostart/org.kde.kalendarac.desktop")
    );
}

#[test]
fn tasklens_override_pair() {
    let rows = scan();
    let yakuake: Vec<_> = rows
        .iter()
        .filter(|r| &*r.file_name == "org.kde.yakuake.desktop")
        .collect();
    assert_eq!(yakuake.len(), 2);

    let system = fixture_path("etc/xdg/autostart/org.kde.yakuake.desktop");
    let user = yakuake[0];
    assert_eq!(user.kind, StartupKindRaw::AutostartUser);
    assert_eq!(user.status, StartupStatusRaw::Disabled);
    assert!(user.tasklens_override);
    assert_eq!(user.overrides.as_deref(), Some(system.as_str()));
    assert_eq!(
        user.resolved_path.as_deref(),
        Some(fixture_path("usr/bin/yakuake").as_str())
    );

    let overridden = yakuake[1];
    assert_eq!(overridden.kind, StartupKindRaw::AutostartSystemOverridden);
    assert!(overridden.overridden_by_user);
    assert_eq!(&*overridden.source_path, system);
    for r in yakuake {
        assert_eq!(
            r.generated_unit.as_deref(),
            Some("app-org.kde.yakuake@autostart.service")
        );
    }
}

#[test]
fn scan_clears_out() {
    let once = scan();
    let mut rows = Vec::new();
    scan_into(&mut rows);
    scan_into(&mut rows);
    assert_eq!(rows, once);
}
