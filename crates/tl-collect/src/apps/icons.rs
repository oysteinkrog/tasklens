//! Icon theme resolution (PROC-006, ADR-016 "Icons").
//!
//! App rows draw a 24 px icon and process rows 16 px. The icon name comes from a desktop entry's
//! `Icon=` key and the file from the icon themes: the user's KDE theme, then `breeze`, then
//! `hicolor`. `Inherits=` is not followed; that fixed order replaces it. Missing themes are
//! skipped.
//!
//! Resolved paths are cached in `$XDG_CACHE_HOME/tasklens/icons.toml` by [`IconCache`].

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::error::CollectError;

/// Fixed icon sizes tried after the requested size, ascending.
pub const SIZES: [u32; 4] = [16, 22, 24, 32];
/// `index.theme` and `kdeglobals` files larger than this are read only up to this many bytes.
pub const INDEX_MAX: u64 = 1024 * 1024;

/// The engine saves the icon cache at most once per this many seconds, and at shutdown.
pub const ICON_CACHE_SAVE_INTERVAL_S: u64 = 60;
/// An icon cache file larger than this is treated as corrupt.
pub const CACHE_MAX: u64 = 4 * 1024 * 1024;

/// One theme: the `<base>/<name>` directories that exist and the icon directories its
/// `index.theme` lists.
#[derive(Clone, Debug)]
pub struct ThemeDirs {
    pub theme: String,
    pub base_dirs: Vec<PathBuf>,
    pub dirs: Vec<ThemeDir>,
}

/// One directory group of `index.theme`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeDir {
    /// Path relative to the theme directory, such as `apps/22` or `24x24/apps`.
    pub rel: String,
    pub size: u32,
    pub scale: u32,
    /// `Type=Scalable`.
    pub scalable: bool,
}

/// Fallback icons the UI draws when a process has no icon of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenericIcon {
    Executable,
    KernelThread,
    Service,
}

/// The `Theme=` value of the `[Icons]` group in `kdeglobals`, trimmed. `None` when absent or
/// empty.
pub fn kde_icon_theme(kdeglobals: &[u8]) -> Option<String> {
    let mut in_group = false;
    for line in kdeglobals.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line).trim_ascii();
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if line[0] == b'[' {
            in_group = line == b"[Icons]";
            continue;
        }
        if !in_group {
            continue;
        }
        let Some(eq) = memchr::memchr(b'=', line) else {
            continue;
        };
        if line[..eq].trim_ascii_end() != b"Theme" {
            continue;
        }
        let value = std::str::from_utf8(line[eq + 1..].trim_ascii_start()).ok()?;
        return (!value.is_empty()).then(|| value.to_owned());
    }
    None
}

/// `[user_theme, "breeze", "hicolor"]` without duplicates. An empty user theme is skipped.
pub fn theme_order(user_theme: Option<&str>) -> SmallVec<[String; 3]> {
    let mut out = SmallVec::new();
    for name in user_theme.into_iter().chain(["breeze", "hicolor"]) {
        if !name.is_empty() && !out.iter().any(|t: &String| t == name) {
            out.push(name.to_owned());
        }
    }
    out
}

/// Loads theme `name` from `icon_bases` in priority order (`~/.local/share/icons`, then each
/// `$XDG_DATA_DIRS/icons`). The first `<base>/<name>/index.theme` found gives the directory
/// list; every `<base>/<name>` directory that exists is searched. `None` when no base has an
/// `index.theme` for the theme or the name is not a single path component.
pub fn load_theme(name: &str, icon_bases: &[PathBuf]) -> Option<ThemeDirs> {
    if !is_component(name) {
        return None;
    }
    let mut base_dirs = Vec::new();
    let mut dirs = None;
    let mut buf = Vec::new();
    for base in icon_bases {
        let theme_dir = base.join(name);
        if !theme_dir.is_dir() {
            continue;
        }
        if dirs.is_none() && read_capped(&theme_dir.join("index.theme"), &mut buf) {
            dirs = Some(parse_index_theme(&buf));
        }
        base_dirs.push(theme_dir);
    }
    Some(ThemeDirs {
        theme: name.to_owned(),
        base_dirs,
        dirs: dirs?,
    })
}

