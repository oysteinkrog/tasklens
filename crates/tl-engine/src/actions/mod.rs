//! Action admission (ADR-006, E11). The reducer calls `admit` on `Command::Action` before any
//! action task starts.
//!
//! Module lines for `process`, `cgroup`, `unit`, `startup`, `dump`, `socket` and `recovery` are
//! added by their beads.

use rustc_hash::FxHashSet;
use tl_model::actions::{
    ActionKind, ActionRequest, ActionTarget, MAX_ACTION_TARGETS, MAX_ACTIONS_IN_FLIGHT,
    RejectReason,
};
use tl_model::ids::ProcessKey;
use tl_model::process::ProcFlags;
use tl_model::table::UnitRef;

use crate::reducer::AdmissionView;

/// Most requests running at once; one more is `Rejected(Busy)`.
pub const MAX_IN_FLIGHT: usize = MAX_ACTIONS_IN_FLIGHT;
/// Most targets in one request; one more is `Rejected(TooManyTargets)`.
pub const MAX_TARGETS: usize = MAX_ACTION_TARGETS;

/// The admission result for one request.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Admission {
    /// Refuse the whole request. Nothing starts.
    Reject(RejectReason),
    /// Start the request. `per_target[i]` is `Some` when target `i` is rejected before start.
    /// Empty for a targetless kind.
    Run {
        per_target: Vec<Option<RejectReason>>,
    },
}

/// True for `OpenUri`, `ShowInFileManager` and `Launch`: they carry no targets (R-134).
pub fn targetless(kind: &ActionKind) -> bool {
    kind.is_targetless()
}

/// Admission rules in ADR-006 order. A request whose every target is rejected still runs, so the
/// UI gets `Started`, the per-target `Rejected` events and a `Rejected` summary.
pub fn admit(
    req: &ActionRequest,
    view: &AdmissionView<'_>,
    in_flight: usize,
    busy_units: &FxHashSet<UnitRef>,
) -> Admission {
    if in_flight >= MAX_IN_FLIGHT {
        return Admission::Reject(RejectReason::Busy);
    }
    if req.targets.len() > MAX_TARGETS {
        return Admission::Reject(RejectReason::TooManyTargets);
    }
    if targetless(&req.kind) {
        if !req.targets.is_empty() {
            return Admission::Reject(RejectReason::Unsupported);
        }
        return Admission::Run {
            per_target: Vec::new(),
        };
    }
    if req.targets.is_empty() {
        return Admission::Reject(RejectReason::Unsupported);
    }
    let per_target = req
        .targets
        .iter()
        .map(|target| match target {
            ActionTarget::Process(key) | ActionTarget::Thread { process: key, .. } => {
                check_process(key, view)
            }
            ActionTarget::Unit(unit) => busy_units.contains(unit).then_some(RejectReason::Busy),
            // Validated by their own modules.
            ActionTarget::Cgroup(_) | ActionTarget::Startup(_) | ActionTarget::Socket { .. } => {
                None
            }
        })
        .collect();
    Admission::Run { per_target }
}

