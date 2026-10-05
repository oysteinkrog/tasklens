//! Group processes into apps by cgroup: the app id from the unit names under
//! `user@<uid>.service/app.slice/`, and the scan of that slice (PROC-005, ADR-016, R-137).
//! Also the Flatpak `bwrap` unwrap for app-level action targets (PROC-060, ADR-006).

use std::collections::VecDeque;
use std::sync::Arc;

use rustix::fd::BorrowedFd;
use smallvec::SmallVec;
use tl_model::ids::ProcessKey;
use tl_model::process::ProcessRecord;

use crate::proc::cgroup::{CgroupRoot, unescape_unit};
use crate::proc::meta::ReadBufs;
use crate::sysroot::read_at;

/// Which launcher naming form a unit name used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppUnitKind {
    /// `app-<rest>-<rand>.scope`.
    Scope,
    /// `app-<id>@<rand>.service`.
    KdeService,
    /// `flatpak-<id>-<n>.scope`.
    Flatpak,
    /// `dbus-:<n>-<id>@<n>.service`.
    DbusActivated,
    /// `snap.<a>.<b>-<uuid>.scope`.
    Snap,
}

/// The app id parsed from one app.slice unit name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitAppId {
    /// The unescaped app id. For `Scope`, the whole remainder after `app-`.
    pub app_id: Arc<str>,
    /// For `Scope` only: `(launcher, id)` from splitting the remainder at its first `-`. The
    /// desktop index resolver (E05-A2) uses it when `app_id` is not a desktop-file id.
    pub alt: Option<(Arc<str>, Arc<str>)>,
    pub kind: AppUnitKind,
    /// True for `app-<id>@autostart.service`.
    pub autostart: bool,
}

/// Parses an escaped unit name into its app id. The first matching form wins. Returns `None` for
/// any other name; such a unit stays a Background unit row (PROC-005).
pub fn parse_app_unit(unit: &str) -> Option<UnitAppId> {
    let mut text = String::new();
    if let Some(body) = strip(unit, "app-", ".scope") {
        // `<rest>-<rand>`, then drop an `@<instance>` part of `<rest>`.
        let (rest, rand) = body.rsplit_once('-')?;
        let rest = rest.split_once('@').map_or(rest, |(r, _)| r);
        if rest.is_empty() || rand.is_empty() {
            return None;
        }
        let alt = rest
            .split_once('-')
            .filter(|(l, id)| !l.is_empty() && !id.is_empty())
            .map(|(l, id)| (unescaped(l, &mut text), unescaped(id, &mut text)));
        return Some(UnitAppId {
            app_id: unescaped(rest, &mut text),
            alt,
            kind: AppUnitKind::Scope,
            autostart: false,
        });
    }
    if let Some(body) = strip(unit, "app-", ".service") {
        let (id, rand) = body.split_once('@')?;
        return plain(id, AppUnitKind::KdeService, rand == "autostart", &mut text);
    }
    if let Some(body) = strip(unit, "flatpak-", ".scope") {
        let (id, n) = body.rsplit_once('-')?;
        if n.is_empty() {
            return None;
        }
        return plain(id, AppUnitKind::Flatpak, false, &mut text);
    }
    if let Some(body) = strip(unit, "dbus-:", ".service") {
        let (_, rest) = body.split_once('-')?;
        let (id, _) = rest.split_once('@')?;
        return plain(id, AppUnitKind::DbusActivated, false, &mut text);
    }
    if let Some(body) = strip(unit, "snap.", ".scope") {
        let (a, rest) = body.split_once('.')?;
        let b = strip_snap_uuid(rest)?;
        if a.is_empty() || b.is_empty() {
            return None;
        }
        unescape_unit(a, &mut text);
        let mut id = std::mem::take(&mut text);
        id.push('_');
        unescape_unit(b, &mut text);
        id.push_str(&text);
        return Some(UnitAppId {
            app_id: Arc::from(id),
            alt: None,
            kind: AppUnitKind::Snap,
            autostart: false,
        });
    }
    None
}

/// Units in app.slice that are never apps.
pub const NOT_APPS: &[&str] = &["dconf.service", "obex.service", "dbus.socket"];

/// True for the names in [`NOT_APPS`], for `run-*.scope`, and for any unit whose name does not
/// start with `app-`, `flatpak-`, `dbus-:` or `snap.`.
pub fn is_not_app(unit: &str) -> bool {
    NOT_APPS.contains(&unit)
        || (unit.starts_with("run-") && unit.ends_with(".scope"))
        || !["app-", "flatpak-", "dbus-:", "snap."]
            .iter()
            .any(|p| unit.starts_with(p))
}

/// The app units found by one [`scan_app_slice`].
#[derive(Debug, Default)]
pub struct AppSliceScan {
    pub units: Vec<AppUnit>,
}

/// One app unit in app.slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppUnit {
    /// The cgroup path relative to `/sys/fs/cgroup`, unit name still escaped.
    pub rel: Arc<str>,
    /// The inode of the unit's cgroup directory.
    pub cgroup_id: u64,
    pub id: UnitAppId,
    /// The pids of the unit and all its descendant cgroups.
    pub pids: SmallVec<[u32; 8]>,
}

