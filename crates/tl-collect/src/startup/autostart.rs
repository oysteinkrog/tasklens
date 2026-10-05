//! Desktop Entry parser for XDG autostart files (SYS-019, SYS-020, SYS-026).
//!
//! Autostart files are untrusted input: any program can drop one in `~/.config/autostart`. The
//! parser reads at most [`MAX_DESKTOP_FILE`] bytes, never panics, and passes every string it keeps
//! through [`crate::sanitize`] (ADR-015). It only reads; writing overrides lives in `tl-ops`.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{Mode, OFlags};
use smallvec::SmallVec;

use super::{StartupKindRaw, StartupRaw, StartupStatusRaw};
use crate::CollectError;
use crate::sanitize::sanitize_text;

/// Keys of the `[Desktop Entry]` group of one autostart file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopEntry {
    /// `Name`, localised with the caller's match list.
    pub name: Option<Arc<str>>,
    /// `Exec`, unescaped.
    pub exec: Option<Arc<str>>,
    /// `TryExec`.
    pub try_exec: Option<Arc<str>>,
    /// `Icon`: a name or a path, resolved later.
    pub icon: Option<Arc<str>>,
    /// `Path`: the working directory.
    pub path: Option<Arc<str>>,
    /// `Type`.
    pub entry_type: Option<Arc<str>>,
    /// `Hidden=true`.
    pub hidden: bool,
    /// `X-GNOME-Autostart-enabled`.
    pub gnome_autostart_enabled: Option<bool>,
    /// `OnlyShowIn`, split on `;`, empty items dropped.
    pub only_show_in: SmallVec<[Arc<str>; 2]>,
    /// `NotShowIn`, split on `;`, empty items dropped.
    pub not_show_in: SmallVec<[Arc<str>; 2]>,
    /// `X-KDE-autostart-phase`.
    pub kde_autostart_phase: Option<Arc<str>>,
    /// `X-Tasklens-Override=true`.
    pub tasklens_override: bool,
}

/// Read limit for one `.desktop` file, in bytes (R-164).
pub const MAX_DESKTOP_FILE: usize = 64 * 1024;

const GROUP: &[u8] = b"[Desktop Entry]";

// Bits in the "seen" mask, so the first of duplicate keys wins.
const K_NAME: u16 = 1 << 0;
const K_EXEC: u16 = 1 << 1;
const K_TRY_EXEC: u16 = 1 << 2;
const K_ICON: u16 = 1 << 3;
const K_PATH: u16 = 1 << 4;
const K_TYPE: u16 = 1 << 5;
const K_HIDDEN: u16 = 1 << 6;
const K_GNOME: u16 = 1 << 7;
const K_ONLY: u16 = 1 << 8;
const K_NOT: u16 = 1 << 9;
const K_KDE_PHASE: u16 = 1 << 10;
const K_OVERRIDE: u16 = 1 << 11;

