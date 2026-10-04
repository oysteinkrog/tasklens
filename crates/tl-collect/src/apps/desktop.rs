//! Desktop entry parser and index (PROC-006, ADR-016).
//!
//! App rows take their name, icon and description from `.desktop` files. The index is built once
//! off the UI thread from the user and system `applications/` directories. Desktop files are
//! user-writable, so every value passes through [`sanitize_text`] before it is stored.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::sanitize::sanitize_text;

/// Limit for one sanitized value, in bytes.
pub const VALUE_MAX: usize = 4096;
/// Files larger than this are read only up to this many bytes.
pub const FILE_MAX: u64 = 1024 * 1024;
/// How deep `build` walks below each `applications/` directory.
pub const WALK_DEPTH_MAX: usize = 8;
/// Fixed `PATH` list for resolving a relative `Exec` binary, so the result does not depend on
/// the caller's environment.
pub const EXEC_PATH_DIRS: [&str; 3] = ["/usr/local/bin", "/usr/bin", "/bin"];

/// One indexed `[Desktop Entry]` of type `Application`.
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopEntry {
    /// Desktop-file id: the path below `applications/` with `/` replaced by `-`, without
    /// `.desktop`.
    pub id: Arc<str>,
    pub path: Arc<str>,
    pub name: Arc<str>,
    pub generic_name: Option<Arc<str>>,
    pub comment: Option<Arc<str>>,
    pub exec: Option<Arc<str>>,
    /// The binary `exec` starts, from [`exec_binary`].
    pub exec_binary: Option<Arc<str>>,
    pub icon: Option<Arc<str>>,
    pub startup_wm_class: Option<Arc<str>>,
    pub flatpak: Option<Arc<str>>,
}

/// Desktop entries by id, by `Exec` binary and by lowercase `StartupWMClass`.
#[derive(Clone, Debug, Default)]
pub struct DesktopIndex {
    by_id: FxHashMap<Arc<str>, Arc<DesktopEntry>>,
    by_exe: FxHashMap<Arc<str>, SmallVec<[Arc<DesktopEntry>; 2]>>,
    by_wm_class: FxHashMap<Arc<str>, Arc<DesktopEntry>>,
    pub generation: u64,
}

impl DesktopIndex {
    /// Builds the index from `dirs` in priority order: an id seen in an earlier directory wins.
    /// Subdirectories are walked. Unreadable directories and files are skipped.
    pub fn build(dirs: &[PathBuf]) -> DesktopIndex {
        Self::build_in(dirs, &default_path_dirs())
    }

    /// [`DesktopIndex::build`] with an explicit `PATH` list for [`exec_binary_in`].
    pub fn build_in(dirs: &[PathBuf], path_dirs: &[&Path]) -> DesktopIndex {
        let mut index = DesktopIndex::default();
        let mut buf = Vec::new();
        for dir in dirs {
            index.walk(dir, "", 0, path_dirs, &mut buf);
        }
        index
    }

    pub fn by_id(&self, id: &str) -> Option<&Arc<DesktopEntry>> {
        self.by_id.get(id)
    }

    /// Entries whose `Exec` binary is `canonical_exe`, in priority order.
    pub fn by_exe(&self, canonical_exe: &str) -> &[Arc<DesktopEntry>] {
        self.by_exe.get(canonical_exe).map_or(&[], |v| v.as_slice())
    }

    /// Case-insensitive `StartupWMClass` lookup.
    pub fn by_wm_class(&self, class: &str) -> Option<&Arc<DesktopEntry>> {
        if class.chars().any(char::is_uppercase) {
            self.by_wm_class.get(class.to_lowercase().as_str())
        } else {
            self.by_wm_class.get(class)
        }
    }