/// Parses `index.theme`: the `[Icon Theme]` group's `Directories=` list and each listed group's
/// `Size=`, `Scale=` (default 1) and `Type=`. Directories without a valid `Size=`, or whose path
/// is absolute or contains `..`, are dropped. The result keeps the `Directories=` order.
pub fn parse_index_theme(bytes: &[u8]) -> Vec<ThemeDir> {
    let mut directories: Option<&[u8]> = None;
    let mut groups: FxHashMap<&[u8], RawDir<'_>> = FxHashMap::default();
    let mut group: Option<&[u8]> = None;
    for line in bytes.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line).trim_ascii();
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if line[0] == b'[' {
            group = line
                .strip_prefix(b"[")
                .and_then(|l| l.strip_suffix(b"]"))
                .map(<[u8]>::trim_ascii);
            continue;
        }
        let Some(name) = group else {
            continue;
        };
        let Some(eq) = memchr::memchr(b'=', line) else {
            continue;
        };
        let key = line[..eq].trim_ascii_end();
        let value = line[eq + 1..].trim_ascii_start();
        if name == b"Icon Theme" {
            if key == b"Directories" && directories.is_none() {
                directories = Some(value);
            }
            continue;
        }
        let raw = groups.entry(name).or_default();
        let slot = match key {
            b"Size" => &mut raw.size,
            b"Scale" => &mut raw.scale,
            b"Type" => &mut raw.kind,
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(value);
        }
    }

    let mut out = Vec::new();
    let Some(directories) = directories else {
        return out;
    };
    for rel in directories.split(|&b| b == b',') {
        let rel = rel.trim_ascii();
        let Ok(rel_str) = std::str::from_utf8(rel) else {
            continue;
        };
        if rel_str.is_empty()
            || rel_str.starts_with('/')
            || rel_str.split('/').any(|c| c == "..")
            || out.iter().any(|d: &ThemeDir| d.rel == rel_str)
        {
            continue;
        }
        let Some(raw) = groups.get(rel) else {
            continue;
        };
        let Some(size) = raw.size.and_then(parse_u32) else {
            continue;
        };
        let scale = match raw.scale {
            Some(s) => match parse_u32(s) {
                Some(s) => s,
                None => continue,
            },
            None => 1,
        };
        out.push(ThemeDir {
            rel: rel_str.to_owned(),
            size,
            scale,
            scalable: raw.kind == Some(b"Scalable".as_slice()),
        });
    }
    out
}

/// Resolves an `Icon=` value to a file path.
///
/// An absolute path is returned when the file exists. Otherwise each theme is searched in
/// order. Within a theme the sizes go `size`, then the other [`SIZES`] ascending, then the
/// scalable directories. For each step every matching directory (scale 1) in every base is
/// checked for `<icon>.svg`, then `<icon>.png`. The first existing file wins.
pub fn resolve(icon: &str, size: u32, themes: &[ThemeDirs]) -> Option<Arc<str>> {
    if icon.starts_with('/') {
        return Path::new(icon).is_file().then(|| Arc::from(icon));
    }
    if !is_component(icon) {
        return None;
    }
    let svg = format!("{icon}.svg");
    let png = format!("{icon}.png");
    let sizes = std::iter::once(size).chain(SIZES.into_iter().filter(|&s| s != size));
    for theme in themes {
        let steps = sizes.clone().map(Some).chain(std::iter::once(None::<u32>));
        for step in steps {
            let matching = theme.dirs.iter().filter(|d| match step {
                Some(s) => d.size == s && d.scale == 1,
                None => d.scalable,
            });
            for dir in matching {
                for base in &theme.base_dirs {
                    let dir_path = base.join(&dir.rel);
                    for file in [&svg, &png] {
                        let path = dir_path.join(file);
                        if path.is_file()
                            && let Some(s) = path.to_str()
                        {
                            return Some(Arc::from(s));
                        }
                    }
                }
            }
        }
    }
    None
}

/// The theme icon name for a fallback icon. The caller resolves it through [`resolve`].
pub fn generic_icon(kind: GenericIcon) -> &'static str {
    match kind {
        GenericIcon::Executable => "application-x-executable",
        GenericIcon::KernelThread => "cpu",
        GenericIcon::Service => "preferences-system-services",
    }
}

/// Cached icon lookups: `(icon name, size)` to the resolved path, or `None` for "not found".
///
/// The file is a small TOML subset written and read only by this type:
///
/// ```toml
/// version = 1
/// theme = "breeze-dark"
/// [paths]
/// "firefox@24" = "/usr/share/icons/breeze-dark/apps/22/firefox.svg"
/// "konsole@16" = ""
/// ```
#[derive(Debug)]
pub struct IconCache {
    path: PathBuf,
    theme: String,
    map: FxHashMap<(Arc<str>, u32), Option<Arc<str>>>,
    dirty: bool,
}

