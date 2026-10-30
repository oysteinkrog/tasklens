//! Memory groups for the memory-by-app chart (D-036, UI-PRF-060).
//!
//! These groups are not the table's `GroupKind`. An app scope keeps only the processes whose
//! executable sits beside the app's own; every other process of the user groups by program. Each
//! process gets the first rule that applies:
//!
//! 1. System: kernel threads and processes of other users. Key `system`.
//! 2. Wine: a Windows program of the current user, by its install folder per Wine prefix (see
//!    [`wine_app`]). Key `wine:<prefix>:<folder>`. Wine's own processes (`C:\windows\...`)
//!    join the app of their prefix with the most processes, else form `wine:<prefix>`, named
//!    "Wine (<prefix folder name>)".
//! 3. Unknown: the executable cannot be read. Key `comm:<comm>`.
//! 4. App: in an app scope, executable in the directory of the scope's oldest process. Key
//!    `app:<app id>`.
//! 5. Program: any other process of the current user with a readable executable. Key
//!    `exe:<name>`.
//!
//! Keys are stable across restarts and reboots, so the stored history continues a group.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_model::process::WineProcess;

use crate::apps::cgroup_app::is_bwrap;
use crate::apps::naming::{exe_basename, windows_file_name};

/// Key of the System group.
pub const SYSTEM_KEY: &str = "system";
/// Display name of the System group.
pub const SYSTEM_NAME: &str = "System";

/// Display name of a Wine prefix group without an app: "Wine (<prefix folder name>)".
pub const WINE_NAME: &str = "Wine";

/// Program name to display name. Names not in the table show as they are.
pub const PROGRAM_NAMES: &[(&str, &str)] = &[("claude", "Claude Code")];

/// Directories that hold the executables of many unrelated programs. An app whose executable is
/// in one of these keeps only the processes that run the same file: a shell in
/// `/usr/bin` started from Konsole in `/usr/bin` is not part of Konsole.
pub const SHARED_BIN_DIRS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/games",
];

/// The directory name that the version rule looks for.
const VERSIONS_DIR: &str = "versions";

/// Suffix the kernel adds to `/proc/<pid>/exe` when the file was removed.
const DELETED_SUFFIX: &str = " (deleted)";

/// Which rule gave a memory group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemGroupKind {
    App,
    Program,
    System,
    Unknown,
    Wine,
}

/// One memory group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemGroup {
    /// `app:<app id>`, `exe:<name>`, `wine:<prefix>[:<folder>]`, `system` or `comm:<comm>`.
    pub key: Arc<str>,
    /// The desktop entry name, the program's display name, the Wine install folder,
    /// "Wine (<prefix folder name>)", "System" or the `comm`.
    pub name: Arc<str>,
    pub kind: MemGroupKind,
    /// The desktop id of an `App` group, for its icon.
    pub app_id: Option<Arc<str>>,
    /// The Wine prefix of a `Wine` group, when known.
    pub wine_prefix: Option<Arc<str>>,
}

/// What the grouping needs to know about one process.
#[derive(Clone, Copy, Debug)]
pub struct MemGroupInput<'a> {
    pub pid: u32,
    /// `/proc/<pid>/stat` field 22. The lowest value in a scope is its oldest process.
    pub start_ticks: u64,
    /// Real uid from status `Uid:`.
    pub ruid: u32,
    pub kernel_thread: bool,
    pub comm: &'a str,
    /// The `/proc/<pid>/exe` link target, `None` when it cannot be read.
    pub exe: Option<&'a str>,
    /// The app id when the process is `GroupKind::App`.
    pub app_id: Option<&'a str>,
    /// The desktop entry name of that app.
    pub app_name: Option<&'a str>,
    /// The app unit the process runs in (its scope or service). Processes with the same app id
    /// and unit share one oldest process.
    pub unit: Option<&'a str>,
    /// Set for a Wine process: its prefix and Windows image.
    pub wine: Option<&'a WineProcess>,
}

/// What a Wine process's Windows image says about its group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WineApp<'a> {
    /// An app, by its install folder or, outside one, its file name without `.exe`.
    App(&'a str),
    /// Wine's own processes in `<drive>:\windows`, and a process whose image is not known.
    System,
}

/// Windows folders that hold the install folders of many apps.
const WINE_CONTAINERS: &[&str] = &["ProgramData"];
/// Folders under `users\<name>` that hold install folders, in order. Each is skipped when it
/// is there.
const WINE_USER_CONTAINERS: &[&[&str]] = &[
    &["AppData"],
    &["Local", "Roaming", "LocalLow"],
    &["Programs"],
];

