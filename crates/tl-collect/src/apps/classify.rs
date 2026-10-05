//! Classify processes as app, background or system.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with};
use smallvec::SmallVec;
use tl_model::ids::GroupKey;
use tl_model::process::{GroupKind, ProcFlags};

use crate::apps::cgroup_app::{AppUnitKind, UnitAppId, parse_app_unit};
use crate::apps::desktop::{DesktopEntry, DesktopIndex};
use crate::apps::naming::exe_basename;
use crate::proc::cgroup::{CgroupPath, UserPlacement, app_slice_unit, placement};

// ---- Classification (E05-015) ----

/// The Type column value of a row (PROC-011, PROC-027).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcClass {
    App,
    Background,
    System,
    KernelThread,
    /// A systemd unit row. Only the reducer (E09) makes these.
    Unit,
    /// A pseudo row. Only the reducer (E09) makes these.
    Pseudo,
}

impl ProcClass {
    pub fn type_label(self) -> &'static str {
        match self {
            ProcClass::App => "App",
            ProcClass::Background => "Background process",
            ProcClass::System => "System process",
            ProcClass::KernelThread => "Kernel thread",
            ProcClass::Unit => "Unit",
            ProcClass::Pseudo => "Pseudo",
        }
    }
}

/// What `classify` needs to know about one process.
pub struct ClassifyInput<'a> {
    pub pid: u32,
    /// Real uid from status `Uid:`.
    pub ruid: u32,
    pub kernel_thread: bool,
    pub cgroup: Option<&'a CgroupPath>,
    /// `/proc/<pid>/cgroup` could not be read.
    pub cgroup_denied: bool,
    /// The desktop app this process belongs to (E05-016), if any.
    pub app_id: Option<&'a Arc<str>>,
    /// A KWin window maps to this pid (PROC-030, P2). Always false in P1.
    pub owns_window: bool,
}

/// Classifies one process. First match wins (PROC-004, PROC-005, PROC-011). Returns only
/// `App`, `Background`, `System` and `KernelThread`. `self_uid` is `getuid()`.
pub fn classify(inp: &ClassifyInput<'_>, self_uid: u32) -> (ProcClass, GroupKind) {
    if inp.kernel_thread {
        return (ProcClass::KernelThread, GroupKind::System);
    }
    if inp.ruid != self_uid {
        return (ProcClass::System, GroupKind::System);
    }
    let cgroup = match inp.cgroup {
        Some(c) if !inp.cgroup_denied => c,
        _ => return (ProcClass::Background, GroupKind::Ungrouped),
    };
    match placement(cgroup) {
        UserPlacement::OutsideUserSlice => return (ProcClass::System, GroupKind::System),
        UserPlacement::InUserManager { uid } | UserPlacement::InSessionScope { uid }
            if uid != self_uid =>
        {
            return (ProcClass::System, GroupKind::System);
        }
        _ => {}
    }
    if let Some(app_id) = inp.app_id {
        return (
            ProcClass::App,
            GroupKind::App {
                app_id: app_id.clone(),
            },
        );
    }
    // Rule 6: a process with a KWin window (`owns_window`) in an app.slice unit becomes an App
    // under the app id parsed from the unit name (E05-011). A unit that does not parse stays
    // Background. `owns_window` is always false in P1.
    if inp.owns_window
        && let Some(id) = app_slice_unit(cgroup, self_uid).and_then(parse_app_unit)
    {
        return (ProcClass::App, GroupKind::App { app_id: id.app_id });
    }
    (ProcClass::Background, GroupKind::Background)
}

/// The stable hash of a group (ADR-003, ADR-012). Same value as `GroupKind::key()`, which
/// defines the scheme (R-279).
pub fn group_key(kind: &GroupKind) -> GroupKey {
    kind.key()
}

// ---- App matching (E05-016) ----