/// Parses the `[Desktop Entry]` group of `bytes`. `locales` is the caller's match list, most
/// specific first, for example ["de_DE@euro", "de_DE", "de@euro", "de"].
pub fn parse_desktop_entry(bytes: &[u8], locales: &[&str]) -> DesktopEntry {
    let bytes = &bytes[..bytes.len().min(MAX_DESKTOP_FILE)];
    let mut entry = DesktopEntry::default();
    let mut seen = 0u16;
    // Index into `locales` of the best `Name[..]` found so far, and its value.
    let mut local_name: Option<(usize, Arc<str>)> = None;
    let mut in_group = false;
    let mut scratch = Vec::new();
    let mut text = String::new();

    let mut start = 0;
    while start < bytes.len() {
        let end = memchr::memchr(b'\n', &bytes[start..]).map_or(bytes.len(), |i| start + i);
        let line = bytes[start..end].trim_ascii();
        start = end + 1;

        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if line[0] == b'[' {
            if in_group {
                break;
            }
            in_group = line == GROUP;
            continue;
        }
        if !in_group {
            continue;
        }
        let Some(eq) = memchr::memchr(b'=', line) else {
            continue;
        };
        let key = line[..eq].trim_ascii_end();
        let value = line[eq + 1..].trim_ascii_start();

        if let Some(locale) = key
            .strip_prefix(b"Name[")
            .and_then(|k| k.strip_suffix(b"]"))
        {
            let Some(rank) = locales.iter().position(|l| l.as_bytes() == locale) else {
                continue;
            };
            if local_name.as_ref().is_none_or(|(best, _)| rank < *best) {
                local_name = Some((rank, string_value(value, &mut scratch, &mut text)));
            }
            continue;
        }

        let bit = match key {
            b"Name" => K_NAME,
            b"Exec" => K_EXEC,
            b"TryExec" => K_TRY_EXEC,
            b"Icon" => K_ICON,
            b"Path" => K_PATH,
            b"Type" => K_TYPE,
            b"Hidden" => K_HIDDEN,
            b"X-GNOME-Autostart-enabled" => K_GNOME,
            b"OnlyShowIn" => K_ONLY,
            b"NotShowIn" => K_NOT,
            b"X-KDE-autostart-phase" => K_KDE_PHASE,
            b"X-Tasklens-Override" => K_OVERRIDE,
            _ => continue,
        };
        if seen & bit != 0 {
            continue;
        }
        seen |= bit;

        match bit {
            K_NAME => entry.name = Some(string_value(value, &mut scratch, &mut text)),
            K_EXEC => entry.exec = Some(string_value(value, &mut scratch, &mut text)),
            K_TRY_EXEC => entry.try_exec = Some(string_value(value, &mut scratch, &mut text)),
            K_ICON => entry.icon = Some(string_value(value, &mut scratch, &mut text)),
            K_PATH => entry.path = Some(string_value(value, &mut scratch, &mut text)),
            K_TYPE => entry.entry_type = Some(string_value(value, &mut scratch, &mut text)),
            K_KDE_PHASE => {
                entry.kde_autostart_phase = Some(string_value(value, &mut scratch, &mut text));
            }
            K_HIDDEN => entry.hidden = bool_value(value) == Some(true),
            K_GNOME => entry.gnome_autostart_enabled = bool_value(value),
            K_OVERRIDE => entry.tasklens_override = bool_value(value) == Some(true),
            K_ONLY => list_value(value, &mut entry.only_show_in, &mut scratch, &mut text),
            K_NOT => list_value(value, &mut entry.not_show_in, &mut scratch, &mut text),
            _ => {}
        }
    }

    if let Some((_, name)) = local_name {
        entry.name = Some(name);
    }
    entry
}

/// Builds the match list from a POSIX locale string such as "de_DE.UTF-8@euro" (encoding dropped).
pub fn locale_match_list(locale: &str) -> SmallVec<[String; 4]> {
    let (rest, modifier) = match locale.split_once('@') {
        Some((rest, m)) if !m.is_empty() => (rest, Some(m)),
        Some((rest, _)) => (rest, None),
        None => (locale, None),
    };
    let rest = rest.split_once('.').map_or(rest, |(r, _)| r);
    let (lang, country) = match rest.split_once('_') {
        Some((l, c)) if !c.is_empty() => (l, Some(c)),
        Some((l, _)) => (l, None),
        None => (rest, None),
    };

    let mut out = SmallVec::new();
    if lang.is_empty() {
        return out;
    }
    if let (Some(c), Some(m)) = (country, modifier) {
        out.push(format!("{lang}_{c}@{m}"));
    }
    if let Some(c) = country {
        out.push(format!("{lang}_{c}"));
    }
    if let Some(m) = modifier {
        out.push(format!("{lang}@{m}"));
    }
    out.push(lang.to_owned());
    out
}

/// The environment the autostart scan reads (ADR-011 fallback rules).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StartupEnv {
    /// `$XDG_CONFIG_HOME`, else `$HOME/.config`.
    pub config_home: PathBuf,
    /// `$XDG_CONFIG_DIRS` split on `:`, else ["/etc/xdg"].
    pub config_dirs: Vec<PathBuf>,
    /// `$XDG_CURRENT_DESKTOP` split on `:`.
    pub current_desktop: Vec<String>,
    /// `$PATH` split on `:`.
    pub path_dirs: Vec<PathBuf>,
    /// [`locale_match_list`] of `$LC_ALL`, else `$LC_MESSAGES`, else `$LANG`.
    pub locales: SmallVec<[String; 4]>,
}

impl StartupEnv {
    /// Reads the process environment. A variable that is unset, empty or not absolute uses the
    /// default; relative entries in `XDG_CONFIG_DIRS` and `PATH` are dropped.
    pub fn from_env() -> Self {
        Self::from_vars(|key| std::env::var_os(key))
    }

    fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Self {
        // Without an absolute HOME there is no user directory; the scan skips a relative one.
        let config_home = absolute(var("XDG_CONFIG_HOME"))
            .or_else(|| absolute(var("HOME")).map(|home| home.join(".config")))
            .unwrap_or_default();
        let mut config_dirs = absolute_list(var("XDG_CONFIG_DIRS"));
        if config_dirs.is_empty() {
            config_dirs.push(PathBuf::from("/etc/xdg"));
        }
        let current_desktop = var("XDG_CURRENT_DESKTOP")
            .and_then(|v| v.into_string().ok())
            .map(|v| {
                v.split(':')
                    .filter(|d| !d.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let locales = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(&var)
            .find(|v| !v.is_empty())
            .and_then(|v| v.into_string().ok())
            .map(|v| locale_match_list(&v))
            .unwrap_or_default();
        Self {
            config_home,
            config_dirs,
            current_desktop,
            path_dirs: absolute_list(var("PATH")),
            locales,
        }
    }
}

fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|p| p.is_absolute())
}

fn absolute_list(value: Option<OsString>) -> Vec<PathBuf> {
    let Some(value) = value else {
        return Vec::new();
    };
    value
        .as_bytes()
        .split(|&b| b == b':')
        .map(|item| PathBuf::from(OsStr::from_bytes(item)))
        .filter(|p| p.is_absolute())
        .collect()
}

/// One `.desktop` file that passed the filters.
struct Found {
    path: Arc<str>,
    dir: Arc<str>,
    entry: DesktopEntry,
}

/// Scans the user and system autostart directories and appends one row per effective file, plus
/// one `AutostartSystemOverridden` row per system file a user file shadows (SYS-019). User rows
/// come first, then system rows, each sorted by file name. Only a user directory that exists but
/// cannot be listed is an error; missing directories and unreadable files are skipped.
pub fn scan_autostart(env: &StartupEnv, out: &mut Vec<StartupRaw>) -> Result<(), CollectError> {
    let locales: SmallVec<[&str; 4]> = env.locales.iter().map(String::as_str).collect();

    let mut user = BTreeMap::new();
    if env.config_home.is_absolute() {
        let dir = env.config_home.join("autostart");
        match std::fs::read_dir(&dir) {
            Ok(entries) => read_autostart_dir(&dir, entries, &locales, &mut user),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
            Err(e) => return Err(e.into()),
        }
    }

    // The first system directory that has a file name wins; later ones are never read for it.
    let mut system = BTreeMap::new();
    for base in env.config_dirs.iter().filter(|d| d.is_absolute()) {
        let dir = base.join("autostart");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            read_autostart_dir(&dir, entries, &locales, &mut system);
        }
    }

    for (file_name, found) in &user {
        let overrides = system.get(file_name).map(|s: &Found| s.path.clone());
        out.push(make_row(
            env,
            StartupKindRaw::AutostartUser,
            file_name,
            found,
            overrides,
        ));
    }
    for (file_name, found) in &system {
        let kind = if user.contains_key(file_name) {
            StartupKindRaw::AutostartSystemOverridden
        } else {
            StartupKindRaw::AutostartSystem
        };
        out.push(make_row(env, kind, file_name, found, None));
    }
    Ok(())
}

/// Adds every readable `*.desktop` regular file of `dir` whose name `map` does not have yet.
fn read_autostart_dir(
    dir: &Path,
    entries: std::fs::ReadDir,
    locales: &[&str],
    map: &mut BTreeMap<String, Found>,
) {
    // Non-UTF-8 paths cannot be a row identity, so they are skipped.
    let Some(dir_str) = dir.to_str() else {
        return;
    };
    let dir_arc: Arc<str> = Arc::from(dir_str);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.len() <= ".desktop".len() || !name.ends_with(".desktop") || map.contains_key(name) {
            continue;
        }
        let path = dir.join(name);
        let (Some(path_str), Some(bytes)) = (path.to_str(), read_desktop_file(&path)) else {
            continue;
        };
        map.insert(
            name.to_owned(),
            Found {
                path: Arc::from(path_str),
                dir: dir_arc.clone(),
                entry: parse_desktop_entry(&bytes, locales),
            },
        );
    }
}

