//! Parser for /proc/<pid>/cgroup and cgroup v2 placement.

use std::sync::Arc;

use smallvec::SmallVec;

/// The cgroup v2 path of a process, split into the parts the grouping needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CgroupPath {
    /// The v2 path without the leading `/`, relative to `/sys/fs/cgroup`. Empty for the root.
    pub rel: Arc<str>,
    /// The innermost component that ends in `.service` or `.scope`, still escaped.
    pub unit: Option<Arc<str>>,
    /// The components that end in `.slice`, outermost first.
    pub slices: SmallVec<[Arc<str>; 4]>,
}

/// Parses `/proc/<pid>/cgroup`. Uses the `0::` line only. Returns `None` when there is no such
/// line (cgroup v1 only, or an empty file), or when its path is not absolute or not UTF-8.
pub fn parse_cgroup_file(buf: &[u8]) -> Option<CgroupPath> {
    let line = buf
        .split(|&b| b == b'\n')
        .find_map(|line| line.strip_prefix(b"0::"))?;
    let path = std::str::from_utf8(line).ok()?;
    let rel = path.strip_prefix('/')?.trim_end_matches('/');
    let mut unit = None;
    let mut slices = SmallVec::new();
    for comp in rel.split('/').filter(|c| !c.is_empty()) {
        if comp.ends_with(".slice") {
            slices.push(Arc::from(comp));
        } else if comp.ends_with(".service") || comp.ends_with(".scope") {
            unit = Some(comp);
        }
    }
    Some(CgroupPath {
        rel: Arc::from(rel),
        unit: unit.map(Arc::from),
        slices,
    })
}

/// Replaces every systemd `\xNN` escape (two hex digits) in `name` with that byte and writes the
/// result to `out`, which is cleared first. A malformed escape is copied as is. Bytes that do not
/// form valid UTF-8 become U+FFFD.
pub fn unescape_unit(name: &str, out: &mut String) {
    out.clear();
    if !name.contains("\\x") {
        out.push_str(name);
        return;
    }
    let src = name.as_bytes();
    let mut bytes = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'\\'
            && src.get(i + 1) == Some(&b'x')
            && let (Some(hi), Some(lo)) = (
                src.get(i + 2).and_then(|&b| hex(b)),
                src.get(i + 3).and_then(|&b| hex(b)),
            )
        {
            bytes.push((hi << 4) | lo);
            i += 4;
        } else {
            bytes.push(src[i]);
            i += 1;
        }
    }
    out.push_str(&String::from_utf8_lossy(&bytes));
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Where a process sits relative to the systemd user slices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserPlacement {
    /// Under `user.slice/user-<uid>.slice/user@<uid>.service/`.
    InUserManager { uid: u32 },
    /// In `user.slice/user-<uid>.slice/session-<n>.scope`.
    InSessionScope { uid: u32 },
    /// Anywhere else: `system.slice`, `init.scope`, `machine.slice`, the root.
    OutsideUserSlice,
}

/// Classifies a cgroup path by its user slice.
pub fn placement(p: &CgroupPath) -> UserPlacement {
    let mut comps = p.rel.split('/');
    if comps.next() != Some("user.slice") {
        return UserPlacement::OutsideUserSlice;
    }
    let Some(uid) = comps.next().and_then(user_slice_uid) else {
        return UserPlacement::OutsideUserSlice;
    };
    match comps.next() {
        Some(c) if user_manager_uid(c) == Some(uid) => UserPlacement::InUserManager { uid },
        Some(c) if is_session_scope(c) => UserPlacement::InSessionScope { uid },
        _ => UserPlacement::OutsideUserSlice,
    }
}

/// Returns the app unit when the path is under
/// `user.slice/user-{uid}.slice/user@{uid}.service/app.slice/`: the first component after
/// `app.slice/`, which may itself be a `.slice`. Returns `None` otherwise.
pub fn app_slice_unit(p: &CgroupPath, uid: u32) -> Option<&str> {
    let mut comps = p.rel.split('/');
    if comps.next() != Some("user.slice")
        || comps.next().and_then(user_slice_uid) != Some(uid)
        || comps.next().and_then(user_manager_uid) != Some(uid)
        || comps.next() != Some("app.slice")
    {
        return None;
    }
    comps.next().filter(|c| !c.is_empty())
}

/// `user-<uid>.slice` to its uid.
fn user_slice_uid(c: &str) -> Option<u32> {
    parse_uid(c.strip_prefix("user-")?.strip_suffix(".slice")?)
}

/// `user@<uid>.service` to its uid.
fn user_manager_uid(c: &str) -> Option<u32> {
    parse_uid(c.strip_prefix("user@")?.strip_suffix(".service")?)
}

/// `session-<n>.scope`, where `<n>` is not empty.
fn is_session_scope(c: &str) -> bool {
    c.strip_prefix("session-")
        .and_then(|s| s.strip_suffix(".scope"))
        .is_some_and(|n| !n.is_empty())
}

fn parse_uid(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