/// The executable of an app unit's oldest process.
#[derive(Clone, Copy)]
struct Reference<'a> {
    start_ticks: u64,
    pid: u32,
    exe: &'a str,
}

/// Puts processes into memory groups. Holds one `MemGroup` per key, so a sweep in steady state
/// allocates only its map of app units.
#[derive(Default)]
pub struct MemGrouper {
    groups: FxHashMap<Box<str>, (Arc<MemGroup>, u64)>,
    sweep: u64,
    key: String,
    name: String,
}

impl MemGrouper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes the memory group of each process in `procs` into `out`, in the same order. `out`
    /// is cleared first. Groups no process used in this call are dropped. `self_uid` is
    /// `getuid()`.
    pub fn classify(
        &mut self,
        procs: &[MemGroupInput<'_>],
        self_uid: u32,
        out: &mut Vec<Arc<MemGroup>>,
    ) {
        self.sweep += 1;
        out.clear();
        let refs = references(procs, self_uid);
        let wine_main = wine_main_apps(procs, self_uid);
        for p in procs {
            let reference = p
                .app_id
                .and_then(|id| refs.get(&(id, p.unit)))
                .map(|r| r.exe);
            let kind = group_of(
                p,
                reference,
                &wine_main,
                self_uid,
                &mut self.key,
                &mut self.name,
            );
            let prefix = p.wine.and_then(|w| w.prefix.as_ref());
            out.push(self.intern(kind, p.app_id, prefix));
        }
        let sweep = self.sweep;
        self.groups.retain(|_, (_, seen)| *seen == sweep);
    }

    fn intern(
        &mut self,
        kind: MemGroupKind,
        app_id: Option<&str>,
        wine_prefix: Option<&Arc<str>>,
    ) -> Arc<MemGroup> {
        let name = self.name.as_str();
        // A renamed desktop entry gives a new group with the same key.
        if let Some((group, seen)) = self.groups.get_mut(self.key.as_str())
            && &*group.name == name
        {
            *seen = self.sweep;
            return group.clone();
        }
        let group = Arc::new(MemGroup {
            key: Arc::from(self.key.as_str()),
            name: Arc::from(name),
            kind,
            app_id: app_id.filter(|_| kind == MemGroupKind::App).map(Arc::from),
            wine_prefix: wine_prefix.filter(|_| kind == MemGroupKind::Wine).cloned(),
        });
        self.groups
            .insert(Box::from(self.key.as_str()), (group.clone(), self.sweep));
        group
    }
}

/// The oldest process of each (app id, unit) with a readable executable. A Flatpak `bwrap`
/// wrapper is skipped, so the app's own executable is the reference.
fn references<'a>(
    procs: &[MemGroupInput<'a>],
    self_uid: u32,
) -> FxHashMap<(&'a str, Option<&'a str>), Reference<'a>> {
    let mut refs: FxHashMap<(&str, Option<&str>), Reference<'_>> = FxHashMap::default();
    for p in procs {
        let (Some(app_id), Some(exe)) = (p.app_id, p.exe) else {
            continue;
        };
        if p.kernel_thread || p.ruid != self_uid || is_bwrap(Some(exe_basename(exe)), p.comm) {
            continue;
        }
        let cand = Reference {
            start_ticks: p.start_ticks,
            pid: p.pid,
            exe,
        };
        refs.entry((app_id, p.unit))
            .and_modify(|r| {
                if (cand.start_ticks, cand.pid) < (r.start_ticks, r.pid) {
                    *r = cand;
                }
            })
            .or_insert(cand);
    }
    refs
}

/// The install folder of the app with the most processes in each Wine prefix of the current
/// user. Equal counts go to the folder that sorts first.
fn wine_main_apps<'a>(
    procs: &[MemGroupInput<'a>],
    self_uid: u32,
) -> FxHashMap<Option<&'a str>, &'a str> {
    let mut counts: FxHashMap<(Option<&'a str>, &'a str), u32> = FxHashMap::default();
    for p in procs {
        let Some(wine) = p.wine else { continue };
        if p.kernel_thread || p.ruid != self_uid {
            continue;
        }
        if let WineApp::App(folder) = wine_app(wine.image.as_deref()) {
            *counts.entry((wine.prefix.as_deref(), folder)).or_default() += 1;
        }
    }
    let mut best: FxHashMap<Option<&str>, (&str, u32)> = FxHashMap::default();
    for ((prefix, folder), n) in counts {
        best.entry(prefix)
            .and_modify(|b| {
                if n > b.1 || (n == b.1 && folder < b.0) {
                    *b = (folder, n);
                }
            })
            .or_insert((folder, n));
    }
    best.into_iter()
        .map(|(prefix, (folder, _))| (prefix, folder))
        .collect()
}

