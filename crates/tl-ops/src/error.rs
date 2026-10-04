//! The error every operation returns, and its mapping to `ActionState` (ADR-015).
//!
//! User text comes from `ActionError` in the UI, never from this type's `Display`. No variant
//! carries a path, command line or process name.

use std::sync::Arc;

use tl_model::actions::{ActionError, ActionState, RejectReason};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpsError {
    #[error("process identity changed")]
    IdentityChanged,
    #[error("target is not owned by the current user")]
    NotOwner,
    #[error("target is protected")]
    Protected,
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("permission denied")]
    PermissionDenied,
    #[error("no such process")]
    NoSuchProcess,
    #[error("partially applied: {done} of {total}")]
    Partial { done: u16, total: u16 },
    #[error("outcome unknown: {0}")]
    OutcomeUnknown(&'static str),
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    #[error("OS error {0}")]
    Os(i32),
}

pub type OpsResult<T = ()> = Result<T, OpsError>;

impl OpsError {
    /// EPERM and EACCES give `PermissionDenied`, ESRCH gives `NoSuchProcess`, any other errno
    /// gives `Os`.
    fn from_errno(errno: i32) -> Self {
        match errno {
            libc::EPERM | libc::EACCES => OpsError::PermissionDenied,
            libc::ESRCH => OpsError::NoSuchProcess,
            e => OpsError::Os(e),
        }
    }

    pub fn to_action_state(&self) -> ActionState {
        match self {
            OpsError::IdentityChanged => ActionState::Rejected(RejectReason::IdentityChanged),
            OpsError::NotOwner => ActionState::Rejected(RejectReason::NotOwner),
            OpsError::Protected => ActionState::Rejected(RejectReason::Protected),
            OpsError::Unsupported(_) => ActionState::Rejected(RejectReason::Unsupported),
            OpsError::PermissionDenied => ActionState::Failed(ActionError::PermissionDenied),
            OpsError::NoSuchProcess => ActionState::Failed(ActionError::NoSuchProcess),
            OpsError::Partial { done, total } => ActionState::Failed(ActionError::Partial {
                done: *done,
                total: *total,
            }),
            OpsError::OutcomeUnknown(s) => ActionState::OutcomeUnknown(Arc::from(*s)),
            OpsError::InvalidInput(s) => ActionState::Failed(ActionError::Internal(Arc::from(*s))),
            OpsError::Os(e) => ActionState::Failed(ActionError::Os(*e)),
        }
    }
}

impl From<rustix::io::Errno> for OpsError {
    fn from(errno: rustix::io::Errno) -> Self {
        OpsError::from_errno(errno.raw_os_error())
    }
}

impl From<std::io::Error> for OpsError {
    /// An error without a raw OS error gives `Os(EIO)`.
    fn from(err: std::io::Error) -> Self {
        match err.raw_os_error() {
            Some(errno) => OpsError::from_errno(errno),
            None => OpsError::Os(libc::EIO),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use rustix::io::Errno;

    use super::*;

    #[test]
    fn errno_perm_and_acces_map_to_permission_denied() {
        assert_eq!(OpsError::from(Errno::PERM), OpsError::PermissionDenied);
        assert_eq!(OpsError::from(Errno::ACCESS), OpsError::PermissionDenied);
        let io = std::io::Error::from_raw_os_error(libc::EACCES);
        assert_eq!(OpsError::from(io), OpsError::PermissionDenied);
    }

    #[test]
    fn errno_srch_maps_to_no_such_process() {
        assert_eq!(OpsError::from(Errno::SRCH), OpsError::NoSuchProcess);
        let io = std::io::Error::from_raw_os_error(libc::ESRCH);
        assert_eq!(OpsError::from(io), OpsError::NoSuchProcess);
    }

    #[test]
    fn other_errno_maps_to_os() {
        assert_eq!(OpsError::from(Errno::INVAL), OpsError::Os(libc::EINVAL));
        let io = std::io::Error::from_raw_os_error(libc::EINVAL);
        assert_eq!(OpsError::from(io), OpsError::Os(libc::EINVAL));
    }

    #[test]
    fn io_error_without_errno_maps_to_eio() {
        // The same value as `Error::new(ErrorKind::Other, "x")`.
        let io = std::io::Error::other("x");
        assert_eq!(io.kind(), ErrorKind::Other);
        assert_eq!(OpsError::from(io), OpsError::Os(libc::EIO));
    }

    #[test]
    fn action_state_mapping_covers_every_variant() {
        assert_eq!(
            OpsError::IdentityChanged.to_action_state(),
            ActionState::Rejected(RejectReason::IdentityChanged)
        );
        assert_eq!(
            OpsError::NotOwner.to_action_state(),
            ActionState::Rejected(RejectReason::NotOwner)
        );
        assert_eq!(
            OpsError::Protected.to_action_state(),
            ActionState::Rejected(RejectReason::Protected)
        );
        assert_eq!(
            OpsError::Unsupported("x").to_action_state(),
            ActionState::Rejected(RejectReason::Unsupported)
        );
        assert_eq!(
            OpsError::PermissionDenied.to_action_state(),
            ActionState::Failed(ActionError::PermissionDenied)
        );
        assert_eq!(
            OpsError::NoSuchProcess.to_action_state(),
            ActionState::Failed(ActionError::NoSuchProcess)
        );
        assert_eq!(
            OpsError::Partial { done: 2, total: 5 }.to_action_state(),
            ActionState::Failed(ActionError::Partial { done: 2, total: 5 })
        );
        assert_eq!(
            OpsError::OutcomeUnknown("timeout").to_action_state(),
            ActionState::OutcomeUnknown(Arc::from("timeout"))
        );
        assert_eq!(
            OpsError::InvalidInput("bad").to_action_state(),
            ActionState::Failed(ActionError::Internal(Arc::from("bad")))
        );
        assert_eq!(
            OpsError::Os(libc::EINVAL).to_action_state(),
            ActionState::Failed(ActionError::Os(libc::EINVAL))
        );
    }
}