impl IconCache {
    /// Reads the cache at `path`. A file with another version or theme, or one that does not
    /// parse, is discarded: the cache starts empty and is written again on the next save.
    pub fn load(path: PathBuf, theme: &str) -> Self {
        let mut buf = Vec::new();
        let (map, dirty) = match File::open(&path) {
            Ok(f) => {
                let read = f.take(CACHE_MAX + 1).read_to_end(&mut buf);
                let parsed = match read {
                    Ok(n) if n as u64 <= CACHE_MAX => parse_icon_cache(&buf, theme),
                    _ => None,
                };
                match parsed {
                    Some(map) => (map, false),
                    None => (FxHashMap::default(), true),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (FxHashMap::default(), false),
            Err(_) => (FxHashMap::default(), true),
        };
        Self {
            path,
            theme: theme.to_owned(),
            map,
            dirty,
        }
    }

    /// The path for `icon` at `size`. A cached path is checked with one `stat`; when the file is
    /// gone, or on a miss, [`resolve`] runs and its result is stored. A cached "not found" returns
    /// `None` without resolving.
    pub fn lookup(&mut self, icon: &str, size: u32, themes: &[ThemeDirs]) -> Option<Arc<str>> {
        let key = (Arc::<str>::from(icon), size);
        if let Some(cached) = self.map.get(&key) {
            match cached {
                None => return None,
                Some(path) if Path::new(&**path).is_file() => return Some(path.clone()),
                Some(_) => {}
            }
        }
        let found = resolve(icon, size, themes);
        self.map.insert(key, found.clone());
        self.dirty = true;
        found
    }

    /// Writes the cache when it changed since the last load or save: creates the parent
    /// directory (mode 0700) if missing, writes `icons.toml.tmp-<pid>` (mode 0600), syncs it,
    /// renames it over the cache file and syncs the directory. Entries are sorted by key.
    pub fn save(&mut self) -> Result<(), CollectError> {
        if !self.dirty {
            return Ok(());
        }
        let text = self.serialize();
        write_atomic(&self.path, text.as_bytes()).map_err(|e| CollectError::ProcRead {
            file: "icons.toml",
            errno: e
                .raw_os_error()
                .unwrap_or(rustix::io::Errno::IO.raw_os_error()),
        })?;
        self.dirty = false;
        Ok(())
    }

    fn serialize(&self) -> String {
        let mut entries: Vec<_> = self
            .map
            .iter()
            .filter(|((name, _), path)| {
                !has_control(name) && !path.as_deref().is_some_and(has_control)
            })
            .collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        // A theme name with a control character cannot be written; an empty theme never matches
        // on load, so the file is discarded then.
        let theme = if has_control(&self.theme) {
            ""
        } else {
            &self.theme
        };
        let mut out = format!("version = 1\ntheme = \"{}\"\n[paths]\n", escape(theme));
        for ((name, size), path) in entries {
            let path = path.as_deref().unwrap_or("");
            out.push_str(&format!(
                "\"{}@{size}\" = \"{}\"\n",
                escape(name),
                escape(path)
            ));
        }
        out
    }
}

/// Parses an icon cache file. `None` when it does not parse, its version is not 1 or its theme
/// is not `theme`.
pub fn parse_icon_cache(
    bytes: &[u8],
    theme: &str,
) -> Option<FxHashMap<(Arc<str>, u32), Option<Arc<str>>>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut version = None;
    let mut file_theme = None;
    let mut in_paths = false;
    let mut map = FxHashMap::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if line != "[paths]" || in_paths {
                return None;
            }
            in_paths = true;
            continue;
        }
        if in_paths {
            let (key, rest) = parse_string(line)?;
            let path = parse_string(rest.trim_start().strip_prefix('=')?.trim_start())
                .and_then(|(path, rest)| rest.trim().is_empty().then_some(path))?;
            let (name, size) = key.rsplit_once('@')?;
            let size: u32 = size.parse().ok()?;
            let path = (!path.is_empty()).then(|| Arc::from(path));
            map.insert((Arc::from(name), size), path);
            continue;
        }
        let (key, value) = line.split_once('=')?;
        match key.trim_end() {
            "version" if version.is_none() => version = Some(value.trim()),
            "theme" if file_theme.is_none() => {
                let (t, rest) = parse_string(value.trim_start())?;
                if !rest.trim().is_empty() {
                    return None;
                }
                file_theme = Some(t);
            }
            _ => return None,
        }
    }
    (version == Some("1") && file_theme.as_deref() == Some(theme)).then_some(map)
}

/// Parses a quoted string at the start of `s` with the `\\` and `\"` escapes. Returns the
/// value and the text after the closing quote. `None` for any other escape, a control
/// character or a missing closing quote.
fn parse_string(s: &str) -> Option<(String, &str)> {
    let mut chars = s.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some((out, chars.as_str())),
            '\\' => match chars.next()? {
                c @ ('\\' | '"') => out.push(c),
                _ => return None,
            },
            c if c.is_control() => return None,
            c => out.push(c),
        }
    }
    None
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// The ADR-011 atomic write steps. `tl-collect` cannot depend on `tl-ops`, so they are
/// repeated here.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    if !dir.is_dir() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let mut tmp_name = path.file_name().unwrap_or_default().to_owned();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = dir.join(tmp_name);
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[derive(Default)]
struct RawDir<'a> {
    size: Option<&'a [u8]>,
    scale: Option<&'a [u8]>,
    kind: Option<&'a [u8]>,
}

fn parse_u32(raw: &[u8]) -> Option<u32> {
    std::str::from_utf8(raw).ok()?.trim().parse().ok()
}

/// True for a non-empty single path component other than `.` and `..`.
fn is_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

/// Reads at most [`INDEX_MAX`] bytes of `path` into `buf`. False when the file cannot be read.
fn read_capped(path: &Path, buf: &mut Vec<u8>) -> bool {
    buf.clear();
    match File::open(path) {
        Ok(f) => f.take(INDEX_MAX).read_to_end(buf).is_ok(),
        Err(_) => false,
    }
}