    /// All entries, in no particular order.
    pub fn entries(&self) -> impl Iterator<Item = &Arc<DesktopEntry>> {
        self.by_id.values()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    fn walk(&mut self, dir: &Path, prefix: &str, depth: usize, path_dirs: &[&Path], buf: &mut Vec<u8>) {
        let Ok(read) = fs::read_dir(dir) else {
            return;
        };
        // Sorted so the result does not depend on directory order when two files map to one id.
        let mut names: Vec<_> = read.filter_map(Result::ok).collect();
        names.sort_by_key(|e| e.file_name());
        for ent in names {
            let Ok(kind) = ent.file_type() else {
                continue;
            };
            let file_name = ent.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let path = ent.path();
            // Symlinked directories are not followed, so a link loop cannot hang the walk.
            if kind.is_dir() {
                if depth < WALK_DEPTH_MAX {
                    let sub = format!("{prefix}{name}-");
                    self.walk(&path, &sub, depth + 1, path_dirs, buf);
                }
                continue;
            }
            let Some(stem) = name.strip_suffix(".desktop") else {
                continue;
            };
            let mut id = String::new();
            sanitize_text(format!("{prefix}{stem}").as_bytes(), VALUE_MAX, &mut id);
            if id.is_empty() || self.by_id.contains_key(id.as_str()) {
                continue;
            }
            if !read_capped(&path, buf) {
                continue;
            }
            let mut path_text = String::new();
            sanitize_text(path.as_os_str().as_bytes(), VALUE_MAX, &mut path_text);
            if let Some(entry) = parse_entry(&id, &path_text, buf, path_dirs) {
                self.insert(entry);
            }
        }
    }

    fn insert(&mut self, entry: DesktopEntry) {
        let entry = Arc::new(entry);
        if let Some(exe) = &entry.exec_binary {
            self.by_exe
                .entry(exe.clone())
                .or_default()
                .push(entry.clone());
        }
        if let Some(class) = &entry.startup_wm_class {
            let key: Arc<str> = Arc::from(class.to_lowercase());
            self.by_wm_class.entry(key).or_insert_with(|| entry.clone());
        }
        self.by_id.insert(entry.id.clone(), entry);
    }
}

/// Parses the `[Desktop Entry]` group of one file. Returns `None` for entries that are not
/// indexed: `NoDisplay=true`, `Hidden=true`, `Type` other than `Application`, or no `Name`.
/// Localised keys such as `Name[de]` are ignored. `path_dirs` is the `PATH` list for
/// [`exec_binary_in`].
pub fn parse_entry(id: &str, path: &str, bytes: &[u8], path_dirs: &[&Path]) -> Option<DesktopEntry> {
    let mut raw = RawEntry::default();
    let mut in_group = false;
    for line in bytes.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line).trim_ascii();
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if line[0] == b'[' {
            in_group = line == b"[Desktop Entry]";
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
        let slot = match key {
            b"Name" => &mut raw.name,
            b"GenericName" => &mut raw.generic_name,
            b"Comment" => &mut raw.comment,
            b"Exec" => &mut raw.exec,
            b"Icon" => &mut raw.icon,
            b"StartupWMClass" => &mut raw.startup_wm_class,
            b"X-Flatpak" => &mut raw.flatpak,
            b"Type" => &mut raw.kind,
            b"NoDisplay" => &mut raw.no_display,
            b"Hidden" => &mut raw.hidden,
            _ => continue,
        };
        // A repeated key is invalid; the first one wins.
        if slot.is_none() {
            *slot = Some(value);
        }
    }

    if raw.kind != Some(b"Application".as_slice())
        || raw.no_display == Some(b"true".as_slice())
        || raw.hidden == Some(b"true".as_slice())
    {
        return None;
    }
    let name = clean(raw.name)?;
    let exec = clean(raw.exec);
    let exec_binary = exec.as_deref().and_then(|e| exec_binary_in(e, path_dirs));
    Some(DesktopEntry {
        id: Arc::from(id),
        path: Arc::from(path),
        name,
        generic_name: clean(raw.generic_name),
        comment: clean(raw.comment),
        exec,
        exec_binary,
        icon: clean(raw.icon),
        startup_wm_class: clean(raw.startup_wm_class),
        flatpak: clean(raw.flatpak),
    })
}

