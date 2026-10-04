//! The error type for every sampler and parser in this crate (ADR-015).

use rustix::io::Errno;
use tl_model::validity::Validity;

/// A failed read or parse. Messages never contain paths or process data.
#[derive(Debug, thiserror::Error)]
pub enum CollectError {
    /// EACCES or EPERM.
    #[error("permission denied")]
    PermissionDenied,
    /// ENOENT, ESRCH, ENODEV or ENXIO.
    #[error("not found")]
    NotFound,
    /// Any other errno.
    #[error("os error {0}")]
    Os(Errno),
    #[error("cannot parse {file}: {what}")]
    Parse {
        file: &'static str,
        what: &'static str,
    },
    /// ENOENT or ESRCH on a `/proc/<pid>` file during a sweep.
    #[error("process {pid} is gone")]
    ProcessGone { pid: u32 },
    /// `file` is a static name such as "stat", never a path.
    #[error("could not parse {file} of process {pid}")]
    ProcParse { pid: u32, file: &'static str },
    #[error("could not read {file}: errno {errno}")]
    ProcRead { file: &'static str, errno: i32 },
    #[error("not an ELF file")]
    NotElf,
    #[error("scan cancelled")]
    Cancelled,
    /// An `NLMSG_ERROR` reply. The field is the positive errno.
    #[error("sock_diag request failed: errno {0}")]
    SockDiagErrno(i32),
    #[error("malformed netlink message: {0}")]
    NetlinkMalformed(&'static str),
    /// A helper program such as `pacman --version` did not run or timed out.
    #[error("child command failed: {0}")]
    Command(&'static str),
}

impl From<Errno> for CollectError {
    fn from(errno: Errno) -> Self {
        match errno {
            Errno::ACCESS | Errno::PERM => Self::PermissionDenied,
            Errno::NOENT | Errno::SRCH | Errno::NODEV | Errno::NXIO => Self::NotFound,
            other => Self::Os(other),
        }
    }
}

impl From<std::io::Error> for CollectError {
    fn from(err: std::io::Error) -> Self {
        match err.raw_os_error() {
            Some(n) => Errno::from_raw_os_error(n).into(),
            None => Self::Os(Errno::IO),
        }
    }
}

impl CollectError {
    /// The Validity a field gets when reading it failed with this error (ADR-016).
    pub fn validity(&self) -> Validity {
        match self {
            Self::PermissionDenied => Validity::Restricted,
            Self::NotFound | Self::ProcessGone { .. } => Validity::Missing,
            Self::Os(_)
            | Self::Parse { .. }
            | Self::ProcParse { .. }
            | Self::ProcRead { .. }
            | Self::NotElf
            | Self::Cancelled
            | Self::SockDiagErrno(_)
            | Self::NetlinkMalformed(_)
            | Self::Command(_) => Validity::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errno_mapping() {
        assert!(matches!(
            CollectError::from(Errno::ACCESS),
            CollectError::PermissionDenied
        ));
        assert!(matches!(
            CollectError::from(Errno::PERM),
            CollectError::PermissionDenied
        ));
        for errno in [Errno::NOENT, Errno::SRCH, Errno::NODEV, Errno::NXIO] {
            assert!(matches!(CollectError::from(errno), CollectError::NotFound));
        }
        assert!(matches!(
            CollectError::from(Errno::IO),
            CollectError::Os(Errno::IO)
        ));
    }

    #[test]
    fn validity_mapping() {
        assert_eq!(
            CollectError::from(Errno::ACCESS).validity(),
            Validity::Restricted
        );
        assert_eq!(
            CollectError::PermissionDenied.validity(),
            Validity::Restricted
        );
        assert_eq!(CollectError::NotFound.validity(), Validity::Missing);
        for errno in [Errno::NOENT, Errno::SRCH, Errno::NODEV, Errno::NXIO] {
            assert_eq!(CollectError::from(errno).validity(), Validity::Missing);
        }
        assert_eq!(CollectError::Os(Errno::IO).validity(), Validity::Failed);
        let parse = CollectError::Parse {
            file: "/proc/stat",
            what: "cpu line",
        };
        assert_eq!(parse.validity(), Validity::Failed);
    }

    #[test]
    fn proc_parse_display() {
        let err = CollectError::ProcParse {
            pid: 7,
            file: "stat",
        };
        assert_eq!(err.to_string(), "could not parse stat of process 7");
    }

    #[test]
    fn sock_diag_errno_message() {
        assert_eq!(
            CollectError::SockDiagErrno(13).to_string(),
            "sock_diag request failed: errno 13"
        );
    }

    #[test]
    fn std_io_mapping() {
        let denied = std::io::Error::from_raw_os_error(13);
        assert!(matches!(
            CollectError::from(denied),
            CollectError::PermissionDenied
        ));
        let other = std::io::Error::other("x");
        assert!(matches!(
            CollectError::from(other),
            CollectError::Os(Errno::IO)
        ));
    }
}
