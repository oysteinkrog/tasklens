//! Action requests, targets, kinds, events and errors (ADR-005, ADR-006).
//!
//! The UI builds an `ActionRequest` with identities captured at menu-open time. The engine admits
//! it and reports ordered `ActionEvent`s. `tl-ui/src/action_tracker.rs::message(&ActionError)`
//! owns the user text, so nothing here implements `Display`.
//!
//! The serde derives carry these types over the engine protocol (ADR-018); serde's `rc` feature
//! sends `Arc` values by value. The helper protocol has its own types.
//!
//! Privacy: `OpenUri`, `Launch` command text and dump paths are user data. Never log them above
//! debug level (ADR-015).

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use smallvec::SmallVec;

use crate::ids::ProcessKey;
use crate::system::CpuSet;
use crate::table::{StartupRef, UnitRef};

/// Most targets one request may carry (ADR-005, ADR-006 admission).
pub const MAX_ACTION_TARGETS: usize = 256;
/// Most requests the engine runs at once; more are `Rejected(Busy)` (ADR-005, ADR-006).
pub const MAX_ACTIONS_IN_FLIGHT: usize = 16;

/// Allocated by the UI from an `AtomicU64` that starts at 1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActionId(pub u64);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionRequest {
    pub id: ActionId,
    pub kind: ActionKind,
    /// At most `MAX_ACTION_TARGETS`, captured at menu-open time. Empty for the targetless kinds
    /// (`ActionKind::is_targetless`). One target is stored inline, without allocation.
    pub targets: SmallVec<[ActionTarget; 1]>,
    /// The user chose "Retry as administrator" (P3).
    pub privileged: bool,
}

/// A cgroup captured with its inode so the engine can tell a recreated cgroup apart.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct CgroupRef {
    /// Relative to /sys/fs/cgroup.
    pub rel_path: Arc<str>,
    /// Inode of the cgroup directory when captured.
    pub cgroup_id: u64,
}

/// Socket address family: `AF_INET` or `AF_INET6`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Family {
    Inet,
    Inet6,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Proto {
    Tcp,
    Udp,
}

/// serde is hand-written: serde's own `SocketAddrV6` drops flowinfo and the scope id in binary
/// formats, and a link-local socket needs its scope id (R-270).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FourTuple {
    pub local: std::net::SocketAddr,
    pub remote: std::net::SocketAddr,
}

/// Wire form of one address: V4 is (ip, port), V6 is (ip, port, flowinfo, scope_id) (R-270).
#[derive(Serialize, Deserialize)]
enum WireAddr {
    V4(Ipv4Addr, u16),
    V6(Ipv6Addr, u16, u32, u32),
}

impl From<SocketAddr> for WireAddr {
    fn from(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(a) => WireAddr::V4(*a.ip(), a.port()),
            SocketAddr::V6(a) => WireAddr::V6(*a.ip(), a.port(), a.flowinfo(), a.scope_id()),
        }
    }
}