/// The group of one process. Writes its key into `key` and its display name into `name`, both
/// cleared first. `reference` is the executable of the oldest process of the process's app
/// unit. `wine_main` is the main app folder per Wine prefix.
fn group_of(
    p: &MemGroupInput<'_>,
    reference: Option<&str>,
    wine_main: &FxHashMap<Option<&str>, &str>,
    self_uid: u32,
    key: &mut String,
    name: &mut String,
) -> MemGroupKind {
    key.clear();
    name.clear();
    if p.kernel_thread || p.ruid != self_uid {
        key.push_str(SYSTEM_KEY);
        name.push_str(SYSTEM_NAME);
        return MemGroupKind::System;
    }
    if let Some(wine) = p.wine {
        let prefix = wine.prefix.as_deref();
        key.push_str("wine:");
        key.push_str(prefix.unwrap_or(""));
        let folder = match wine_app(wine.image.as_deref()) {
            WineApp::App(folder) => Some(folder),
            WineApp::System => wine_main.get(&prefix).copied(),
        };
        match folder {
            Some(folder) => {
                key.push(':');
                key.push_str(folder);
                name.push_str(folder);
            }
            None => {
                name.push_str(WINE_NAME);
                if let Some(dir) = prefix.and_then(|p| p.rsplit('/').next()) {
                    name.push_str(" (");
                    name.push_str(dir);
                    name.push(')');
                }
            }
        }
        return MemGroupKind::Wine;
    }
    let Some(exe) = p.exe else {
        key.push_str("comm:");
        key.push_str(p.comm);
        name.push_str(p.comm);
        return MemGroupKind::Unknown;
    };
    if let (Some(app_id), Some(reference)) = (p.app_id, reference)
        && beside(exe, reference)
    {
        key.push_str("app:");
        key.push_str(app_id);
        name.push_str(p.app_name.unwrap_or(app_id));
        return MemGroupKind::App;
    }
    let program = program_name(exe);
    key.push_str("exe:");
    key.push_str(program);
    name.push_str(program_display_name(program));
    MemGroupKind::Program
}

/// The app a Wine process's Windows image belongs to.
///
/// - `<drive>:\windows\...` and an unknown image: [`WineApp::System`].
/// - In `Program Files`, `Program Files (x86)`, `ProgramData` or
///   `users\<name>\AppData\{Local,Roaming,LocalLow}\Programs`: the first folder inside it.
///   `C:\Program Files\Backblaze\bztransmit.exe` gives `Backblaze`.
/// - In any other folder of a drive: that drive's first-level folder.
/// - On drive `Z:`, which Wine maps to the Unix root, outside a folder, or with no drive: the
///   file name without `.exe`.
pub fn wine_app(image: Option<&str>) -> WineApp<'_> {
    let Some(image) = image else {
        return WineApp::System;
    };
    let file = windows_file_name(image);
    let stem = WineApp::App(
        file.len()
            .checked_sub(4)
            .filter(|&n| file.is_char_boundary(n) && file[n..].eq_ignore_ascii_case(".exe"))
            .map_or(file, |n| &file[..n]),
    );
    let dir = &image[..image.len() - file.len()];
    let mut dirs = dir.split(['\\', '/']).filter(|s| !s.is_empty());
    let is_drive = |d: &str| {
        let b = d.as_bytes();
        b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    };
    match dirs.next() {
        Some(drive) if is_drive(drive) && !drive.eq_ignore_ascii_case("z:") => {}
        _ => return stem,
    }
    let Some(first) = dirs.next() else {
        return stem;
    };
    let folder = if first.eq_ignore_ascii_case("windows") {
        return WineApp::System;
    } else if starts_with_ignore_case(first, "Program Files")
        || WINE_CONTAINERS
            .iter()
            .any(|c| first.eq_ignore_ascii_case(c))
    {
        dirs.next()
    } else if first.eq_ignore_ascii_case("users") {
        // The user name.
        dirs.next();
        let mut next = dirs.next();
        for names in WINE_USER_CONTAINERS {
            if next.is_some_and(|d| names.iter().any(|n| d.eq_ignore_ascii_case(n))) {
                next = dirs.next();
            }
        }
        next
    } else {
        Some(first)
    };
    folder.map_or(stem, WineApp::App)
}

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// True when `exe` is in the same directory as `reference`. In a [`SHARED_BIN_DIRS`] directory
/// it must be the same file.
pub fn beside(exe: &str, reference: &str) -> bool {
    let exe = strip_deleted(exe);
    let reference = strip_deleted(reference);
    let dir = exe_dir(exe);
    if dir != exe_dir(reference) {
        return false;
    }
    !SHARED_BIN_DIRS.contains(&dir) || exe == reference
}

