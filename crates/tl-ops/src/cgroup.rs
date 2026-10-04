//! Validated cgroup v2 writes.
//!
//! Freeze, thaw and kill act on an app cgroup under the user's `app.slice`. Before any write,
//! `open_validated` proves the captured `CgroupRef` still names the same cgroup, that the current
//! user owns it, that it is an app scope or service, and that tasklens itself is not inside it
//! (ADR-006). Units outside `app.slice` go through systemd, not here (R-092).

use std::os::fd::{BorrowedFd, OwnedFd};
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use tl_model::actions::CgroupRef;

use crate::error::{OpsError, OpsResult};

pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// How often `freeze_group` reads `cgroup.events` while it waits for `frozen 1`.
pub const FREEZE_POLL: Duration = Duration::from_millis(50);
/// How long `freeze_group` waits for `frozen 1` before it reports the outcome as unknown.
pub const FREEZE_WAIT: Duration = Duration::from_secs(2);

/// An `O_DIRECTORY` fd of a validated cgroup.
#[derive(Debug)]
pub struct CgroupDir {
    fd: OwnedFd,
}

impl CgroupDir {
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.fd.as_fd()
    }
}

/// Validates `cg` against the real cgroup root and the tasklens process's own cgroup.
pub fn open_validated(cg: &CgroupRef, uid: u32) -> OpsResult<CgroupDir> {
    open_validated_at(Path::new(CGROUP_ROOT), cg, uid, &own_cgroup()?)
}

/// Validates `cg` under `root`. `own_cgroup` is the tasklens cgroup path relative to the root.
pub fn open_validated_at(
    root: &Path,
    cg: &CgroupRef,
    uid: u32,
    own_cgroup: &str,
) -> OpsResult<CgroupDir> {
    let result = open_validated_inner(root, cg, uid, own_cgroup);
    let leaf = cg.rel_path.rsplit('/').next().unwrap_or("");
    tracing::debug!(cgroup = leaf, ok = result.is_ok(), error = ?result.as_ref().err(), "cgroup validation");
    result
}

fn open_validated_inner(
    root: &Path,
    cg: &CgroupRef,
    uid: u32,
    own_cgroup: &str,
) -> OpsResult<CgroupDir> {
    let rel = &*cg.rel_path;
    check_rel_path(rel, uid)?;

    let dir_flags = OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::RDONLY;
    let root_fd = rustix::fs::open(root, dir_flags, Mode::empty())?;
    let fd = match rustix::fs::openat(&root_fd, rel, dir_flags | OFlags::NOFOLLOW, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Err(OpsError::IdentityChanged),
        Err(e) => return Err(e.into()),
    };

    let st = rustix::fs::fstat(&fd)?;
    #[allow(clippy::unnecessary_cast)] // `st_ino` is `c_ulong` on some targets.
    if st.st_ino as u64 != cg.cgroup_id {
        return Err(OpsError::IdentityChanged);
    }
    if st.st_uid != uid {
        return Err(OpsError::NotOwner);
    }

    if is_inside(own_cgroup, rel) {
        return Err(OpsError::Protected);
    }

    Ok(CgroupDir { fd })
}

/// True when `path` equals `ancestor` or lies below it.
fn is_inside(path: &str, ancestor: &str) -> bool {
    match path.strip_prefix(ancestor) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

/// The tasklens process's own cgroup, relative to the cgroup root (empty for the root cgroup).
pub fn own_cgroup() -> OpsResult<String> {
    let text = std::fs::read_to_string("/proc/self/cgroup")?;
    parse_own_cgroup(&text)
}

fn parse_own_cgroup(text: &str) -> OpsResult<String> {
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or(OpsError::Unsupported("cgroup v2 not mounted"))?;
    Ok(line.strip_prefix('/').unwrap_or(line).to_owned())
}

/// Checks that `rel` is an app scope or service under
/// `user.slice/user-{uid}.slice/user@{uid}.service/app.slice/`. Pure, no I/O.
pub(crate) fn check_rel_path(rel: &str, uid: u32) -> OpsResult {
    if rel.is_empty() || rel.contains('\0') {
        return Err(OpsError::NotOwner);
    }
    // Splitting on `/` gives an empty component for a leading or trailing `/` and for `//`.
    if rel
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err(OpsError::NotOwner);
    }

    let mut parts = rel.split('/');
    let prefix_ok = parts.next() == Some("user.slice")
        && parts.next().is_some_and(|c| {
            c.strip_prefix("user-")
                .and_then(|c| c.strip_suffix(".slice"))
                .is_some_and(|n| is_uid(n, uid))
        })
        && parts.next().is_some_and(|c| {
            c.strip_prefix("user@")
                .and_then(|c| c.strip_suffix(".service"))
                .is_some_and(|n| is_uid(n, uid))
        })
        && parts.next() == Some("app.slice");
    if !prefix_ok {
        return Err(OpsError::NotOwner);
    }

    match parts.next_back() {
        Some(last) if last.ends_with(".scope") || last.ends_with(".service") => Ok(()),
        _ => Err(OpsError::NotOwner),
    }
}