/// The binary an `Exec` value starts, resolved through [`EXEC_PATH_DIRS`].
///
/// Splits with Desktop Entry quoting rules, drops field codes, strips the `env`, `flatpak run`
/// and `snap run` wrappers and returns the first remaining token. A relative binary is looked up
/// in the fixed `PATH` list. The canonical path of an existing file is returned, else the token
/// as is.
pub fn exec_binary(exec: &str) -> Option<Arc<str>> {
    exec_binary_in(exec, &default_path_dirs())
}

/// [`exec_binary`] with an explicit `PATH` list.
pub fn exec_binary_in(exec: &str, path_dirs: &[&Path]) -> Option<Arc<str>> {
    let tokens = split_exec(exec);
    let mut rest = tokens.as_slice();
    loop {
        let first = rest.first()?;
        match basename(first) {
            "env" => {
                rest = &rest[1..];
                while rest.first().is_some_and(|t| is_env_assignment(t)) {
                    rest = &rest[1..];
                }
            }
            "flatpak" if rest.get(1).is_some_and(|t| t == "run") => {
                let mut command = None;
                let mut i = 2;
                while let Some(opt) = rest.get(i).filter(|t| t.starts_with('-')) {
                    if let Some(cmd) = opt.strip_prefix("--command=") {
                        command = Some(cmd);
                    }
                    i += 1;
                }
                let app_id = rest.get(i).map(String::as_str);
                return command.or(app_id).and_then(|t| resolve(t, path_dirs));
            }
            "snap" if rest.get(1).is_some_and(|t| t == "run") => {
                let name = rest[2..].iter().find(|t| !t.starts_with('-'))?;
                return resolve(name, path_dirs);
            }
            _ => return resolve(first, path_dirs),
        }
    }
}

/// `$XDG_DATA_HOME/applications` (default `~/.local/share`), then each
/// `$XDG_DATA_DIRS/applications` (default `/usr/local/share:/usr/share`). An unset, empty or
/// relative value falls back to the default, per the XDG base directory spec. A relative entry
/// inside `XDG_DATA_DIRS` is dropped; if none is left, the default is used.
pub fn data_dirs_from_env(
    home: &Path,
    xdg_data_home: Option<&OsStr>,
    xdg_data_dirs: Option<&OsStr>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let data_home = xdg_data_home
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .map_or_else(|| home.join(".local/share"), Path::to_path_buf);
    out.push(data_home.join("applications"));

    let mut system: Vec<PathBuf> = xdg_data_dirs
        .map(|v| {
            v.as_bytes()
                .split(|&b| b == b':')
                .map(|p| Path::new(OsStr::from_bytes(p)))
                .filter(|p| p.is_absolute())
                .map(|p| p.join("applications"))
                .collect()
        })
        .unwrap_or_default();
    if system.is_empty() {
        system = ["/usr/local/share", "/usr/share"]
            .iter()
            .map(|p| Path::new(p).join("applications"))
            .collect();
    }
    out.extend(system);
    out
}

#[derive(Default)]
struct RawEntry<'a> {
    name: Option<&'a [u8]>,
    generic_name: Option<&'a [u8]>,
    comment: Option<&'a [u8]>,
    exec: Option<&'a [u8]>,
    icon: Option<&'a [u8]>,
    startup_wm_class: Option<&'a [u8]>,
    flatpak: Option<&'a [u8]>,
    kind: Option<&'a [u8]>,
    no_display: Option<&'a [u8]>,
    hidden: Option<&'a [u8]>,
}

fn default_path_dirs() -> [&'static Path; 3] {
    EXEC_PATH_DIRS.map(Path::new)
}

/// Reads at most [`FILE_MAX`] bytes of `path` into `buf`. False when the file cannot be read.
fn read_capped(path: &Path, buf: &mut Vec<u8>) -> bool {
    buf.clear();
    match File::open(path) {
        Ok(f) => f.take(FILE_MAX).read_to_end(buf).is_ok(),
        Err(_) => false,
    }
}