impl From<WireAddr> for SocketAddr {
    fn from(addr: WireAddr) -> Self {
        match addr {
            WireAddr::V4(ip, port) => SocketAddr::V4(SocketAddrV4::new(ip, port)),
            WireAddr::V6(ip, port, flowinfo, scope_id) => {
                SocketAddr::V6(SocketAddrV6::new(ip, port, flowinfo, scope_id))
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename = "FourTuple")]
struct WireFourTuple {
    local: WireAddr,
    remote: WireAddr,
}

impl Serialize for FourTuple {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        WireFourTuple {
            local: self.local.into(),
            remote: self.remote.into(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FourTuple {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireFourTuple::deserialize(deserializer)?;
        Ok(FourTuple {
            local: wire.local.into(),
            remote: wire.remote.into(),
        })
    }
}

/// What an action acts on. Every variant carries a full identity so the engine and the helper
/// can revalidate it. Never add a PID-only target.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum ActionTarget {
    Process(ProcessKey),
    /// P2, Threads tab. Allowed kinds: `Signal`, `SetNice`, `SetSched`, `SetAffinity`.
    Thread {
        process: ProcessKey,
        tid: u32,
    },
    /// App cgroups under app.slice.
    Cgroup(CgroupRef),
    /// Services, and freeze, thaw and kill of units outside app.slice.
    Unit(UnitRef),
    Startup(StartupRef),
    /// P3, for `SockDestroy`.
    Socket {
        cookie: u64,
        family: Family,
        proto: Proto,
        four_tuple: FourTuple,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Signal {
    Term,
    Kill,
    Hup,
    Int,
    Quit,
    Stop,
    Cont,
    Usr1,
    Usr2,
    /// Any other signal number, including SIGRTMIN..=SIGRTMAX.
    Other(u8),
}

/// glibc's first real-time signal on Linux. Kernel signals 32 and 33 are reserved by glibc.
pub const SIGRTMIN: i32 = 34;
/// The last real-time signal on Linux.
pub const SIGRTMAX: i32 = 64;

impl Signal {
    /// Linux signal number: Term 15, Kill 9, Hup 1, Int 2, Quit 3, Stop 19, Cont 18, Usr1 10,
    /// Usr2 12, Other(n) n.
    pub const fn number(self) -> i32 {
        match self {
            Signal::Term => 15,
            Signal::Kill => 9,
            Signal::Hup => 1,
            Signal::Int => 2,
            Signal::Quit => 3,
            Signal::Stop => 19,
            Signal::Cont => 18,
            Signal::Usr1 => 10,
            Signal::Usr2 => 12,
            Signal::Other(n) => n as i32,
        }
    }
}

/// `rt_prio` is 1..=99 for `Fifo` and `Rr`, else 0. `Deadline` (`SCHED_DEADLINE`) is display
/// only: tasklens never sets it, and every action that asks for it is refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum SchedPolicy {
    Other,
    Batch,
    Idle,
    Fifo,
    Rr,
    Deadline,
}

/// Levels are 0..=7. `None` is `IOPRIO_CLASS_NONE`, shown as "Default (follows nice)".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum IoPriority {
    None,
    Realtime(u8),
    BestEffort(u8),
    Idle,
}

/// The setting `processes.dump_type`: `"mini"` or `"full"`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DumpMode {
    #[default]
    Mini,
    Full,
}

/// P3: only the keys the user changed, plus the keys already in the tasklens drop-in (SYS-013).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct RecoveryEdit {
    /// `Restart=` word: no, on-success, on-failure, on-abnormal, on-watchdog, on-abort, always.
    pub restart: Option<Arc<str>>,
    /// `RestartSec=` in microseconds.
    pub restart_usec: Option<u64>,
    /// `RestartSteps=`.
    pub restart_steps: Option<u32>,
    /// `RestartMaxDelaySec=` in microseconds.
    pub restart_max_delay_usec: Option<u64>,
    /// `StartLimitBurst=`.
    pub start_limit_burst: Option<u32>,
    /// `StartLimitIntervalSec=` in microseconds.
    pub start_limit_interval_usec: Option<u64>,
    /// `StartLimitAction=` word.
    pub start_limit_action: Option<Arc<str>>,
    /// `FailureAction=` word.
    pub failure_action: Option<Arc<str>>,
    /// `OnFailure=` unit names.
    pub on_failure: Option<Arc<[Arc<str>]>>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum LaunchWhat {
    Command(Arc<str>),
    DesktopEntry(Arc<str>),
    Unit(UnitRef),
    /// org.freedesktop.Application.ActivateAction on `bus_name` at `object_path`, 2 s deadline,
    /// P3 (PERF-061, R-170 item 11). Used for GNOME network settings.
    DbusActivate {
        bus_name: Arc<str>,
        object_path: Arc<str>,
        action: Arc<str>,
        parameter: Option<Arc<str>>,
    },
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub what: LaunchWhat,
    pub as_admin: bool,
    pub cwd: Option<Arc<str>>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum ActionKind {
    /// End Task is `Signal(Term)`, End Process is `Signal(Kill)`.
    Signal(Signal),
    /// SIGSTOP every target, then SIGKILL every target.
    KillTree,
    /// SIGSTOP on one process.
    Suspend,
    /// SIGCONT on one process.
    Resume,
    /// -20..=19.
    SetNice(i8),
    /// P2.
    SetSched {
        policy: SchedPolicy,
        rt_prio: u8,
    },
    SetAffinity(CpuSet),
    SetIoPriority(IoPriority),
    FreezeGroup,
    ThawGroup,
    KillGroup,
    /// P2. `name` is a file name only, such as `firefox_1234_20261003_120000.core`. The engine
    /// puts it in its own dumps dir and reports the full path in `ActionState::Saved`: a path on
    /// a host is always chosen by that host's engine (ADR-018).
    Dump {
        mode: DumpMode,
        name: Arc<str>,
    },
    UnitStart,
    UnitStop,
    UnitRestart,
    UnitReload,
    UnitResetFailed,
    UnitEnable,
    UnitDisable,
    UnitMask,
    UnitUnmask,
    /// P3.
    UnitWriteRecovery(RecoveryEdit),
    /// P3: deletes the tasklens drop-in.
    UnitRemoveRecovery,
    StartupEnable,
    StartupDisable,
    StartupRunNow,
    StartupDelete,
    /// P3, target `ActionTarget::Socket`.
    SockDestroy,
    /// Portal OpenURI, else xdg-open.
    OpenUri(Arc<str>),
    /// FileManager1 ShowItems, or ShowItemProperties when `properties` is true.
    ShowInFileManager {
        path: Arc<str>,
        properties: bool,
    },
    /// Run dialog and Startup "Run now".
    Launch(LaunchSpec),
    /// Switch to window: KWin activates the window `internal_id` (`WindowEntry::internal_id`)
    /// on the host's screen. Unlike `KWinCommand::Activate` it ends with a result, so a window
    /// can say it switched or why not (ADR-018, leader decision on q4x76.12):
    /// `Failed(NoSuchProcess)` when the window is gone, `Failed(Unsupported)` when the host has
    /// no KWin bridge.
    ActivateWindow {
        internal_id: Arc<str>,
    },
}

impl ActionKind {
    /// True for OpenUri, ShowInFileManager, Launch and ActivateWindow: targets is empty and
    /// admission skips the target checks (R-134).
    pub fn is_targetless(&self) -> bool {
        matches!(
            self,
            ActionKind::OpenUri(_)
                | ActionKind::ShowInFileManager { .. }
                | ActionKind::Launch(_)
                | ActionKind::ActivateWindow { .. }
        )
    }
}

/// Why an action failed (ADR-015, R-148 item 3, R-170 item 9).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ActionError {
    PermissionDenied,
    NoSuchProcess,
    Partial {
        done: u16,
        total: u16,
    },
    /// A JobRemoved result other than "done", or the D-Bus error name when the method call itself
    /// failed (R-170 item 9).
    UnitJob {
        result: Arc<str>,
        error_name: Option<Arc<str>>,
    },
    /// pkexec 127, polkit NotAuthorized, systemd AccessDenied.
    NotAuthorized,
    /// pkexec 126, polkit NotAuthorizedDismissed.
    AuthCancelled,
    /// No FileManager1 owner, invalid command text.
    Unsupported,
    /// errno.
    Os(i32),
    Internal(Arc<str>),
}

/// Why admission refused a request. Nothing was started.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum RejectReason {
    /// `MAX_ACTIONS_IN_FLIGHT` requests are running.
    Busy,
    TooManyTargets,
    IdentityChanged,
    KernelThread,
    Protected,
    NotOwner,
    Unsupported,
    /// StartupRunNow or Launch of a program that is running. UI toast "Already running"
    /// (SYS-023, R-148 item 4).
    AlreadyRunning,
    /// A command that acts on the host's screen (open, launch, run now, switch to window)
    /// while nobody is logged in to a desktop there. UI text "home has no desktop session"
    /// (ADR-018).
    NoDesktopSession,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ActionState {
    /// Never started, nothing changed.
    Rejected(RejectReason),
    /// Admitted and running.
    Started,
    Succeeded,
    /// The OS or service reported failure; nothing changed.
    Failed(ActionError),
    /// Timed out, or the target vanished after the effect may have happened.
    OutcomeUnknown(Arc<str>),
    /// Not final: a running target has written `bytes` so far. Only `Dump` sends it, about every
    /// 500 ms, between `Started` and the target's final state (ADR-005, ADR-018).
    Progress {
        bytes: u64,
    },
    /// Succeeded and wrote the file at this full path on the engine's host. A `Dump` target ends
    /// with it, and so does the summary of a one-target dump.
    Saved(Arc<str>),
}

impl ActionState {
    /// Every state except `Started` and `Progress`.
    pub fn is_final(&self) -> bool {
        !matches!(self, ActionState::Started | ActionState::Progress { .. })
    }

    /// `Succeeded` or `Saved`.
    pub fn is_success(&self) -> bool {
        matches!(self, ActionState::Succeeded | ActionState::Saved(_))
    }

    /// Summary rule (ADR-005): Succeeded only if every target succeeded; otherwise the worst state
    /// in the order Failed, OutcomeUnknown, Rejected. Returns the first state of the worst class
    /// found. An empty iterator gives Succeeded: a targetless kind has no per-target events.
    ///
    /// A `Started` target ranks below Rejected, so the summary stays `Started` while a target is
    /// still running and nothing worse has happened. `Saved` counts as success; `Progress` is
    /// skipped.
    pub fn summarize<'a>(per_target: impl IntoIterator<Item = &'a ActionState>) -> ActionState {
        fn rank(state: &ActionState) -> u8 {
            match state {
                ActionState::Succeeded | ActionState::Saved(_) => 0,
                ActionState::Started | ActionState::Progress { .. } => 1,
                ActionState::Rejected(_) => 2,
                ActionState::OutcomeUnknown(_) => 3,
                ActionState::Failed(_) => 4,
            }
        }
        let mut worst: Option<&ActionState> = None;
        for state in per_target {
            if matches!(state, ActionState::Progress { .. }) {
                continue;
            }
            if worst.is_none_or(|w| rank(state) > rank(w)) {
                worst = Some(state);
            }
        }
        worst.cloned().unwrap_or(ActionState::Succeeded)
    }
}

/// One step of an action. Events for one `ActionId` arrive in order.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ActionEvent {
    pub id: ActionId,
    /// Index into the request's targets; `None` for the whole request.
    pub target: Option<u16>,
    pub state: ActionState,
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use serde::de::value::{Error as ValueError, StrDeserializer};

