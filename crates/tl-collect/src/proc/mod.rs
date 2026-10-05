//! Per-process readers for /proc/<pid>.
pub mod cgroup;
pub mod detail;
pub mod extras_p2;
pub mod fdcount;
pub mod handles;
pub mod io;
pub mod meta;
pub mod pid_stat;
pub mod smaps;
pub mod statm;
pub mod status;
pub mod strings;
pub mod sweep;

use rustix::io::Errno;

/// True when the process went away during the read (ENOENT or ESRCH).
#[allow(dead_code, reason = "used by the E05 readers")]
pub(crate) fn is_gone(e: Errno) -> bool {
    matches!(e, Errno::NOENT | Errno::SRCH)
}

/// True when the read was not allowed (EACCES or EPERM). The field is Restricted, not zero.
#[allow(dead_code, reason = "used by the E05 readers")]
pub(crate) fn is_denied(e: Errno) -> bool {
    matches!(e, Errno::ACCESS | Errno::PERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errno_classes() {
        assert!(is_gone(Errno::NOENT));
        assert!(is_gone(Errno::SRCH));
        assert!(!is_gone(Errno::ACCESS));
        assert!(!is_gone(Errno::PERM));
        assert!(!is_gone(Errno::IO));
        assert!(is_denied(Errno::ACCESS));
        assert!(is_denied(Errno::PERM));
        assert!(!is_denied(Errno::NOENT));
        assert!(!is_denied(Errno::SRCH));
        assert!(!is_denied(Errno::IO));
    }
}