/// True when `s` is `uid` in canonical decimal: digits only, no sign, no leading zero.
fn is_uid(s: &str, uid: u32) -> bool {
    let canonical = !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s.len() == 1 || !s.starts_with('0'));
    canonical && s.parse::<u32>() == Ok(uid)
}

/// Freezes every task in the cgroup: writes `1` to `cgroup.freeze`. Does not wait.
pub fn freeze(dir: &CgroupDir) -> OpsResult {
    write_control(dir, "cgroup.freeze", b"1", "cgroup.freeze not available")
}

/// Thaws the cgroup: writes `0` to `cgroup.freeze`.
pub fn thaw(dir: &CgroupDir) -> OpsResult {
    write_control(dir, "cgroup.freeze", b"0", "cgroup.freeze not available")
}

/// Kills every task in the cgroup: writes `1` to `cgroup.kill`.
pub fn kill(dir: &CgroupDir) -> OpsResult {
    write_control(dir, "cgroup.kill", b"1", "cgroup.kill not available")
}

/// One write of `value` to the control file `name`. `ENOENT` gives `Unsupported(missing)`.
fn write_control(dir: &CgroupDir, name: &str, value: &[u8], missing: &'static str) -> OpsResult {
    let fd = match rustix::fs::openat(
        dir.as_fd(),
        name,
        OFlags::WRONLY | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Err(OpsError::Unsupported(missing)),
        Err(e) => return Err(e.into()),
    };
    let n = rustix::io::write(&fd, value)?;
    if n != value.len() {
        return Err(OpsError::Os(libc::EIO));
    }
    Ok(())
}