    use super::*;

    #[test]
    fn signal_numbers_linux() {
        let named = [
            Signal::Term,
            Signal::Kill,
            Signal::Hup,
            Signal::Int,
            Signal::Quit,
            Signal::Stop,
            Signal::Cont,
            Signal::Usr1,
            Signal::Usr2,
        ];
        let numbers: Vec<i32> = named.iter().map(|s| s.number()).collect();
        assert_eq!(numbers, [15, 9, 1, 2, 3, 19, 18, 10, 12]);
        assert_eq!(Signal::Other(34).number(), 34);
    }

    #[test]
    fn summarize_all_succeeded() {
        let states = [ActionState::Succeeded, ActionState::Succeeded];
        assert_eq!(ActionState::summarize(&states), ActionState::Succeeded);
    }

    #[test]
    fn summarize_failed_beats_unknown_and_rejected() {
        let states = [
            ActionState::Rejected(RejectReason::Protected),
            ActionState::Failed(ActionError::PermissionDenied),
            ActionState::OutcomeUnknown("x".into()),
        ];
        assert_eq!(
            ActionState::summarize(&states),
            ActionState::Failed(ActionError::PermissionDenied)
        );
    }

    #[test]
    fn summarize_unknown_beats_rejected() {
        let states = [
            ActionState::Succeeded,
            ActionState::Rejected(RejectReason::IdentityChanged),
            ActionState::OutcomeUnknown("x".into()),
        ];
        assert_eq!(
            ActionState::summarize(&states),
            ActionState::OutcomeUnknown("x".into())
        );
    }

