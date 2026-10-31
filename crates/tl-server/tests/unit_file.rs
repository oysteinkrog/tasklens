//! `packaging/tasklens-engine.service` (ADR-018): the keys the ADR lists, and a lint with
//! `systemd-analyze --user verify` when it is available.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::Command;

fn unit_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packaging/tasklens-engine.service")
}

fn text(name: &str) -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging")
            .join(name),
    )
    .unwrap()
}

#[test]
fn the_unit_has_the_adr_keys() {
    let unit = std::fs::read_to_string(unit_path()).unwrap();
    for line in [
        "Type=notify",
        "ExecStart=/usr/bin/tasklens-engine",
        "Restart=on-failure",
        "RestartPreventExitStatus=0",
        "RestartSec=2",
        "TimeoutStopSec=10",
        "WantedBy=default.target",
    ] {
        assert!(unit.lines().any(|l| l == line), "missing {line}");
    }
}

#[test]
fn preset_and_pkgbuild_list_the_files() {
    assert_eq!(
        text("80-tasklens.preset"),
        "enable tasklens-engine.service\n"
    );
    let pkgbuild = text("PKGBUILD");
    for part in [
        "target/release/tasklens-engine",
        "/usr/bin/tasklens-engine",
        "packaging/tasklens-engine.service",
        "/usr/lib/systemd/user/tasklens-engine.service",
        "packaging/80-tasklens.preset",
        "/usr/lib/systemd/user-preset/80-tasklens.preset",
        "install=tasklens.install",
    ] {
        assert!(pkgbuild.contains(part), "PKGBUILD lacks {part}");
    }
    let install = text("tasklens.install");
    assert!(install.contains("systemctl --global preset tasklens-engine.service"));
    assert!(install.contains("post_upgrade"));
    assert!(install.contains("systemctl --global disable tasklens-engine.service"));
}

#[test]
fn systemd_analyze_accepts_the_unit_without_warnings() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipped: no XDG_RUNTIME_DIR");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let unit = std::fs::read_to_string(unit_path()).unwrap().replace(
        "ExecStart=/usr/bin/tasklens-engine",
        &format!("ExecStart={}", env!("CARGO_BIN_EXE_tasklens-engine")),
    );
    let path = dir.path().join("tasklens-engine.service");
    std::fs::write(&path, unit).unwrap();
    let out = match Command::new("systemd-analyze")
        .args(["--user", "verify"])
        .arg(&path)
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            eprintln!("skipped: systemd-analyze: {e}");
            return;
        }
    };
    let err = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{err}{stdout}");
    assert!(
        err.trim().is_empty() && stdout.trim().is_empty(),
        "{err}{stdout}"
    );
}