/// Exe basename to the desktop id tried first for it. Mission Center 1.2.0's table (R-250). When
/// the mapped id is not in the index, matching goes on with the normal steps.
pub const EXCEPTIONS: &[(&str, &str)] = &[
    ("firefox-bin", "firefox"),
    ("soffice.bin", "libreoffice"),
    ("resources-processes", "resources"),
    ("gnome-terminal-server", "gnome-terminal"),
    ("chrome", "google-chrome-stable"),
];

/// Exe basenames never matched to an app.
pub const STAY_BACKGROUND: &[&str] = &["xdg-desktop-portal-kde", "Xwayland"];

/// Exe basenames never matched to an app. `foot` only when its argv contains `--server` (the
/// foot server); `guake-prefs` always.
pub const IGNORE: &[&str] = &["guake-prefs", "foot"];

/// Command-line tokens that tell apart entries sharing one binary.
const TIE_TOKENS: [&str; 3] = ["--app-id=", "--profile-directory=", "--class="];

/// What `match_app` needs to know about one process. Command lines are read for tokens only and
/// never logged.
pub struct MatchInput<'a> {
    /// The app id of the app.slice unit the process runs in (E05-011).
    pub unit_app: Option<&'a UnitAppId>,
    /// The `/proc/<pid>/exe` link target.
    pub exe: Option<&'a str>,
    pub argv0: Option<&'a str>,
    /// The command line, tokens joined by spaces.
    pub cmdline: Option<&'a str>,
    /// KWin `resourceClass` (P2). Always `None` in P1.
    pub wm_class: Option<&'a str>,
}

/// Finds the desktop entry of the app a process belongs to (PROC-005, PROC-006). Steps in
/// order: the ignore lists, the unit app id, the window class, the exe (with the exceptions
/// table and argv tie-breaking), then the Wine `.exe` name.
pub fn match_app(inp: &MatchInput<'_>, index: &DesktopIndex) -> Option<Arc<DesktopEntry>> {
    let exe = inp.exe.map(|e| e.strip_suffix(" (deleted)").unwrap_or(e));
    let base = exe.map(exe_basename);
    if let Some(base) = base
        && (STAY_BACKGROUND.contains(&base)
            || base == "guake-prefs"
            || (base == "foot" && cmdline_tokens(inp).any(|t| t == "--server")))
    {
        return None;
    }
    if let Some(unit) = inp.unit_app
        && let Some(entry) = match_unit(unit, index)
    {
        return Some(entry.clone());
    }
    if let Some(entry) = inp.wm_class.and_then(|c| index.by_wm_class(c)) {
        return Some(entry.clone());
    }
    if let (Some(exe), Some(base)) = (exe, base) {
        if let Some(entry) = EXCEPTIONS
            .iter()
            .find(|(b, _)| *b == base)
            .and_then(|(_, id)| index.by_id(id))
        {
            return Some(entry.clone());
        }
        match index.by_exe(exe) {
            [] => {}
            [one] => return Some(one.clone()),
            many => return break_tie(inp, many),
        }
    }
    let name = wine_exe(inp)?;
    let mut found = index.entries().filter(|e| wine_entry_matches(e, name));
    match (found.next(), found.next()) {
        (Some(one), None) => Some(one.clone()),
        _ => None,
    }
}

/// The qualifier shown on an app row (PROC-008): `Flatpak` or `Snap` from the unit kind, `Wine`
/// when the process carries a Windows `.exe` name (the key of `match_app`'s Wine step), else
/// `None`. `unit` falls back to `inp.unit_app`.
pub fn qualifier(inp: &MatchInput<'_>, unit: Option<&UnitAppId>) -> Option<&'static str> {
    match unit.or(inp.unit_app).map(|u| u.kind) {
        Some(AppUnitKind::Flatpak) => Some("Flatpak"),
        Some(AppUnitKind::Snap) => Some("Snap"),
        _ => wine_exe(inp).map(|_| "Wine"),
    }
}

/// The unit's app id, then the launcher form's `alt` id (E05-011), then for a Flatpak unit the
/// entry whose `X-Flatpak` equals the id.
fn match_unit<'i>(unit: &UnitAppId, index: &'i DesktopIndex) -> Option<&'i Arc<DesktopEntry>> {
    index
        .by_id(&unit.app_id)
        .or_else(|| unit.alt.as_ref().and_then(|(_, id)| index.by_id(id)))
        .or_else(|| {
            (unit.kind == AppUnitKind::Flatpak)
                .then(|| {
                    index
                        .entries()
                        .find(|e| e.flatpak.as_deref() == Some(&*unit.app_id))
                })
                .flatten()
        })
}