/// Reads at most [`MAX_DESKTOP_FILE`] bytes of `path` if it is a regular file after following
/// symlinks. Opening non-blocking keeps a FIFO from stalling the scan.
fn read_desktop_file(path: &Path) -> Option<Vec<u8>> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK;
    let file = File::from(rustix::fs::open(path, flags, Mode::empty()).ok()?);
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_DESKTOP_FILE as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn make_row(
    env: &StartupEnv,
    kind: StartupKindRaw,
    file_name: &str,
    found: &Found,
    overrides: Option<Arc<str>>,
) -> StartupRaw {
    let entry = &found.entry;
    let status = entry_status(entry, env);
    let name = match &entry.name {
        Some(name) if !name.is_empty() => name.clone(),
        _ => {
            let stem = file_name.strip_suffix(".desktop").unwrap_or(file_name);
            let mut text = String::new();
            sanitize_text(stem.as_bytes(), MAX_DESKTOP_FILE, &mut text);
            Arc::from(text)
        }
    };
    let resolved_path = entry
        .exec
        .as_deref()
        .and_then(first_word)
        .and_then(|program| {
            if Path::new(&program).is_absolute() {
                Some(PathBuf::from(program))
            } else {
                find_on_path(&program, &env.path_dirs)
            }
        })
        .and_then(|p| p.to_str().map(Arc::from));
    StartupRaw {
        kind,
        file_name: Arc::from(file_name),
        source_path: found.path.clone(),
        location: found.dir.clone(),
        name,
        icon: entry.icon.clone(),
        command: entry.exec.clone().unwrap_or_else(|| Arc::from("")),
        resolved_path,
        status,
        try_exec: entry.try_exec.clone(),
        only_show_in: entry.only_show_in.clone(),
        not_show_in: entry.not_show_in.clone(),
        kde_autostart_phase: entry.kde_autostart_phase.clone(),
        generated_unit: None,
        overridden_by_user: kind == StartupKindRaw::AutostartSystemOverridden,
        overrides,
        tasklens_override: entry.tasklens_override,
        unit_file_state: None,
        service_type: None,
        can_toggle: !matches!(
            status,
            StartupStatusRaw::NotInThisDesktop | StartupStatusRaw::MissingProgram
        ) && kind != StartupKindRaw::AutostartSystemOverridden,
        can_delete: kind == StartupKindRaw::AutostartUser,
        can_run: true,
        file_exists: true,
    }
}

/// The first rule that matches wins (SYS-019, SYS-020). Desktop names compare case-sensitively.
fn entry_status(entry: &DesktopEntry, env: &StartupEnv) -> StartupStatusRaw {
    let in_desktop = |list: &[Arc<str>]| {
        list.iter()
            .any(|d| env.current_desktop.iter().any(|c| c.as_str() == &**d))
    };
    if (!entry.only_show_in.is_empty() && !in_desktop(&entry.only_show_in))
        || in_desktop(&entry.not_show_in)
    {
        return StartupStatusRaw::NotInThisDesktop;
    }
    if let Some(try_exec) = entry.try_exec.as_deref().filter(|t| !t.is_empty()) {
        let present = if Path::new(try_exec).is_absolute() {
            is_executable(Path::new(try_exec))
        } else {
            find_on_path(try_exec, &env.path_dirs).is_some()
        };
        if !present {
            return StartupStatusRaw::MissingProgram;
        }
    }
    if entry.hidden || entry.gnome_autostart_enabled == Some(false) {
        StartupStatusRaw::Disabled
    } else {
        StartupStatusRaw::Enabled
    }
}

/// The first `PATH` directory that holds `program` as an executable regular file. A name with a
/// `/` is never searched for, as in a shell.
pub fn find_on_path(program: &str, path_dirs: &[PathBuf]) -> Option<PathBuf> {
    if program.is_empty() || program.contains('/') {
        return None;
    }
    path_dirs
        .iter()
        .map(|dir| dir.join(program))
        .find(|p| is_executable(p))
}

/// A regular file, after following symlinks, with at least one execute bit.
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The program word of an `Exec` value, with `"..."` quoting and its backslash escapes removed.
pub(super) fn first_word(exec: &str) -> Option<String> {
    let exec = exec.trim_start();
    let mut word = String::new();
    if let Some(rest) = exec.strip_prefix('"') {
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => word.extend(chars.next()),
                _ => word.push(c),
            }
        }
    } else {
        word.extend(exec.chars().take_while(|c| !c.is_whitespace()));
    }
    (!word.is_empty()).then_some(word)
}

