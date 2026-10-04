//! Privileged helper requests and replies (ADR-006, R-004, R-157).
//!
//! The engine and the root helper `tl-helper` talk with these types over `wire.rs` frames of at
//! most `MAX_HELPER_FRAME` bytes on the helper's socketpair stdin and stdout. Replies are read
//! with `wire::read_reply(.., MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL, |r: &HelperReply| r.last)`.
//!
//! `HelperRequest` is the security boundary: no path, command or free-form file argument may
//! appear in it. A new variant breaks `helper_request_variant_count` and needs a security review.
//! The helper re-runs `open_verified` on every process target and never trusts the engine.
//!
//! Privacy: environment and memory contents travel in `File` and `Strings` replies. Never log
//! reply bodies.

use serde::{Deserialize, Serialize};

use crate::actions::{DumpMode, Family, FourTuple, IoPriority, Proto, SchedPolicy, Signal};
use crate::details::{StringEncoding, StringScanOpts};
use crate::ids::ProcessKey;
use crate::system::CpuSet;
use crate::table::UnitType;

/// Protocol version sent in both `Hello` messages.
pub const HELPER_PROTO_VERSION: u16 = 1;

/// Longest unit name `validate_drop_in` accepts, in bytes.
const MAX_UNIT_NAME_LEN: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperTarget {
    Process(ProcessKey),
    Thread { process: ProcessKey, tid: u32 },
}

bitflags::bitflags! {
    /// The files `ReadProcessFiles` may read under /proc/<pid> (R-004, R-098). Bit values are
    /// frozen.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
    pub struct ProcFiles: u16 {
        const IO = 1 << 0;
        const FD_LIST = 1 << 1;
        const FDINFO = 1 << 2;
        const ENVIRON = 1 << 3;
        const MAPS = 1 << 4;
        const SMAPS = 1 << 5;
        const SMAPS_ROLLUP = 1 << 6;
        const NS = 1 << 7;
        const CWD = 1 << 8;
        const EXE = 1 << 9;
        const STATUS = 1 << 10;
        /// /proc/<pid>/root for PROC-078 and PROC-096 (R-157).
        const ROOT = 1 << 11;
    }
}

/// The only keys a recovery drop-in may set: Restart*, StartLimit*, FailureAction, OnFailure
/// (SYS-013).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DropInKey {
    Restart,
    RestartSec,
    RestartSteps,
    RestartMaxDelaySec,
    StartLimitBurst,
    StartLimitIntervalSec,
    StartLimitAction,
    FailureAction,
    OnFailure,
}

impl DropInKey {
    /// The systemd key name.
    pub const fn as_str(self) -> &'static str {
        match self {
            DropInKey::Restart => "Restart",
            DropInKey::RestartSec => "RestartSec",
            DropInKey::RestartSteps => "RestartSteps",
            DropInKey::RestartMaxDelaySec => "RestartMaxDelaySec",
            DropInKey::StartLimitBurst => "StartLimitBurst",
            DropInKey::StartLimitIntervalSec => "StartLimitIntervalSec",
            DropInKey::StartLimitAction => "StartLimitAction",
            DropInKey::FailureAction => "FailureAction",
            DropInKey::OnFailure => "OnFailure",
        }
    }
}

/// A cgroup of any uid under /sys/fs/cgroup, with the inode of its directory when captured.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperCgroup {
    /// Relative to /sys/fs/cgroup.
    pub rel_path: String,
    pub cgroup_id: u64,
}

/// Engine to helper. Exactly R-004's allowlist.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum HelperRequest {
    Hello {
        proto: u16,
    },
    Signal {
        target: HelperTarget,
        signal: Signal,
    },
    SetNice {
        target: HelperTarget,
        nice: i8,
    },
    SetSched {
        target: HelperTarget,
        policy: SchedPolicy,
        rt_prio: u8,
    },
    SetAffinity {
        target: HelperTarget,
        set: CpuSet,
    },
    SetIoPriority {
        key: ProcessKey,
        prio: IoPriority,
    },
    FreezeGroup(HelperCgroup),
    ThawGroup(HelperCgroup),
    KillGroup(HelperCgroup),
    ReadProcessFiles {
        key: ProcessKey,
        files: ProcFiles,
    },
    /// Streams `Strings` batches until `last`.
    ScanStrings {
        key: ProcessKey,
        opts: StringScanOpts,
    },
    /// The output file descriptor travels as `SCM_RIGHTS` data with this frame (R-145).
    Dump {
        key: ProcessKey,
        mode: DumpMode,
    },
    ResolveSocketInodes {
        inodes: Vec<u64>,
    },
    SockDestroy {
        family: Family,
        proto: Proto,
        four_tuple: FourTuple,
        cookie: u64,
    },
    /// The manager is always System. Empty `keys` deletes the drop-in file. Checked with
    /// `validate_drop_in` on both sides.
    WriteServiceDropIn {
        unit: String,
        keys: Vec<(DropInKey, String)>,
    },
    NetnsSockDiagDump {
        netns_ino: u64,
    },
    /// Stops the `ScanStrings` stream that has this frame `seq` (R-157).
    CancelScan {
        seq: u32,
    },
}

/// Engine to helper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HelperFrame {
    pub seq: u32,
    pub request: HelperRequest,
}