    #[test]
    fn summarize_single_rejected() {
        let states = [ActionState::Rejected(RejectReason::KernelThread)];
        assert_eq!(
            ActionState::summarize(&states),
            ActionState::Rejected(RejectReason::KernelThread)
        );
    }

    #[test]
    fn summarize_empty_is_succeeded() {
        assert_eq!(ActionState::summarize(&[]), ActionState::Succeeded);
    }

    #[test]
    fn summarize_already_running() {
        let states = [
            ActionState::Succeeded,
            ActionState::Rejected(RejectReason::AlreadyRunning),
        ];
        assert_eq!(
            ActionState::summarize(&states),
            ActionState::Rejected(RejectReason::AlreadyRunning)
        );
    }

    #[test]
    fn summarize_auth_errors() {
        let states = [
            ActionState::Failed(ActionError::AuthCancelled),
            ActionState::Failed(ActionError::NotAuthorized),
        ];
        assert_eq!(
            ActionState::summarize(&states),
            ActionState::Failed(ActionError::AuthCancelled)
        );
    }

    #[test]
    fn targetless_kinds() {
        let launch = LaunchSpec {
            what: LaunchWhat::Command("true".into()),
            as_admin: false,
            cwd: None,
        };
        assert!(ActionKind::OpenUri("https://example.org".into()).is_targetless());
        assert!(
            ActionKind::ShowInFileManager {
                path: "/home".into(),
                properties: false,
            }
            .is_targetless()
        );
        assert!(ActionKind::Launch(launch).is_targetless());
        assert!(
            ActionKind::ActivateWindow {
                internal_id: "{0a1b2c3d-0000-4000-8000-00000000000a}".into(),
            }
            .is_targetless()
        );
        assert!(!ActionKind::Signal(Signal::Term).is_targetless());
        assert!(!ActionKind::UnitStart.is_targetless());
    }

