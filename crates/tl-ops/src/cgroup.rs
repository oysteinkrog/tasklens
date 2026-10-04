//! Validated cgroup v2 writes.
//!
//! Freeze, thaw and kill act on an app cgroup under the user's `app.slice`. Before any write,
//! `open_validated` proves the captured `CgroupRef` still names the same cgroup, that the current
//! user owns it, that it is an app scope or service, and that tasklens itself is not inside it
//! (ADR-006). Units outside `app.slice` go through systemd, not here (R-092).

use std::os::fd::{BorrowedFd, OwnedFd};
use std::path::Path;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use tl_model::actions::CgroupRef;

use crate::error::{OpsError, OpsResult};

pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

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

    match parts.last() {
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