/// `true` or `false` exactly; anything else is absent.
fn bool_value(value: &[u8]) -> Option<bool> {
    match value {
        b"true" => Some(true),
        b"false" => Some(false),
        _ => None,
    }
}

/// Unescapes and sanitizes one string value.
fn string_value(value: &[u8], scratch: &mut Vec<u8>, text: &mut String) -> Arc<str> {
    scratch.clear();
    let mut rest = value;
    while let Some((&b, tail)) = rest.split_first() {
        rest = tail;
        if b == b'\\' {
            match rest
                .split_first()
                .and_then(|(&e, t)| unescape(e, false).map(|u| (u, t)))
            {
                Some((u, t)) => {
                    scratch.push(u);
                    rest = t;
                }
                None => scratch.push(b),
            }
        } else {
            scratch.push(b);
        }
    }
    sanitize_text(scratch, MAX_DESKTOP_FILE, text);
    Arc::from(text.as_str())
}

/// Splits a list value on unescaped `;`, unescapes and sanitizes each item, and drops empty items.
fn list_value(
    value: &[u8],
    out: &mut SmallVec<[Arc<str>; 2]>,
    scratch: &mut Vec<u8>,
    text: &mut String,
) {
    scratch.clear();
    let mut rest = value;
    loop {
        let Some((&b, tail)) = rest.split_first() else {
            push_item(scratch, out, text);
            return;
        };
        rest = tail;
        match b {
            b';' => {
                push_item(scratch, out, text);
                scratch.clear();
            }
            b'\\' => match rest
                .split_first()
                .and_then(|(&e, t)| unescape(e, true).map(|u| (u, t)))
            {
                Some((u, t)) => {
                    scratch.push(u);
                    rest = t;
                }
                None => scratch.push(b),
            },
            _ => scratch.push(b),
        }
    }
}

fn push_item(item: &[u8], out: &mut SmallVec<[Arc<str>; 2]>, text: &mut String) {
    sanitize_text(item, MAX_DESKTOP_FILE, text);
    if !text.is_empty() {
        out.push(Arc::from(text.as_str()));
    }
}

