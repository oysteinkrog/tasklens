//! Memory groups for the memory-by-app chart (D-036, UI-PRF-060).
//!
//! These groups are not the table's `GroupKind`. An app scope keeps only the processes whose
//! executable sits beside the app's own; every other process of the user groups by program. Each
//! process gets the first rule that applies:
//!
//! 1. App: in an app scope, executable in the directory of the scope's oldest process. Key
//!    `app:<app id>`.
//! 2. Program: any other process of the current user with a readable executable. Key
//!    `exe:<name>`.
//! 3. System: kernel threads and processes of other users. Key `system`.
//! 4. Unknown: the executable cannot be read. Key `comm:<comm>`.
//!
//! Keys are stable across restarts and reboots, so the stored history continues a group.

use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::apps::cgroup_app::is_bwrap;
use crate::apps::naming::exe_basename;

/// Key of the System group.
pub const SYSTEM_KEY: &str = "system";
/// Display name of the System group.
pub const SYSTEM_NAME: &str = "System";

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
}

/// One memory group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemGroup {
    /// `app:<app id>`, `exe:<name>`, `system` or `comm:<comm>`.
    pub key: Arc<str>,
    /// The desktop entry name, the program's display name, "System" or the `comm`.
    pub name: Arc<str>,
    pub kind: MemGroupKind,
    /// The desktop id of an `App` group, for its icon.
    pub app_id: Option<Arc<str>>,
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
}

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
        for p in procs {
            let reference = p
                .app_id
                .and_then(|id| refs.get(&(id, p.unit)))
                .map(|r| r.exe);
            let (kind, name) = group_of(p, reference, self_uid, &mut self.key);
            out.push(self.intern(kind, name, p.app_id));
        }
        let sweep = self.sweep;
        self.groups.retain(|_, (_, seen)| *seen == sweep);
    }

    fn intern(
        &mut self,
        kind: MemGroupKind,
        name: GroupName<'_>,
        app_id: Option<&str>,
    ) -> Arc<MemGroup> {
        // A renamed desktop entry gives a new group with the same key.
        if let Some((group, seen)) = self.groups.get_mut(self.key.as_str())
            && &*group.name == name.as_str()
        {
            *seen = self.sweep;
            return group.clone();
        }
        let group = Arc::new(MemGroup {
            key: Arc::from(self.key.as_str()),
            name: Arc::from(name.as_str()),
            kind,
            app_id: (kind == MemGroupKind::App)
                .then(|| app_id.map(Arc::from))
                .flatten(),
        });
        self.groups
            .insert(Box::from(self.key.as_str()), (group.clone(), self.sweep));
        group
    }
}

/// The display name of a group, borrowed from the input.
enum GroupName<'a> {
    Text(&'a str),
    System,
}

impl GroupName<'_> {
    fn as_str(&self) -> &str {
        match self {
            GroupName::Text(s) => s,
            GroupName::System => SYSTEM_NAME,
        }
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

/// The group of one process. Writes its key into `key`, which is cleared first. `reference` is
/// the executable of the oldest process of the process's app unit.
fn group_of<'a>(
    p: &MemGroupInput<'a>,
    reference: Option<&str>,
    self_uid: u32,
    key: &mut String,
) -> (MemGroupKind, GroupName<'a>) {
    key.clear();
    if p.kernel_thread || p.ruid != self_uid {
        key.push_str(SYSTEM_KEY);
        return (MemGroupKind::System, GroupName::System);
    }
    let Some(exe) = p.exe else {
        key.push_str("comm:");
        key.push_str(p.comm);
        return (MemGroupKind::Unknown, GroupName::Text(p.comm));
    };
    if let (Some(app_id), Some(reference)) = (p.app_id, reference)
        && beside(exe, reference)
    {
        key.push_str("app:");
        key.push_str(app_id);
        return (
            MemGroupKind::App,
            GroupName::Text(p.app_name.unwrap_or(app_id)),
        );
    }
    let name = program_name(exe);
    key.push_str("exe:");
    key.push_str(name);
    (
        MemGroupKind::Program,
        GroupName::Text(program_display_name(name)),
    )
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