    #[test]
    fn state_is_final() {
        assert!(!ActionState::Started.is_final());
        assert!(!ActionState::Progress { bytes: 7 }.is_final());
        for state in [
            ActionState::Rejected(RejectReason::Busy),
            ActionState::Succeeded,
            ActionState::Failed(ActionError::NoSuchProcess),
            ActionState::OutcomeUnknown("timeout".into()),
            ActionState::Saved("/s/dumps/x.core".into()),
        ] {
            assert!(state.is_final(), "{state:?}");
        }
    }

    #[test]
    fn state_is_success() {
        assert!(ActionState::Succeeded.is_success());
        assert!(ActionState::Saved("/s/dumps/x.core".into()).is_success());
        for state in [
            ActionState::Started,
            ActionState::Progress { bytes: 1 },
            ActionState::Rejected(RejectReason::Busy),
            ActionState::Failed(ActionError::NoSuchProcess),
            ActionState::OutcomeUnknown("timeout".into()),
        ] {
            assert!(!state.is_success(), "{state:?}");
        }
    }

    /// A one-target dump's summary keeps the path; progress never decides the summary.
    #[test]
    fn summarize_saved_and_progress() {
        let saved = ActionState::Saved("/s/dumps/x.core".into());
        assert_eq!(ActionState::summarize([&saved]), saved);
        let progress = ActionState::Progress { bytes: 9 };
        assert_eq!(ActionState::summarize([&progress]), ActionState::Succeeded);
        assert_eq!(ActionState::summarize([&progress, &saved]), saved);
        let failed = ActionState::Failed(ActionError::Os(5));
        assert_eq!(ActionState::summarize([&saved, &failed]), failed);
    }

    /// Formats a value through serde's `fmt::Formatter` serializer, which writes a unit variant
    /// as its serde name.
    struct SerdeName<T>(T);

    impl<T: Serialize> fmt::Display for SerdeName<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.serialize(f)
        }
    }

    #[test]
    fn dump_mode_serde_lowercase() {
        assert_eq!(DumpMode::default(), DumpMode::Mini);
        for (mode, name) in [(DumpMode::Mini, "mini"), (DumpMode::Full, "full")] {
            assert_eq!(SerdeName(mode).to_string(), name);
            let back = DumpMode::deserialize(StrDeserializer::<ValueError>::new(name));
            assert_eq!(back.ok(), Some(mode));
        }
        let upper = DumpMode::deserialize(StrDeserializer::<ValueError>::new("Full"));
        assert!(upper.is_err());
    }
}