/// Reads `frozen` from `cgroup.events`. A missing or malformed line gives `Os(EIO)`.
pub fn is_frozen(dir: &CgroupDir) -> OpsResult<bool> {
    let fd = rustix::fs::openat(
        dir.as_fd(),
        "cgroup.events",
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut buf = [0u8; 256];
    let n = rustix::io::read(&fd, &mut buf)?;
    parse_frozen(&buf[..n]).ok_or(OpsError::Os(libc::EIO))
}

/// Waits until `cgroup.events` says `frozen 1`. Checks at once, then every `poll` until `wait`
/// has passed. Sleeps the calling thread, so it must run on the blocking pool.
pub fn wait_frozen(dir: &CgroupDir, wait: Duration, poll: Duration) -> OpsResult {
    let start = Instant::now();
    loop {
        if is_frozen(dir)? {
            return Ok(());
        }
        let elapsed = start.elapsed();
        if elapsed >= wait {
            return Err(OpsError::OutcomeUnknown("freeze still in progress"));
        }
        std::thread::sleep(poll.min(wait - elapsed));
    }
}

/// The value of the `frozen` line in `cgroup.events` text: `Some(true)` for `1`, `Some(false)`
/// for `0`, `None` when the line is missing or malformed.
pub(crate) fn parse_frozen(events: &[u8]) -> Option<bool> {
    let value = events
        .split(|&b| b == b'\n')
        .find_map(|line| line.strip_prefix(b"frozen "))?;
    match value {
        b"1" => Some(true),
        b"0" => Some(false),
        _ => None,
    }
}

/// Validates `cg`, freezes it and waits up to `FREEZE_WAIT` for every task to stop.
pub fn freeze_group(cg: &CgroupRef, uid: u32) -> OpsResult {
    let result = open_validated(cg, uid).and_then(|dir| {
        freeze(&dir)?;
        wait_frozen(&dir, FREEZE_WAIT, FREEZE_POLL)
    });
    log_result("freeze", cg, &result);
    result
}

/// Validates `cg` and thaws it.
pub fn thaw_group(cg: &CgroupRef, uid: u32) -> OpsResult {
    let result = open_validated(cg, uid).and_then(|dir| thaw(&dir));
    log_result("thaw", cg, &result);
    result
}

/// Validates `cg` and kills every task in it.
pub fn kill_group(cg: &CgroupRef, uid: u32) -> OpsResult {
    let result = open_validated(cg, uid).and_then(|dir| kill(&dir));
    log_result("kill", cg, &result);
    result
}

/// Logs only the cgroup's last component, never the full path.
fn log_result(op: &str, cg: &CgroupRef, result: &OpsResult) {
    let leaf = cg.rel_path.rsplit('/').next().unwrap_or("");
    tracing::debug!(op, cgroup = leaf, ok = result.is_ok(), error = ?result.as_ref().err(), "cgroup op");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    fn prefix(uid: u32) -> String {
        format!("user.slice/user-{uid}.slice/user@{uid}.service/app.slice/")
    }

    #[test]
    fn rel_path_valid_app_service() {
        let u = uid();
        let rel = format!("{}app-org.kde.konsole@1a2b.service", prefix(u));
        assert_eq!(check_rel_path(&rel, u), Ok(()));
    }

    #[test]
    fn rel_path_valid_app_scope() {
        let u = uid();
        let rel = format!("{}app-test@1.scope", prefix(u));
        assert_eq!(check_rel_path(&rel, u), Ok(()));
        let nested = format!("{}app-flatpak.slice/app-test@1.scope", prefix(u));
        assert_eq!(check_rel_path(&nested, u), Ok(()));
    }

    #[test]
    fn rel_path_rejects_session_scope() {
        let u = uid();
        let rel = format!("user.slice/user-{u}.slice/session-2.scope");
        assert_eq!(check_rel_path(&rel, u), Err(OpsError::NotOwner));
    }

    #[test]
    fn rel_path_rejects_dotdot() {
        let u = uid();
        let rel = format!("{}../../../system.slice/sshd.service", prefix(u));
        assert_eq!(check_rel_path(&rel, u), Err(OpsError::NotOwner));
        let dot = format!("{}./app-test@1.scope", prefix(u));
        assert_eq!(check_rel_path(&dot, u), Err(OpsError::NotOwner));
    }

    #[test]
    fn rel_path_rejects_other_uid_slice() {
        let u = uid();
        let other = u.wrapping_add(1);
        let rel = format!("{}app-test@1.scope", prefix(other));
        assert_eq!(check_rel_path(&rel, u), Err(OpsError::NotOwner));
        let padded =
            format!("user.slice/user-0{u}.slice/user@0{u}.service/app.slice/app-test@1.scope");
        assert_eq!(check_rel_path(&padded, u), Err(OpsError::NotOwner));
        let mixed =
            format!("user.slice/user-{u}.slice/user@{other}.service/app.slice/app-test@1.scope");
        assert_eq!(check_rel_path(&mixed, u), Err(OpsError::NotOwner));
    }

    #[test]
    fn rel_path_rejects_bad_suffix() {
        let u = uid();
        let rel = format!("{}foo.slice", prefix(u));
        assert_eq!(check_rel_path(&rel, u), Err(OpsError::NotOwner));
        // The prefix alone has no component after `app.slice`.
        let bare = prefix(u);
        assert_eq!(
            check_rel_path(bare.trim_end_matches('/'), u),
            Err(OpsError::NotOwner)
        );
    }

    #[test]
    fn rel_path_rejects_malformed() {
        let u = uid();
        let valid = format!("{}app-test@1.scope", prefix(u));
        assert_eq!(check_rel_path(&valid, u), Ok(()));

        let leading = format!("/{valid}");
        let trailing = format!("{valid}/");
        let double = valid.replacen("app.slice/", "app.slice//", 1);
        let nul = valid.replacen("app-test", "app-\0test", 1);
        for bad in [leading.as_str(), &trailing, &double, &nul, ""] {
            assert_eq!(check_rel_path(bad, u), Err(OpsError::NotOwner), "{bad:?}");
        }
    }

    #[test]
    fn parse_frozen_values() {
        assert_eq!(parse_frozen(b"populated 1\nfrozen 1\n"), Some(true));
        assert_eq!(parse_frozen(b"populated 1\nfrozen 0\n"), Some(false));
        assert_eq!(parse_frozen(b"populated 1\n"), None);
        assert_eq!(parse_frozen(b"frozen 1"), Some(true));
        assert_eq!(parse_frozen(b"frozen 2\n"), None);
        assert_eq!(parse_frozen(b"frozen \n"), None);
        assert_eq!(parse_frozen(b"unfrozen 1\n"), None);
        assert_eq!(parse_frozen(b""), None);
    }

    #[test]
    fn own_cgroup_parses_v2_line() {
        let text = "0::/user.slice/user-1000.slice/session-2.scope\n";
        assert_eq!(
            parse_own_cgroup(text),
            Ok("user.slice/user-1000.slice/session-2.scope".to_owned())
        );
        assert_eq!(parse_own_cgroup("0::/\n"), Ok(String::new()));
        assert_eq!(
            parse_own_cgroup("1:name=systemd:/x\n"),
            Err(OpsError::Unsupported("cgroup v2 not mounted"))
        );
    }

    #[test]
    fn inside_matches_whole_components_only() {
        assert!(is_inside("a/b.scope", "a/b.scope"));
        assert!(is_inside("a/b.scope/sub", "a/b.scope"));
        assert!(!is_inside("a/b.scope2", "a/b.scope"));
        assert!(!is_inside("a", "a/b.scope"));
    }
}