/// The engine maps these to `ActionError` (`PermissionDenied`, `NoSuchProcess`, `Partial`, `Os`)
/// or to `Rejected(IdentityChanged)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperError {
    IdentityChanged,
    PermissionDenied,
    NoSuchProcess,
    NotAllowed,
    Partial {
        done: u16,
        total: u16,
    },
    /// errno.
    Os(i32),
    /// The reply would pass the 4 MiB cap: per reply message for `ScanStrings` and
    /// `NetnsSockDiagDump`, in total for the rest (R-157).
    TooLarge,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum HelperReplyBody {
    Hello {
        proto: u16,
    },
    Ok,
    Err(HelperError),
    /// Raw content of one file (`file` has exactly one bit). A file larger than one frame comes
    /// as several `File` replies.
    File {
        file: ProcFiles,
        data: Vec<u8>,
    },
    /// (address, encoding, text).
    Strings(Vec<(u64, StringEncoding, String)>),
    /// (socket inode, owner).
    SocketOwners(Vec<(u64, ProcessKey)>),
    /// Raw sock_diag messages.
    NetlinkDump(Vec<u8>),
}

/// Helper to engine. Every request gets one or more replies with its `seq`; the final one has
/// `last` set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HelperReply {
    pub seq: u32,
    pub last: bool,
    pub body: HelperReplyBody,
}

/// Checks a `WriteServiceDropIn` request. The engine runs it before sending and the helper on
/// receipt.
///
/// The unit name must be non-empty, at most 255 bytes, contain no '/', NUL or "..", and end in
/// a known unit suffix. Values must contain no '\n', '\r' or NUL, because systemd ends a line
/// at any of them and a second line would inject a key. Any failure is `NotAllowed`.
pub fn validate_drop_in(unit: &str, keys: &[(DropInKey, String)]) -> Result<(), HelperError> {
    let unit_ok = !unit.is_empty()
        && unit.len() <= MAX_UNIT_NAME_LEN
        && !unit.contains(['/', '\0'])
        && !unit.contains("..")
        && UnitType::from_unit_name(unit).is_some();
    let values_ok = keys
        .iter()
        .all(|(_, value)| !value.contains(['\n', '\r', '\0']));
    if unit_ok && values_ok {
        Ok(())
    } else {
        Err(HelperError::NotAllowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_in_valid() {
        let keys = [(DropInKey::Restart, "on-failure".to_string())];
        assert_eq!(validate_drop_in("sshd.service", &keys), Ok(()));
        assert_eq!(validate_drop_in("sshd.service", &[]), Ok(()));
    }

    #[test]
    fn drop_in_rejects_bad_unit_names() {
        let keys = [(DropInKey::Restart, "on-failure".to_string())];
        let long = format!("{}.service", "a".repeat(MAX_UNIT_NAME_LEN));
        for unit in [
            "../etc/passwd",
            "a/b.service",
            "",
            "x\0.service",
            "sshd",
            "a..b.service",
            "sshd.device",
            long.as_str(),
        ] {
            assert_eq!(
                validate_drop_in(unit, &keys),
                Err(HelperError::NotAllowed),
                "{unit:?}"
            );
        }
    }

    /// Security regression: a newline in a value would add a second key such as ExecStart.
    #[test]
    fn drop_in_rejects_newline_value() {
        for value in [
            "on-failure\nExecStart=/bin/sh",
            "on-failure\rExecStart=/bin/sh",
            "on-failure\0",
        ] {
            let keys = [
                (DropInKey::RestartSec, "5".to_string()),
                (DropInKey::Restart, value.to_string()),
            ];
            assert_eq!(
                validate_drop_in("sshd.service", &keys),
                Err(HelperError::NotAllowed),
                "{value:?}"
            );
        }
    }

    /// Fails to compile when a variant is added: a new request needs a security review (R-157).
    #[test]
    fn helper_request_variant_count() {
        fn index(request: &HelperRequest) -> usize {
            match request {
                HelperRequest::Hello { .. } => 0,
                HelperRequest::Signal { .. } => 1,
                HelperRequest::SetNice { .. } => 2,
                HelperRequest::SetSched { .. } => 3,
                HelperRequest::SetAffinity { .. } => 4,
                HelperRequest::SetIoPriority { .. } => 5,
                HelperRequest::FreezeGroup(_) => 6,
                HelperRequest::ThawGroup(_) => 7,
                HelperRequest::KillGroup(_) => 8,
                HelperRequest::ReadProcessFiles { .. } => 9,
                HelperRequest::ScanStrings { .. } => 10,
                HelperRequest::Dump { .. } => 11,
                HelperRequest::ResolveSocketInodes { .. } => 12,
                HelperRequest::SockDestroy { .. } => 13,
                HelperRequest::WriteServiceDropIn { .. } => 14,
                HelperRequest::NetnsSockDiagDump { .. } => 15,
                HelperRequest::CancelScan { .. } => 16,
            }
        }
        const VARIANTS: usize = 17;
        let last = HelperRequest::CancelScan { seq: 0 };
        assert_eq!(index(&last) + 1, VARIANTS);
    }

    #[test]
    fn proc_files_bits_distinct() {
        assert_eq!(ProcFiles::ROOT.bits(), 1 << 11);
        let mut seen = 0u16;
        for (name, flag) in ProcFiles::all().iter_names() {
            assert_eq!(flag.bits().count_ones(), 1, "{name}");
            assert_eq!(seen & flag.bits(), 0, "{name} shares a bit");
            seen |= flag.bits();
        }
        assert_eq!(ProcFiles::all().iter_names().count(), 12);
        assert_eq!(seen, (1 << 12) - 1);
    }

    #[test]
    fn drop_in_key_names() {
        assert_eq!(DropInKey::Restart.as_str(), "Restart");
        assert_eq!(
            DropInKey::StartLimitIntervalSec.as_str(),
            "StartLimitIntervalSec"
        );
        assert_eq!(DropInKey::OnFailure.as_str(), "OnFailure");
    }

    #[test]
    fn proto_version_one() {
        assert_eq!(HELPER_PROTO_VERSION, 1);
    }
}