/// The byte an escape `\<e>` stands for. `None` keeps both characters.
fn unescape(e: u8, list: bool) -> Option<u8> {
    match e {
        b's' => Some(b' '),
        b'n' => Some(b'\n'),
        b't' => Some(b'\t'),
        b'r' => Some(b'\r'),
        b'\\' => Some(b'\\'),
        b';' if list => Some(b';'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[Arc<str>]) -> Vec<&str> {
        list.iter().map(|s| &**s).collect()
    }

    fn fixture_env() -> StartupEnv {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/autostart-kde");
        StartupEnv {
            config_home: root.join("home/.config"),
            config_dirs: vec![root.join("etc/xdg"), root.join("etc/xdg2")],
            current_desktop: vec!["KDE".to_owned()],
            path_dirs: Vec::new(),
            locales: SmallVec::new(),
        }
    }

    fn fixture_path(rel: &str) -> String {
        format!(
            "{}/tests/fixtures/autostart-kde/{rel}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    fn scan(env: &StartupEnv) -> Vec<StartupRaw> {
        let mut rows = Vec::new();
        scan_autostart(env, &mut rows).unwrap();
        rows
    }

    fn row<'a>(rows: &'a [StartupRaw], kind: StartupKindRaw, file: &str) -> &'a StartupRaw {
        rows.iter()
            .find(|r| r.kind == kind && &*r.file_name == file)
            .unwrap_or_else(|| panic!("no {kind:?} row for {file}"))
    }

    #[test]
    fn user_overrides_system() {
        let rows = scan(&fixture_env());
        let system_a = fixture_path("etc/xdg/autostart/a.desktop");
        let user = row(&rows, StartupKindRaw::AutostartUser, "a.desktop");
        assert_eq!(user.overrides.as_deref(), Some(system_a.as_str()));
        assert!(!user.overridden_by_user);
        assert_eq!(&*user.name, "Alpha Tray");
        let shadowed = row(
            &rows,
            StartupKindRaw::AutostartSystemOverridden,
            "a.desktop",
        );
        assert!(shadowed.overridden_by_user);
        assert_eq!(&*shadowed.source_path, system_a);
        assert!(!shadowed.can_toggle);
        assert_eq!(
            rows.iter().filter(|r| &*r.file_name == "a.desktop").count(),
            2
        );
    }

    #[test]
    fn earlier_config_dir_wins() {
        let rows = scan(&fixture_env());
        let b: Vec<_> = rows
            .iter()
            .filter(|r| &*r.file_name == "b.desktop")
            .collect();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].kind, StartupKindRaw::AutostartSystem);
        assert_eq!(&*b[0].name, "Bravo One");
        assert_eq!(&*b[0].location, fixture_path("etc/xdg/autostart"));
    }

    #[test]
    fn only_show_in_excludes() {
        let rows = scan(&fixture_env());
        let r = row(&rows, StartupKindRaw::AutostartSystem, "gnome-only.desktop");
        assert_eq!(r.status, StartupStatusRaw::NotInThisDesktop);
        assert!(!r.can_toggle);

        // Desktop names are case-sensitive.
        let mut env = fixture_env();
        env.current_desktop = vec!["gnome".to_owned()];
        let rows = scan(&env);
        let r = row(&rows, StartupKindRaw::AutostartSystem, "gnome-only.desktop");
        assert_eq!(r.status, StartupStatusRaw::NotInThisDesktop);

        env.current_desktop = vec!["ubuntu".to_owned(), "GNOME".to_owned()];
        let rows = scan(&env);
        let r = row(&rows, StartupKindRaw::AutostartSystem, "gnome-only.desktop");
        assert_eq!(r.status, StartupStatusRaw::Enabled);
    }

    #[test]
    fn try_exec_missing() {
        let rows = scan(&fixture_env());
        let r = row(
            &rows,
            StartupKindRaw::AutostartSystem,
            "missing-tryexec.desktop",
        );
        assert_eq!(r.status, StartupStatusRaw::MissingProgram);
        assert!(!r.can_toggle);
        assert_eq!(r.try_exec.as_deref(), Some("/nonexistent/bin"));
    }

    #[test]
    fn hidden_disables() {
        let rows = scan(&fixture_env());
        let r = row(&rows, StartupKindRaw::AutostartUser, "hidden.desktop");
        assert_eq!(r.status, StartupStatusRaw::Disabled);
        assert!(r.can_toggle);
    }

    #[test]
    fn gnome_enabled_false_disables() {
        let rows = scan(&fixture_env());
        let r = row(&rows, StartupKindRaw::AutostartUser, "gnome-off.desktop");
        assert_eq!(r.status, StartupStatusRaw::Disabled);
    }

    #[test]
    fn env_fallback_drops_relative() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        let env = StartupEnv::from_vars(vars(&[
            ("XDG_CONFIG_DIRS", "relative:/etc/xdg"),
            ("HOME", "/home/sample"),
            ("XDG_CONFIG_HOME", "rel/config"),
            ("PATH", "/usr/bin:bin::/opt/tools/bin"),
            ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
            ("LC_ALL", ""),
            ("LANG", "nb_NO.UTF-8"),
        ]));
        assert_eq!(env.config_dirs, [PathBuf::from("/etc/xdg")]);
        assert_eq!(env.config_home, PathBuf::from("/home/sample/.config"));
        assert_eq!(
            env.path_dirs,
            [PathBuf::from("/usr/bin"), PathBuf::from("/opt/tools/bin")]
        );
        assert_eq!(env.current_desktop, ["ubuntu", "GNOME"]);
        assert_eq!(env.locales.as_slice(), ["nb_NO", "nb"]);

        let env = StartupEnv::from_vars(vars(&[
            ("XDG_CONFIG_DIRS", "only/relative"),
            ("XDG_CONFIG_HOME", "/srv/sample-config"),
        ]));
        assert_eq!(env.config_dirs, [PathBuf::from("/etc/xdg")]);
        assert_eq!(env.config_home, PathBuf::from("/srv/sample-config"));
        assert!(env.path_dirs.is_empty());
        assert!(env.locales.is_empty());
    }

    #[test]
    fn delete_only_user() {
        let rows = scan(&fixture_env());
        assert!(!rows.is_empty());
        for r in &rows {
            assert_eq!(
                r.can_delete,
                r.kind == StartupKindRaw::AutostartUser,
                "{}",
                r.file_name
            );
            assert!(r.can_run && r.file_exists);
        }
        assert!(row(&rows, StartupKindRaw::AutostartUser, "a.desktop").can_delete);
        assert!(!row(&rows, StartupKindRaw::AutostartSystem, "b.desktop").can_delete);
    }

    #[test]
    fn row_order_and_fields() {
        let rows = scan(&fixture_env());
        let order: Vec<_> = rows.iter().map(|r| (r.kind, &*r.file_name)).collect();
        use StartupKindRaw::{
            AutostartSystem as S, AutostartSystemOverridden as O, AutostartUser as U,
        };
        assert_eq!(
            order,
            [
                (U, "a.desktop"),
                (U, "gnome-off.desktop"),
                (U, "hidden.desktop"),
                (O, "a.desktop"),
                (S, "b.desktop"),
                (S, "delta.desktop"),
                (S, "gnome-only.desktop"),
                (S, "missing-tryexec.desktop"),
            ]
        );

        // No Name: the file name without its extension.
        let delta = row(&rows, S, "delta.desktop");
        assert_eq!(&*delta.name, "delta");
        assert_eq!(delta.kde_autostart_phase.as_deref(), Some("2"));
        assert_eq!(delta.status, StartupStatusRaw::Enabled);
        assert_eq!(delta.resolved_path, None);

        // A quoted absolute program keeps its space and loses the quotes.
        let b = row(&rows, S, "b.desktop");
        assert_eq!(b.resolved_path.as_deref(), Some("/opt/bravo tools/bravo"));
        assert_eq!(&*b.command, "\"/opt/bravo tools/bravo\" --start");
    }

    #[test]
    fn path_lookup_needs_exec_bit() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("sample-tool");
        std::fs::write(&tool, b"#!/bin/sh\n").unwrap();
        let dirs = [dir.path().to_path_buf()];
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_on_path("sample-tool", &dirs), None);
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_on_path("sample-tool", &dirs), Some(tool.clone()));
        assert_eq!(find_on_path("./sample-tool", &dirs), None);
        assert_eq!(find_on_path("", &dirs), None);

        // TryExec and Exec resolve through the same lookup.
        let auto = dir.path().join("home/autostart");
        std::fs::create_dir_all(&auto).unwrap();
        std::fs::write(
            auto.join("t.desktop"),
            b"[Desktop Entry]\nExec=sample-tool --x\nTryExec=sample-tool\n",
        )
        .unwrap();
        let env = StartupEnv {
            config_home: dir.path().join("home"),
            path_dirs: dirs.to_vec(),
            ..StartupEnv::default()
        };
        let rows = scan(&env);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, StartupStatusRaw::Enabled);
        assert_eq!(rows[0].resolved_path.as_deref(), tool.to_str());
    }

    #[test]
    fn first_word_quotes() {
        assert_eq!(first_word("  foo --bar").as_deref(), Some("foo"));
        assert_eq!(first_word(r#""/a b/c" -x"#).as_deref(), Some("/a b/c"));
        assert_eq!(first_word(r#""a\"b""#).as_deref(), Some("a\"b"));
        assert_eq!(first_word("   "), None);
    }

    #[test]
    fn basic_keys_and_locale() {
        let file = b"[Desktop Entry]\nName=Foo\nName[de]=Fuh\nExec=foo --bar\nHidden=true\nOnlyShowIn=KDE;GNOME;\n";
        let e = parse_desktop_entry(file, &["de"]);
        assert_eq!(e.name.as_deref(), Some("Fuh"));
        assert_eq!(e.exec.as_deref(), Some("foo --bar"));
        assert!(e.hidden);
        assert_eq!(names(&e.only_show_in), ["KDE", "GNOME"]);

        // Without a matching locale, plain Name is used.
        let e = parse_desktop_entry(file, &["fr"]);
        assert_eq!(e.name.as_deref(), Some("Foo"));

        // The most specific locale wins, whatever the order in the file.
        let file = b"[Desktop Entry]\nName[de]=A\nName[de_DE]=B\nName=C\n";
        let e = parse_desktop_entry(file, &["de_DE", "de"]);
        assert_eq!(e.name.as_deref(), Some("B"));

        // Spaces around `=` are trimmed, and the first duplicate key wins.
        let file = b"[Desktop Entry]\nExec = first\nExec=second\nType=Application\n";
        let e = parse_desktop_entry(file, &[]);
        assert_eq!(e.exec.as_deref(), Some("first"));
        assert_eq!(e.entry_type.as_deref(), Some("Application"));
    }

    #[test]
    fn escapes() {
        let e = parse_desktop_entry(b"[Desktop Entry]\nExec=foo\\sbar\\\\x\n", &[]);
        assert_eq!(e.exec.as_deref(), Some("foo bar\\x"));

        // An unknown escape keeps both characters; `\;` is only special in lists.
        let e = parse_desktop_entry(b"[Desktop Entry]\nExec=a\\qb\\;c\\\n", &[]);
        assert_eq!(e.exec.as_deref(), Some("a\\qb\\;c\\"));

        let e = parse_desktop_entry(b"[Desktop Entry]\nNotShowIn=A\\;B;;C\n", &[]);
        assert_eq!(names(&e.not_show_in), ["A;B", "C"]);
    }

    #[test]
    fn ignores_other_groups() {
        let file = b"# comment\nExec=outside\n[Desktop Action new]\nExec=action\nName=Action\n\
            [Desktop Entry]\nName=Real\nnot a key line\n[Other Group]\nExec=other\n";
        let e = parse_desktop_entry(file, &[]);
        assert_eq!(e.name.as_deref(), Some("Real"));
        assert_eq!(e.exec, None);
    }

    #[test]
    fn bool_strict() {
        let e = parse_desktop_entry(b"[Desktop Entry]\nHidden=yes\n", &[]);
        assert!(!e.hidden);
        let e = parse_desktop_entry(
            b"[Desktop Entry]\nX-GNOME-Autostart-enabled=false\nHidden=True\n",
            &[],
        );
        assert_eq!(e.gnome_autostart_enabled, Some(false));
        assert!(!e.hidden);
        let e = parse_desktop_entry(b"[Desktop Entry]\nX-GNOME-Autostart-enabled=1\n", &[]);
        assert_eq!(e.gnome_autostart_enabled, None);
    }

    #[test]
    fn sanitized_strings() {
        let e = parse_desktop_entry(
            "[Desktop Entry]\nName=ab\u{202E}cd\x07ef\nX-KDE-autostart-phase=\u{2066}1\n"
                .as_bytes(),
            &[],
        );
        assert_eq!(e.name.as_deref(), Some("abcd\u{FFFD}ef"));
        assert_eq!(e.kde_autostart_phase.as_deref(), Some("1"));

        // Invalid UTF-8 becomes U+FFFD.
        let e = parse_desktop_entry(b"[Desktop Entry]\nIcon=a\xFFb\n", &[]);
        assert_eq!(e.icon.as_deref(), Some("a\u{FFFD}b"));
    }

    #[test]
    fn tasklens_override_key() {
        let e = parse_desktop_entry(b"[Desktop Entry]\nX-Tasklens-Override=true\n", &[]);
        assert!(e.tasklens_override);
        let e = parse_desktop_entry(b"[Desktop Entry]\nX-Tasklens-Override=yes\n", &[]);
        assert!(!e.tasklens_override);
    }

    #[test]
    fn oversized_input_is_cut() {
        let mut file = b"[Desktop Entry]\nName=Big\n".to_vec();
        file.resize(MAX_DESKTOP_FILE, b'#');
        file.extend_from_slice(b"\nExec=past-the-limit\n");
        let e = parse_desktop_entry(&file, &[]);
        assert_eq!(e.name.as_deref(), Some("Big"));
        assert_eq!(e.exec, None);
    }

    #[test]
    fn locale_list() {
        assert_eq!(
            locale_match_list("de_DE.UTF-8@euro").as_slice(),
            ["de_DE@euro", "de_DE", "de@euro", "de"]
        );
        assert_eq!(locale_match_list("nb_NO.UTF-8").as_slice(), ["nb_NO", "nb"]);
        assert_eq!(locale_match_list("sr@latin").as_slice(), ["sr@latin", "sr"]);
        assert_eq!(locale_match_list("C").as_slice(), ["C"]);
        assert!(locale_match_list("").is_empty());
    }
}