/// PROC-059: identity first, then kernel threads, then tasklens itself and PID 1. Other
/// `PROTECTED` targets were confirmed in the UI and pass. A key that exited within the linger
/// passes too: the pidfd open reports it.
fn check_process(key: &ProcessKey, view: &AdmissionView<'_>) -> Option<RejectReason> {
    if key.boot_id != view.boot_id {
        return Some(RejectReason::IdentityChanged);
    }
    let flags = match view.live.get(key) {
        Some(flags) => *flags,
        None if view.exited_within_linger.contains(key) => ProcFlags::empty(),
        None => return Some(RejectReason::IdentityChanged),
    };
    if flags.contains(ProcFlags::KERNEL_THREAD) {
        return Some(RejectReason::KernelThread);
    }
    if flags.contains(ProcFlags::IS_SELF) || key.pid == 1 {
        return Some(RejectReason::Protected);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustc_hash::FxHashMap;
    use smallvec::SmallVec;
    use tl_model::actions::{ActionId, CgroupRef, Signal};
    use tl_model::table::Manager;
    use tl_model::time::BootId;

    use super::*;

    const BOOT: BootId = BootId(1);

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 100 + u64::from(pid),
            boot_id: BOOT,
            pid,
        }
    }

    fn request(kind: ActionKind, targets: Vec<ActionTarget>) -> ActionRequest {
        ActionRequest {
            id: ActionId(1),
            kind,
            targets: SmallVec::from_vec(targets),
            privileged: false,
        }
    }

    fn term(targets: Vec<ActionTarget>) -> ActionRequest {
        request(ActionKind::Signal(Signal::Term), targets)
    }

    struct Fixture {
        live: FxHashMap<ProcessKey, ProcFlags>,
        exited: FxHashSet<ProcessKey>,
        busy: FxHashSet<UnitRef>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                live: FxHashMap::default(),
                exited: FxHashSet::default(),
                busy: FxHashSet::default(),
            }
        }

        fn admit(&self, req: &ActionRequest, in_flight: usize) -> Admission {
            let view = AdmissionView {
                boot_id: BOOT,
                live: &self.live,
                exited_within_linger: &self.exited,
            };
            admit(req, &view, in_flight, &self.busy)
        }

        fn per_target(&self, req: &ActionRequest) -> Vec<Option<RejectReason>> {
            match self.admit(req, 0) {
                Admission::Run { per_target } => per_target,
                other => panic!("expected Run, got {other:?}"),
            }
        }
    }

    fn unit(name: &str) -> UnitRef {
        UnitRef {
            manager: Manager::User,
            name: Arc::from(name),
        }
    }

    #[test]
    fn admission_limits() {
        let mut f = Fixture::new();
        f.live.insert(key(500), ProcFlags::empty());
        let one = term(vec![ActionTarget::Process(key(500))]);
        assert_eq!(f.admit(&one, 16), Admission::Reject(RejectReason::Busy));
        assert!(matches!(f.admit(&one, 15), Admission::Run { .. }));

        let many = |n: u32| {
            term(
                (0..n)
                    .map(|i| ActionTarget::Process(key(1000 + i)))
                    .collect(),
            )
        };
        assert_eq!(
            f.admit(&many(257), 0),
            Admission::Reject(RejectReason::TooManyTargets)
        );
        match f.admit(&many(256), 0) {
            Admission::Run { per_target } => assert_eq!(per_target.len(), 256),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn admission_identity_and_flags() {
        let mut f = Fixture::new();
        f.live.insert(key(500), ProcFlags::empty());
        f.live.insert(key(2), ProcFlags::KERNEL_THREAD);
        f.live.insert(key(600), ProcFlags::IS_SELF);
        f.live.insert(key(1), ProcFlags::PROTECTED);
        f.live.insert(key(700), ProcFlags::PROTECTED);
        f.exited.insert(key(800));
        let other_boot = ProcessKey {
            boot_id: BootId(2),
            ..key(500)
        };
        let req = term(vec![
            ActionTarget::Process(key(500)),
            ActionTarget::Process(other_boot),
            ActionTarget::Process(key(900)),
            ActionTarget::Process(key(800)),
            ActionTarget::Process(key(2)),
            ActionTarget::Process(key(600)),
            ActionTarget::Process(key(1)),
            ActionTarget::Process(key(700)),
            ActionTarget::Thread {
                process: other_boot,
                tid: 501,
            },
            ActionTarget::Thread {
                process: key(500),
                tid: 501,
            },
        ]);
        assert_eq!(
            f.per_target(&req),
            vec![
                None,
                Some(RejectReason::IdentityChanged),
                Some(RejectReason::IdentityChanged),
                None,
                Some(RejectReason::KernelThread),
                Some(RejectReason::Protected),
                Some(RejectReason::Protected),
                None,
                Some(RejectReason::IdentityChanged),
                None,
            ]
        );

        // Every target rejected still runs, so the UI gets per-target events.
        let all_bad = term(vec![ActionTarget::Process(key(900))]);
        assert_eq!(
            f.per_target(&all_bad),
            vec![Some(RejectReason::IdentityChanged)]
        );
    }

    #[test]
    fn admission_targetless_kinds() {
        let mut f = Fixture::new();
        f.live.insert(key(500), ProcFlags::empty());
        assert_eq!(
            f.admit(&term(vec![]), 0),
            Admission::Reject(RejectReason::Unsupported)
        );
        let open = |targets| {
            request(
                ActionKind::OpenUri(Arc::from("https://example.org")),
                targets,
            )
        };
        assert_eq!(
            f.admit(&open(vec![]), 0),
            Admission::Run {
                per_target: Vec::new()
            }
        );
        assert_eq!(
            f.admit(&open(vec![ActionTarget::Process(key(500))]), 0),
            Admission::Reject(RejectReason::Unsupported)
        );
        let show = request(
            ActionKind::ShowInFileManager {
                path: Arc::from("/home"),
                properties: false,
            },
            vec![],
        );
        assert!(targetless(&show.kind));
        assert!(!targetless(&ActionKind::KillTree));
        assert_eq!(
            f.admit(&show, 0),
            Admission::Run {
                per_target: Vec::new()
            }
        );
        // The in-flight limit applies to targetless kinds too.
        assert_eq!(
            f.admit(&open(vec![]), 16),
            Admission::Reject(RejectReason::Busy)
        );
    }

    #[test]
    fn admission_unit_busy() {
        let mut f = Fixture::new();
        f.busy.insert(unit("busy.service"));
        let req = request(
            ActionKind::UnitRestart,
            vec![
                ActionTarget::Unit(unit("busy.service")),
                ActionTarget::Unit(unit("idle.service")),
                ActionTarget::Cgroup(CgroupRef {
                    rel_path: Arc::from("user.slice/app.slice/app-foo.scope"),
                    cgroup_id: 42,
                }),
            ],
        );
        assert_eq!(
            f.per_target(&req),
            vec![Some(RejectReason::Busy), None, None]
        );
    }
}