/// Unescapes a string value (`\s` and `\t` become a space, plus `\n`, `\r` and `\\`), then
/// sanitizes it. Empty values become `None`.
fn clean(raw: Option<&[u8]>) -> Option<Arc<str>> {
    let raw = raw?;
    let mut unescaped = Vec::with_capacity(raw.len());
    let mut iter = raw.iter().copied();
    while let Some(b) = iter.next() {
        if b != b'\\' {
            unescaped.push(b);
            continue;
        }
        match iter.next() {
            Some(b's') => unescaped.push(b' '),
            Some(b'n') => unescaped.push(b'\n'),
            Some(b't') => unescaped.push(b' '),
            Some(b'r') => unescaped.push(b'\r'),
            Some(b'\\') => unescaped.push(b'\\'),
            // Other escapes (such as `\"` inside `Exec`) are kept for the next layer.
            Some(other) => unescaped.extend_from_slice(&[b'\\', other]),
            None => unescaped.push(b'\\'),
        }
    }
    let mut out = String::new();
    sanitize_text(&unescaped, VALUE_MAX, &mut out);
    (!out.is_empty()).then(|| Arc::from(out))
}

/// Splits an `Exec` value into arguments and drops field codes.
///
/// Arguments are separated by spaces and tabs. Inside double quotes, a backslash escapes `"`,
/// `` ` ``, `$` and `\`. Outside quotes a backslash escapes the next character. `%%` becomes `%`
/// and other `%x` codes are removed; an argument that was only field codes is dropped.
fn split_exec(exec: &str) -> SmallVec<[String; 8]> {
    let mut out = SmallVec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut had_code = false;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' => {
                started = true;
                match chars.next() {
                    Some(n @ ('"' | '`' | '$' | '\\')) if quoted => cur.push(n),
                    Some(n) if quoted => {
                        cur.push('\\');
                        cur.push(n);
                    }
                    Some(n) => cur.push(n),
                    None => cur.push('\\'),
                }
            }
            '%' => {
                started = true;
                match chars.next() {
                    Some('%') => cur.push('%'),
                    Some(_) => had_code = true,
                    None => cur.push('%'),
                }
            }
            ' ' | '\t' if !quoted => {
                push_token(&mut out, &mut cur, started, had_code);
                started = false;
                had_code = false;
            }
            _ => {
                started = true;
                cur.push(c);
            }
        }
    }
    push_token(&mut out, &mut cur, started, had_code);
    out
}

fn push_token(out: &mut SmallVec<[String; 8]>, cur: &mut String, started: bool, had_code: bool) {
    if started && !(cur.is_empty() && had_code) {
        out.push(std::mem::take(cur));
    }
    cur.clear();
}

fn basename(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

/// `VAR=value` as `env` takes it: a non-empty name before `=` without `/`.
fn is_env_assignment(token: &str) -> bool {
    match token.find('=') {
        Some(eq) => eq > 0 && !token[..eq].contains('/'),
        None => false,
    }
}

fn resolve(token: &str, path_dirs: &[&Path]) -> Option<Arc<str>> {
    if token.is_empty() {
        return None;
    }
    if token.contains('/') {
        let path = Path::new(token);
        if path.is_absolute() && path.is_file() {
            if let Some(canon) = canonical(path) {
                return Some(canon);
            }
        }
        return Some(Arc::from(token));
    }
    for dir in path_dirs {
        let candidate = dir.join(token);
        if candidate.is_file() {
            if let Some(canon) = canonical(&candidate) {
                return Some(canon);
            }
        }
    }
    Some(Arc::from(token))
}

fn canonical(path: &Path) -> Option<Arc<str>> {
    let canon = fs::canonicalize(path).ok()?;
    Some(Arc::from(canon.to_string_lossy().as_ref()))
}
