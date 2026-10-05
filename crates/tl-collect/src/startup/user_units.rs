//! systemd user units as startup rows (SYS-019 source 3), and systemd unit-name escaping.
//!
//! The engine lists the unit files over D-Bus (E14) and passes the paths here. This module only
//! reads those files. Unit files are untrusted input: any program can drop one in
//! `~/.config/systemd/user`. The parser reads at most [`MAX_UNIT_FILE`] bytes, never panics, and
//! passes every string it keeps through [`crate::sanitize`] (ADR-015).

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use rustix::fs::{Mode, OFlags};
use smallvec::SmallVec;

use super::autostart::{StartupEnv, find_on_path, first_word};
use super::{StartupKindRaw, StartupRaw, StartupStatusRaw};
use crate::sanitize::sanitize_text;

/// Read limit for one unit file, in bytes.
pub const MAX_UNIT_FILE: usize = 64 * 1024;

/// One entry of ListUnitFilesByPatterns on the user manager.
pub struct UnitFileListing<'a> {
    pub path: &'a str,
    pub state: &'a str,
}

/// The keys of one unit file that startup rows need.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitFile {
    /// `[Install] WantedBy=`, accumulated; an empty value resets the list.
    pub wanted_by: SmallVec<[Arc<str>; 2]>,
    /// The first non-empty `[Service] ExecStart=`, with its prefix characters removed.
    pub exec_start: Option<Arc<str>>,
    /// The last `[Service] Type=`; an empty value resets it.
    pub service_type: Option<Arc<str>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Service,
    Install,
    Other,
}

/// Parses the `[Service]` and `[Install]` keys of a unit file. `#` and `;` lines are comments and
/// a trailing `\` joins a line with the next one, as in systemd.
pub fn parse_unit_file(bytes: &[u8]) -> UnitFile {
    let bytes = &bytes[..bytes.len().min(MAX_UNIT_FILE)];
    let mut unit = UnitFile::default();
    let mut section = Section::Other;
    let mut logical = Vec::new();
    let mut text = String::new();
    for raw in bytes.split(|&b| b == b'\n') {
        let line = raw.trim_ascii();
        // systemd skips comment lines inside a continued line too.
        if matches!(line.first(), Some(b'#' | b';')) {
            continue;
        }
        if let Some(head) = line.strip_suffix(b"\\") {
            logical.extend_from_slice(head);
            logical.push(b' ');
            continue;
        }
        logical.extend_from_slice(line);
        parse_line(&logical, &mut section, &mut unit, &mut text);
        logical.clear();
    }
    parse_line(&logical, &mut section, &mut unit, &mut text);
    unit
}

fn parse_line(line: &[u8], section: &mut Section, unit: &mut UnitFile, text: &mut String) {
    let line = line.trim_ascii();
    if line.is_empty() {
        return;
    }
    if line.starts_with(b"[") && line.ends_with(b"]") {
        *section = match line {
            b"[Service]" => Section::Service,
            b"[Install]" => Section::Install,
            _ => Section::Other,
        };
        return;
    }
    let Some(eq) = line.iter().position(|&b| b == b'=') else {
        return;
    };
    let key = line[..eq].trim_ascii();
    let value = line[eq + 1..].trim_ascii();
    match (*section, key) {
        (Section::Install, b"WantedBy") => {
            if value.is_empty() {
                unit.wanted_by.clear();
            }
            for item in value
                .split(u8::is_ascii_whitespace)
                .filter(|i| !i.is_empty())
            {
                sanitize_text(item, MAX_UNIT_FILE, text);
                unit.wanted_by.push(Arc::from(text.as_str()));
            }
        }
        (Section::Service, b"ExecStart") if unit.exec_start.is_none() => {
            let start = value
                .iter()
                .position(|b| !matches!(b, b'@' | b'-' | b':' | b'+' | b'!'))
                .unwrap_or(value.len());
            let command = value[start..].trim_ascii_start();
            if !command.is_empty() {
                sanitize_text(command, MAX_UNIT_FILE, text);
                unit.exec_start = Some(Arc::from(text.as_str()));
            }
        }
        (Section::Service, b"Type") => {
            unit.service_type = (!value.is_empty()).then(|| {
                sanitize_text(value, MAX_UNIT_FILE, text);
                Arc::from(text.as_str())
            });
        }
        _ => {}
    }
}

/// default.target, graphical-session.target or plasma-*.target.
fn is_startup_target(target: &str) -> bool {
    target == "default.target"
        || target == "graphical-session.target"
        || target
            .strip_prefix("plasma-")
            .and_then(|rest| rest.strip_suffix(".target"))
            .is_some_and(|middle| !middle.is_empty())
}

