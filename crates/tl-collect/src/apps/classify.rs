//! Classify processes as app, background or system.

use std::sync::Arc;

use tl_model::ids::GroupKey;
use tl_model::process::GroupKind;

use crate::proc::cgroup::{CgroupPath, UserPlacement, placement};

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
    // under the unit's app id. That id comes from the unit name parser (E05-011,
    // tasklens-yms.14), which does not exist yet, and `owns_window` is false in P1. Until then
    // such a process stays Background, the rule's fallback for a unit that does not parse.
    (ProcClass::Background, GroupKind::Background)
}

/// The stable hash of a group (ADR-003, ADR-012). Same value as `GroupKind::key()`, which
/// defines the scheme (R-279).
pub fn group_key(kind: &GroupKind) -> GroupKey {
    kind.key()
}