/// Lists the app units of `user@{uid}.service/app.slice/` into `out`, which is cleared first,
/// sorted by `rel`. Runs every sweep; there is no inotify watch (R-163 item 3). Entries that do
/// not parse, are not apps, or cannot be read are skipped.
pub fn scan_app_slice(cg: &CgroupRoot, uid: u32, bufs: &mut ReadBufs, out: &mut AppSliceScan) {
    out.units.clear();
    let slice = format!("user.slice/user-{uid}.slice/user@{uid}.service/app.slice");
    let mut names = Vec::new();
    if cg.child_dirs(&slice, &mut names).is_err() {
        return;
    }
    let mut pids = Vec::new();
    for name in names {
        if !(name.ends_with(".scope") || name.ends_with(".service") || name.ends_with(".slice"))
            || is_not_app(&name)
        {
            continue;
        }
        let Some(id) = parse_app_unit(&name) else {
            continue;
        };
        let rel = format!("{slice}/{name}");
        pids.clear();
        if cg
            .read_procs(&rel, true, &mut bufs.small, &mut pids)
            .is_err()
        {
            continue;
        }
        let Ok(cgroup_id) = cg.dir_ino(&rel) else {
            continue;
        };
        out.units.push(AppUnit {
            rel: Arc::from(rel),
            cgroup_id,
            id,
            pids: SmallVec::from_slice(&pids),
        });
    }
    out.units.sort_unstable_by(|a, b| a.rel.cmp(&b.rel));
}

fn strip<'a>(unit: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    unit.strip_prefix(prefix)?.strip_suffix(suffix)
}

fn unescaped(s: &str, text: &mut String) -> Arc<str> {
    unescape_unit(s, text);
    Arc::from(text.as_str())
}

fn plain(id: &str, kind: AppUnitKind, autostart: bool, text: &mut String) -> Option<UnitAppId> {
    if id.is_empty() {
        return None;
    }
    Some(UnitAppId {
        app_id: unescaped(id, text),
        alt: None,
        kind,
        autostart,
    })
}

/// `<b>-<uuid>` to `<b>`. A full `8-4-4-4-12` hex uuid is removed whole, since `<b>` may hold a
/// `-` too; any other suffix is the last `-`-separated token.
fn strip_snap_uuid(rest: &str) -> Option<&str> {
    const UUID_LEN: usize = 36;
    if let Some(cut) = rest.len().checked_sub(UUID_LEN + 1)
        && rest.is_char_boundary(cut)
        && let Some(uuid) = rest[cut..].strip_prefix('-')
        && is_uuid(uuid)
    {
        return Some(&rest[..cut]);
    }
    rest.rsplit_once('-').map(|(b, _)| b)
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// True when the exe basename is `bwrap`, or when the exe is unreadable and `comm` is `bwrap`.
pub fn is_bwrap(exe_basename: Option<&str>, comm: &str) -> bool {
    match exe_basename {
        Some(base) => base == "bwrap",
        None => comm == "bwrap",
    }
}

/// Replaces each wrapper root with the first non-wrapper descendant on each of its branches,
/// searched breadth-first in `records`. A child is a record with `ppid == parent.pid` and
/// `start_ticks >= parent.start_ticks`, so a reused pid is never followed (ADR-006). A
/// non-wrapper root, or one not in `records`, is returned as is; a wrapper with no non-wrapper
/// descendant is dropped. Duplicates are removed and the order follows `roots`.
pub fn unwrap_bwrap_targets(
    roots: &[ProcessKey],
    records: &[ProcessRecord],
    is_wrapper: impl Fn(&ProcessRecord) -> bool,
) -> SmallVec<[ProcessKey; 4]> {
    let mut out: SmallVec<[ProcessKey; 4]> = SmallVec::new();
    let mut push = |key: ProcessKey| {
        if !out.contains(&key) {
            out.push(key);
        }
    };
    let mut queue = VecDeque::new();
    for root in roots {
        let Some(rec) = records.iter().find(|r| r.key == *root) else {
            push(*root);
            continue;
        };
        if !is_wrapper(rec) {
            push(*root);
            continue;
        }
        // Each direct child starts one branch, in record order.
        for child in children(rec.key, records) {
            queue.clear();
            queue.push_back(child);
            // Every record is visited at most once per branch, even for a malformed cycle.
            let mut steps = records.len();
            while let Some(node) = queue.pop_front() {
                if steps == 0 {
                    break;
                }
                steps -= 1;
                if is_wrapper(node) {
                    queue.extend(children(node.key, records));
                } else {
                    push(node.key);
                }
            }
        }
    }
    out
}

fn children(parent: ProcessKey, records: &[ProcessRecord]) -> impl Iterator<Item = &ProcessRecord> {
    records.iter().filter(move |r| {
        r.ppid == parent.pid
            && r.key.boot_id == parent.boot_id
            && r.key.start_ticks >= parent.start_ticks
            && r.key != parent
    })
}

/// Upper bound on one `task/<pid>/children` read.
const CHILDREN_LIMIT: usize = 64 << 10;

/// Reads the child pids of `pid` from `task/<pid>/children`, relative to the `/proc/<pid>`
/// dirfd `dir`, into `out` (cleared first). Used when the record list is stale. A missing file
/// (kernel without `CONFIG_PROC_CHILDREN`) or any read error leaves `out` empty.
pub fn children_from_proc(
    dir: BorrowedFd<'_>,
    pid: u32,
    buf: &mut Vec<u8>,
    out: &mut SmallVec<[u32; 8]>,
) {
    out.clear();
    let Ok((_, truncated)) = read_at(dir, &format!("task/{pid}/children"), buf, CHILDREN_LIMIT)
    else {
        return;
    };
    if truncated {
        // Drop the last token, which may be a cut pid.
        let keep = buf.iter().rposition(u8::is_ascii_whitespace).unwrap_or(0);
        buf.truncate(keep);
    }
    let pids = buf
        .split(|b| b.is_ascii_whitespace())
        .filter_map(|tok| std::str::from_utf8(tok).ok()?.parse().ok());
    out.extend(pids);
}