/// Reads each listed unit file and appends a `UserUnit` row for every unit whose [Install]
/// WantedBy= names default.target, graphical-session.target or a target matching plasma-*.target.
/// Generated autostart units are skipped: they are linked to their desktop entry instead. A
/// listed file that cannot be read gives a row with `file_exists = false`.
pub fn scan_user_units(units: &[UnitFileListing<'_>], env: &StartupEnv, out: &mut Vec<StartupRaw>) {
    let user_dir = env.config_home.join("systemd/user");
    for listing in units {
        let path = Path::new(listing.path);
        let Some(unit_name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if is_generated_autostart_unit(unit_name) {
            continue;
        }
        let unit = match read_unit_file(path) {
            Some(bytes) => {
                let unit = parse_unit_file(&bytes);
                if !unit.wanted_by.iter().any(|t| is_startup_target(t)) {
                    continue;
                }
                Some(unit)
            }
            None => None,
        };
        let status = match listing.state {
            "enabled" | "enabled-runtime" => StartupStatusRaw::Enabled,
            "masked" | "masked-runtime" => StartupStatusRaw::Masked,
            _ => StartupStatusRaw::Disabled,
        };
        let file_exists = unit.is_some();
        let (command, service_type) = match unit {
            Some(unit) => (unit.exec_start, unit.service_type),
            None => (None, None),
        };
        let resolved_path = command
            .as_deref()
            .and_then(first_word)
            .and_then(|program| {
                if Path::new(&program).is_absolute() {
                    Some(program)
                } else {
                    find_on_path(&program, &env.path_dirs)
                        .and_then(|p| p.into_os_string().into_string().ok())
                }
            })
            .map(Arc::from);
        let name: Arc<str> = Arc::from(unit_name);
        out.push(StartupRaw {
            kind: StartupKindRaw::UserUnit,
            file_name: name.clone(),
            source_path: Arc::from(listing.path),
            location: Arc::from(path.parent().and_then(Path::to_str).unwrap_or("")),
            name,
            icon: None,
            command: command.unwrap_or_else(|| Arc::from("")),
            resolved_path,
            status,
            try_exec: None,
            only_show_in: SmallVec::new(),
            not_show_in: SmallVec::new(),
            kde_autostart_phase: None,
            generated_unit: None,
            overrides: None,
            overridden_by_user: false,
            tasklens_override: false,
            unit_file_state: Some(Arc::from(listing.state)),
            service_type,
            can_toggle: status != StartupStatusRaw::Masked,
            can_delete: env.config_home.is_absolute()
                && path.starts_with(&user_dir)
                && path != user_dir,
            can_run: true,
            file_exists,
        });
    }
}

/// Reads at most [`MAX_UNIT_FILE`] bytes of `path` if it is a regular file after following
/// symlinks. Opening non-blocking keeps a FIFO from stalling the scan.
fn read_unit_file(path: &Path) -> Option<Vec<u8>> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK;
    let file = File::from(rustix::fs::open(path, flags, Mode::empty()).ok()?);
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_UNIT_FILE as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// systemd unit-name escaping (systemd `unit_name_escape`): `/` becomes `-`, and every byte
/// outside `[A-Za-z0-9:_.]`, and a leading `.`, becomes `\xNN` with lowercase hex.
pub fn systemd_escape(s: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, &b) in s.as_bytes().iter().enumerate() {
        match b {
            b'/' => out.push('-'),
            b'.' if i == 0 => out.push_str("\\x2e"),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b':' | b'_' | b'.' => out.push(char::from(b)),
            _ => {
                out.push_str("\\x");
                out.push(char::from(HEX[usize::from(b >> 4)]));
                out.push(char::from(HEX[usize::from(b & 0xf)]));
            }
        }
    }
}

/// "app-" + systemd_escape(desktop id without ".desktop") + "@autostart.service"
pub fn autostart_unit_name(desktop_file_name: &str) -> String {
    let id = desktop_file_name
        .strip_suffix(".desktop")
        .unwrap_or(desktop_file_name);
    let mut name = String::with_capacity(id.len() + 24);
    name.push_str("app-");
    systemd_escape(id, &mut name);
    name.push_str("@autostart.service");
    name
}

/// A unit that systemd-xdg-autostart-generator made from a desktop entry.
pub fn is_generated_autostart_unit(unit_name: &str) -> bool {
    unit_name.starts_with("app-") && unit_name.ends_with("@autostart.service")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/autostart-kde")
    }

    fn fixture_path(rel: &str) -> String {
        fixture_root().join(rel).to_str().unwrap().to_owned()
    }

    fn fixture_env() -> StartupEnv {
        StartupEnv {
            config_home: fixture_root().join("home/.config"),
            ..StartupEnv::default()
        }
    }

    fn scan(listing: &[(&str, &str)]) -> Vec<StartupRaw> {
        let paths: Vec<(String, &str)> = listing
            .iter()
            .map(|&(rel, state)| (fixture_path(rel), state))
            .collect();
        let units: Vec<UnitFileListing<'_>> = paths
            .iter()
            .map(|(path, state)| UnitFileListing { path, state })
            .collect();
        let mut rows = Vec::new();
        scan_user_units(&units, &fixture_env(), &mut rows);
        rows
    }

    #[test]
    fn wanted_by_graphical() {
        let rows = scan(&[("usr/lib/systemd/user/foo.service", "enabled")]);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.kind, StartupKindRaw::UserUnit);
        assert_eq!(row.status, StartupStatusRaw::Enabled);
        assert_eq!(&*row.name, "foo.service");
        assert_eq!(&*row.file_name, "foo.service");
        assert_eq!(
            &*row.source_path,
            fixture_path("usr/lib/systemd/user/foo.service")
        );
        assert_eq!(&*row.location, fixture_path("usr/lib/systemd/user"));
        assert_eq!(row.unit_file_state.as_deref(), Some("enabled"));
        assert_eq!(row.service_type.as_deref(), Some("simple"));
        assert!(row.can_toggle && row.can_run && row.file_exists);
    }

    #[test]
    fn wanted_by_other_ignored() {
        assert!(scan(&[("usr/lib/systemd/user/bar.service", "enabled")]).is_empty());
    }

    #[test]
    fn masked_not_toggleable() {
        let rows = scan(&[("usr/lib/systemd/user/baz.service", "masked")]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, StartupStatusRaw::Masked);
        assert!(!rows[0].can_toggle);
        let rows = scan(&[("usr/lib/systemd/user/baz.service", "disabled")]);
        assert_eq!(rows[0].status, StartupStatusRaw::Disabled);
        assert!(rows[0].can_toggle);
    }

    #[test]
    fn wanted_by_reset() {
        let unit =
            parse_unit_file(b"[Install]\nWantedBy=a.target\nWantedBy=\nWantedBy=default.target\n");
        let list: Vec<&str> = unit.wanted_by.iter().map(|s| &**s).collect();
        assert_eq!(list, ["default.target"]);
        // The fixture adds a commented-out WantedBy and a continued ExecStart.
        let rows = scan(&[("usr/lib/systemd/user/qux.service", "enabled-runtime")]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, StartupStatusRaw::Enabled);
        assert_eq!(&*rows[0].command, "/usr/bin/qux  --flag");
        // WantedBy outside [Install] does not count.
        let unit = parse_unit_file(b"[Unit]\nWantedBy=default.target\n");
        assert!(unit.wanted_by.is_empty());
    }

    #[test]
    fn exec_prefix_stripped() {
        let rows = scan(&[("usr/lib/systemd/user/foo.service", "enabled")]);
        assert_eq!(&*rows[0].command, "/usr/bin/foo --x");
        assert_eq!(rows[0].resolved_path.as_deref(), Some("/usr/bin/foo"));
        let unit = parse_unit_file(b"[Service]\nExecStart=!!-+:/bin/x\n");
        assert_eq!(unit.exec_start.as_deref(), Some("/bin/x"));
        let rows = scan(&[("usr/lib/systemd/user/baz.service", "enabled")]);
        assert_eq!(&*rows[0].command, "/usr/bin/baz baz --session");
    }

    #[test]
    fn delete_only_config_home() {
        let rows = scan(&[
            ("usr/lib/systemd/user/foo.service", "enabled"),
            ("home/.config/systemd/user/foo.service", "enabled"),
        ]);
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].can_delete);
        assert!(rows[1].can_delete);
    }

    #[test]
    fn escape_desktop_id() {
        assert_eq!(
            autostart_unit_name("org.kde.kalendarac.desktop"),
            "app-org.kde.kalendarac@autostart.service"
        );
        assert_eq!(
            autostart_unit_name("my-app.desktop"),
            "app-my\\x2dapp@autostart.service"
        );
        let mut out = String::new();
        systemd_escape(".a/b c\u{e9}", &mut out);
        assert_eq!(out, "\\x2ea-b\\x20c\\xc3\\xa9");
        assert!(is_generated_autostart_unit(&autostart_unit_name(
            "x.desktop"
        )));
    }

    #[test]
    fn generated_unit_skipped() {
        let rows = scan(&[(
            "usr/lib/systemd/user/app-org.kde.foo@autostart.service",
            "enabled",
        )]);
        assert!(rows.is_empty());
    }

    #[test]
    fn unreadable_file_kept() {
        let rows = scan(&[("usr/lib/systemd/user/missing.service", "disabled")]);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].file_exists);
        assert_eq!(&*rows[0].command, "");
        assert_eq!(rows[0].resolved_path, None);
    }
}