/// The program name of an executable path: its file name, or for a version-number file name
/// (`2.1.309`) inside a `versions` directory the directory above that.
/// `/home/u/.local/share/claude/versions/2.1.309` gives `claude`.
pub fn program_name(exe: &str) -> &str {
    let exe = strip_deleted(exe);
    let base = exe_basename(exe);
    if !is_version(base) {
        return base;
    }
    let mut dirs = exe_dir(exe).rsplit('/');
    if dirs.next() == Some(VERSIONS_DIR)
        && let Some(above) = dirs.next()
        && !above.is_empty()
    {
        return above;
    }
    base
}

/// The display name of a program name: from [`PROGRAM_NAMES`], else the name itself.
pub fn program_display_name(name: &str) -> &str {
    PROGRAM_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(name, |(_, display)| display)
}

/// True for a file name of digits and dots only, with at least one digit.
fn is_version(name: &str) -> bool {
    name.bytes().any(|b| b.is_ascii_digit())
        && name.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

fn strip_deleted(exe: &str) -> &str {
    exe.strip_suffix(DELETED_SUFFIX).unwrap_or(exe)
}

/// The part of `exe` before its last `/`. Empty for a path without one or at the root.
fn exe_dir(exe: &str) -> &str {
    exe.rsplit_once('/').map_or("", |(dir, _)| dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_names() {
        assert!(is_version("2.1.309"));
        assert!(is_version("12"));
        assert!(!is_version("..."));
        assert!(!is_version(""));
        assert!(!is_version("2.1.309-beta"));
    }

    #[test]
    fn program_names() {
        assert_eq!(program_name("/usr/bin/rustc"), "rustc");
        assert_eq!(
            program_name("/home/u/.local/share/claude/versions/2.1.309"),
            "claude"
        );
        assert_eq!(
            program_name("/home/u/.local/share/claude/versions/2.1.309 (deleted)"),
            "claude"
        );
        // A version name outside a `versions` directory keeps its file name.
        assert_eq!(program_name("/opt/tool/2.0"), "2.0");
        assert_eq!(program_name("/versions/2.0"), "2.0");
        assert_eq!(program_display_name("claude"), "Claude Code");
        assert_eq!(program_display_name("node"), "node");
    }

    #[test]
    fn wine_apps() {
        let app = |image| wine_app(Some(image));
        assert_eq!(
            app("C:\\Program Files\\Backblaze\\bztransmit.exe"),
            WineApp::App("Backblaze")
        );
        assert_eq!(
            app("c:\\program files (x86)\\Steam\\bin\\steam.exe"),
            WineApp::App("Steam")
        );
        assert_eq!(app("C:\\ProgramData\\Tool\\t.exe"), WineApp::App("Tool"));
        assert_eq!(
            app("C:\\users\\u\\AppData\\Local\\Programs\\Code\\code.exe"),
            WineApp::App("Code")
        );
        assert_eq!(
            app("C:\\users\\u\\AppData\\Roaming\\Zoom\\bin\\zoom.exe"),
            WineApp::App("Zoom")
        );
        assert_eq!(app("C:\\Games\\Foo\\foo.exe"), WineApp::App("Games"));
        assert_eq!(app("D:/Backblaze/bzbui.exe"), WineApp::App("Backblaze"));
        assert_eq!(app("C:\\windows\\system32\\services.exe"), WineApp::System);
        assert_eq!(app("C:\\WINDOWS\\explorer.exe"), WineApp::System);
        assert_eq!(wine_app(None), WineApp::System);
        // Outside a folder, on the Unix root drive or with no path: the file name.
        assert_eq!(app("C:\\setup.exe"), WineApp::App("setup"));
        assert_eq!(app("C:\\Program Files\\x.EXE"), WineApp::App("x"));
        assert_eq!(app("Z:\\home\\u\\bin\\Tool.exe"), WineApp::App("Tool"));
        assert_eq!(app("UiMcpHost.exe"), WineApp::App("UiMcpHost"));
    }

    #[test]
    fn beside_rules() {
        assert!(beside(
            "/opt/google/chrome/chrome_crashpad_handler",
            "/opt/google/chrome/chrome"
        ));
        assert!(!beside("/usr/bin/bash", "/usr/bin/konsole"));
        assert!(beside("/usr/bin/konsole", "/usr/bin/konsole"));
        assert!(beside("/usr/bin/konsole (deleted)", "/usr/bin/konsole"));
        assert!(!beside(
            "/opt/google/chrome/sub/x",
            "/opt/google/chrome/chrome"
        ));
    }
}