/// Keeps the entries whose `Exec` holds every tie token of the command line with the same value.
/// Exactly one left is the match; none or several is a true tie and stays unmatched.
fn break_tie(inp: &MatchInput<'_>, entries: &[Arc<DesktopEntry>]) -> Option<Arc<DesktopEntry>> {
    let wanted: SmallVec<[&str; 3]> = cmdline_tokens(inp)
        .filter(|t| TIE_TOKENS.iter().any(|p| t.starts_with(p)))
        .collect();
    if wanted.is_empty() {
        return None;
    }
    let mut kept = entries.iter().filter(|e| {
        e.exec.as_deref().is_some_and(|exec| {
            wanted
                .iter()
                .all(|w| exec.split_ascii_whitespace().any(|t| t == *w))
        })
    });
    match (kept.next(), kept.next()) {
        (Some(one), None) => Some(one.clone()),
        _ => None,
    }
}

fn cmdline_tokens<'a>(inp: &MatchInput<'a>) -> impl Iterator<Item = &'a str> {
    inp.cmdline.unwrap_or("").split_ascii_whitespace()
}

/// The Windows `.exe` name: `argv0`, else the first command-line token, ending in `.exe` (any
/// case), cut to its last segment split on `/` and `\`.
fn wine_exe<'a>(inp: &MatchInput<'a>) -> Option<&'a str> {
    let token = inp
        .argv0
        .filter(|a| ends_with_exe(a))
        .or_else(|| cmdline_tokens(inp).find(|t| ends_with_exe(t)))?;
    token.rsplit(['/', '\\']).next()
}

fn wine_entry_matches(entry: &DesktopEntry, name: &str) -> bool {
    let name = name.to_lowercase();
    let by_exec = entry.exec.as_deref().is_some_and(|exec| {
        let exec = exec.to_lowercase();
        exec.contains("wine") && exec.contains(&name)
    });
    by_exec
        || entry
            .startup_wm_class
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case(&name))
}

fn ends_with_exe(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 4 && b[b.len() - 4..].eq_ignore_ascii_case(b".exe")
}

// ---- Process flags and the protected set (E05-017) ----

/// Inputs to the protected set that need sockets or D-Bus. Filled by the engine (E10).
#[derive(Clone, Debug, Default)]
pub struct ProtectInputs {
    /// tasklens's own pid and the `tl-gpu-probe` pid when it runs.
    pub self_pids: SmallVec<[u32; 2]>,
    /// The Wayland compositor, from `wayland_compositor_pid`.
    pub compositor_pid: Option<u32>,
    /// The session bus daemon, from `GetConnectionUnixProcessID("org.freedesktop.DBus")`.
    pub session_bus_pid: Option<u32>,
}

/// The pid of the process that serves the Wayland socket, read with `SO_PEERCRED`. The socket is
/// `<runtime_dir>/<display>`, or `display` itself when it is absolute. An empty `display` means
/// `wayland-0`. Returns `None` when the socket cannot be reached.
pub fn wayland_compositor_pid(runtime_dir: &Path, display: &OsStr) -> Option<u32> {
    let display = if display.is_empty() {
        OsStr::new("wayland-0")
    } else {
        display
    };
    let path = runtime_dir.join(display);
    let addr = SocketAddrUnix::new(path.as_path()).ok()?;
    // Non-blocking so a full listen backlog fails at once instead of stalling the sweep.
    let fd = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .ok()?;
    connect(&fd, &addr).ok()?;
    let cred = rustix::net::sockopt::socket_peercred(&fd).ok()?;
    u32::try_from(cred.pid.as_raw_nonzero().get()).ok()
}

/// The pids that must not be ended or signalled without care (ADR-006, PROC-059).
#[derive(Clone, Debug, Default)]
pub struct ProtectedSet {
    pids: FxHashSet<u32>,
}

impl ProtectedSet {
    pub fn contains(&self, pid: u32) -> bool {
        self.pids.contains(&pid)
    }

    pub fn len(&self) -> usize {
        self.pids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pids.is_empty()
    }
}

/// Session processes protected for the current user only.
const SESSION_NAMES: [&str; 3] = ["plasmashell", "ksmserver", "Xwayland"];

/// Login ancestors of the compositor or of plasmashell.
const LOGIN_NAMES: [&str; 2] = ["sddm", "login"];

/// Longest ppid walk. Real chains are under 10; the cap guards against a ppid loop in racy data.
const MAX_WALK: usize = 64;

/// Builds the protected set from one sweep's records: `(pid, ppid, exe basename or comm,
/// cgroup, ruid)`. Missing inputs give a smaller set, never a panic.
pub fn protected_set(
    records: &[(u32, u32, &str, Option<&CgroupPath>, u32)],
    self_uid: u32,
    inp: &ProtectInputs,
) -> ProtectedSet {
    let mut pids = FxHashSet::default();
    pids.insert(1);
    pids.extend(inp.compositor_pid);
    pids.extend(inp.session_bus_pid);
    pids.extend(inp.self_pids.iter().copied());

    let user_init = format!("user.slice/user-{self_uid}.slice/user@{self_uid}.service/init.scope");
    let mut parents: FxHashMap<u32, (u32, &str)> = FxHashMap::default();
    let mut shells: SmallVec<[u32; 2]> = SmallVec::new();
    for &(pid, ppid, name, cgroup, ruid) in records {
        parents.insert(pid, (ppid, name));
        let rel = cgroup.map(|c| &*c.rel);
        if ruid == self_uid && SESSION_NAMES.contains(&name) {
            pids.insert(pid);
            if name == "plasmashell" {
                shells.push(pid);
            }
        }
        if name == "systemd" && rel == Some(user_init.as_str()) {
            pids.insert(pid);
        }
        if rel == Some("system.slice/systemd-logind.service") {
            pids.insert(pid);
        }
    }

    let starts: SmallVec<[u32; 2]> = match inp.compositor_pid {
        Some(pid) => SmallVec::from_slice(&[pid]),
        None => shells,
    };
    for start in starts {
        let mut pid = start;
        for _ in 0..MAX_WALK {
            let Some(&(ppid, _)) = parents.get(&pid) else {
                break;
            };
            if ppid <= 1 || ppid == pid {
                break;
            }
            if let Some(&(_, name)) = parents.get(&ppid)
                && LOGIN_NAMES.contains(&name)
            {
                pids.insert(ppid);
            }
            pid = ppid;
        }
    }
    ProtectedSet { pids }
}

/// The flags this module owns for one process. `NEW` and `EXITED` belong to the reducer (E09)
/// and are never set here.
pub fn flags_for(
    pid: u32,
    kernel_thread: bool,
    ruid: u32,
    self_uid: u32,
    set: &ProtectedSet,
    inp: &ProtectInputs,
) -> ProcFlags {
    let mut flags = ProcFlags::empty();
    flags.set(ProcFlags::KERNEL_THREAD, kernel_thread);
    flags.set(ProcFlags::OTHER_USER, ruid != self_uid);
    flags.set(ProcFlags::IS_SELF, inp.self_pids.contains(&pid));
    flags.set(ProcFlags::PROTECTED, set.contains(pid));
    flags
}
