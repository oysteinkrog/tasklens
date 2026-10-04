//! Properties payloads, affinity masks and handle search state (ADR-003).
//!
//! The `ProcessDetail` worker fills one `TargetDetail` per target in `Demand.details`; the
//! Properties windows read it. Live numbers (CPU, memory, I/O and network rates, GPU) are not
//! here: the window reads them from `ProcessRecord`. Each tab payload is a whole `Arc`, and an
//! unchanged tab reuses its `Arc`, so the UI compares pointers.

use std::sync::Arc;

use crate::actions::SchedPolicy;
use crate::ids::{GroupKey, ProcessKey};
use crate::process::ProcState;
use crate::system::CpuSet;
use crate::table::{Total, UnitRef};
use crate::validity::Validity;

/// What a Properties window shows: one process or one app group.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DetailTarget {
    Process(ProcessKey),
    App(GroupKey),
}

/// The Properties tabs. The declaration order is the index into `TargetDetail::tabs`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DetailTab {
    General,
    Threads,
    Fds,
    Maps,
    Security,
    Environment,
    Strings,
    AppGeneral,
    AppProcesses,
}

impl DetailTab {
    pub const ALL: [DetailTab; 9] = [
        DetailTab::General,
        DetailTab::Threads,
        DetailTab::Fds,
        DetailTab::Maps,
        DetailTab::Security,
        DetailTab::Environment,
        DetailTab::Strings,
        DetailTab::AppGeneral,
        DetailTab::AppProcesses,
    ];

    /// Declaration order, 0..=8.
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// The data of one tab. `AppProcesses` has no payload; it reads `Snapshot.processes`.
#[derive(Clone, Debug)]
pub enum TabPayload {
    General(Arc<GeneralDetail>),
    Threads(Arc<[ThreadRow]>),
    Fds(Arc<FdDetail>),
    Maps(Arc<MapsDetail>),
    Security(Arc<SecurityDetail>),
    Environ(Arc<EnvDetail>),
    Strings(Arc<StringScanState>),
    AppGeneral(Arc<AppGeneralDetail>),
}

/// One tab of one target, with its own version and validity.
#[derive(Clone, Debug, Default)]
pub struct TabSlot {
    pub payload: Option<Arc<TabPayload>>,
    pub version: u64,
    pub validity: Validity,
}

/// All tabs of one target, indexed by `DetailTab::index()`.
#[derive(Clone, Debug, Default)]
pub struct TargetDetail {
    pub tabs: [TabSlot; 9],
}

impl TargetDetail {
    pub fn tab(&self, tab: DetailTab) -> &TabSlot {
        &self.tabs[tab.index()]
    }
}

/// Why the executable hashes failed (UI-PWN-008).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HashError {
    /// UI: "Failed to compute one or more hashes", with the errno text.
    ReadFailed { errno: i32 },
    /// UI: "Executable path unavailable".
    ExePathUnavailable,
}

/// SHA-256 and MD5 of the executable (UI-PWN-008).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum HashState {
    #[default]
    Pending,
    Done {
        sha256: [u8; 32],
        md5: [u8; 16],
    },
    Failed(HashError),
}

/// Result of checking the executable against the package database (UI-PWN-008).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PackageCheck {
    /// "Not verified"
    #[default]
    NotVerified,
    /// "Verifying…"
    Verifying,
    /// "Matches package"
    Matches,
    /// "Does not match package"
    DoesNotMatch,
    /// "Not packaged"
    NotPackaged,
    /// "No checksum in package database"
    NoChecksum,
    /// Toast: "Could not verify against the package."
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relro {
    Full,
    Partial,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seccomp {
    Disabled,
    Strict,
    Filter { filters: u32 },
}

/// UI words: "x86-64", "x86 (32-bit)", "AArch64", "Unknown".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfArch {
    X86_64,
    X86,
    AArch64,
    Unknown,
}

/// Where the executable lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExeLocation {
    TempDirectory,
    UserDownloads,
    UserData,
    UserProfile,
    System,
    OptionalSoftware,
    Flatpak,
    AppImage,
    WinePrefix,
    Deleted,
    Other,
}

/// UI words: "3D", "Compute", "Video decode", "Video encode".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuEngine {
    ThreeD,
    Compute,
    VideoDecode,
    VideoEncode,
}

/// Real, effective, saved and filesystem IDs from /proc/<pid>/status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdTriple {
    pub real: u32,
    pub effective: u32,
    pub saved: u32,
    pub fs: u32,
}

/// Shown as "firefox 131.0-1".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: Arc<str>,
    pub version: Arc<str>,
    pub packager: Option<Arc<str>>,
}

/// The General tab values that are not in `ProcessRecord` (UI-PWN-006).
///
/// Exe, cwd, root, personality, io and smaps of another user's process are `Restricted`.
#[derive(Clone, Debug)]
pub struct GeneralDetail {
    // Column 1.
    /// Unix seconds, shown as "Started:" YYYY-MM-DD HH:MM:SS local time.
    pub started_s: Total<i64>,
    pub uids: Total<IdTriple>,
    /// Real, effective and saved user names.
    pub uid_names: Total<(Arc<str>, Arc<str>, Arc<str>)>,
    pub policy: Total<SchedPolicy>,
    pub rt_prio: u8,
    pub kernel_prio: Total<i32>,
    /// ("autogroup-12", nice). `Unsupported` when /proc/<pid>/autogroup is absent.
    pub autogroup: Total<(Arc<str>, i8)>,
    pub affinity: Total<CpuSet>,
    pub affinity_threads_differ: bool,
    pub pie: Total<bool>,
    pub nx_stack: Total<bool>,
    pub relro: Total<Relro>,
    /// `Unsupported` when there is no x86_Thread_features line.
    pub shadow_stack: Total<bool>,
    /// 0, 1 or 2 from /proc/sys/kernel/randomize_va_space.
    pub aslr_system: Total<u8>,
    /// Personality ADDR_NO_RANDOMIZE.
    pub aslr_disabled_for_process: Total<bool>,
    pub no_new_privs: Total<bool>,
    pub seccomp: Total<Seccomp>,
    pub architecture: Total<ElfArch>,
    pub exe: Total<Arc<str>>,
    pub exe_deleted: bool,
    pub location: Total<ExeLocation>,
    /// `Valid(None)` is "Not packaged".
    pub package: Total<Option<PackageInfo>>,
    /// Package %DESC%, else the desktop file Comment, else `Unsupported`.
    pub description: Total<Arc<str>>,
    pub cwd: Total<Arc<str>>,
    pub root: Total<Arc<str>>,
    pub cgroup: Option<Arc<str>>,
    pub unit: Option<UnitRef>,
    pub app_id: Option<Arc<str>>,
    /// Full command line, up to 32 KiB (R-008). Sanitized by tl-collect (ADR-015); never log it.
    pub cmdline: Total<Arc<str>>,
    // Columns 2 and 3.
    pub hashes: HashState,
    pub package_check: PackageCheck,
    pub handles: Total<u32>,
    /// RssFile + RssShmem.
    pub rss_shared_bytes: Total<u64>,
    /// VmData + VmStk.
    pub commit_bytes: Total<u64>,
    /// utime + stime.
    pub cpu_time_ms: Total<u64>,
    pub io_rchar: Total<u64>,
    pub io_wchar: Total<u64>,
    pub io_syscr: Total<u64>,
    pub io_syscw: Total<u64>,
    pub disk_read_bytes: Total<u64>,
    pub disk_write_bytes: Total<u64>,
    pub cancelled_write_bytes: Total<u64>,
    /// `Unsupported` when the driver does not report it.
    pub gpu_mem_shared_bytes: Total<u64>,
    /// `Valid(None)` is "None".
    pub gpu_engine: Total<Option<GpuEngine>>,
}

/// One row of the Threads tab (UI-PWN-010).
#[derive(Clone, Debug)]
pub struct ThreadRow {
    pub tid: u32,
    pub comm: Arc<str>,
    pub state: ProcState,
    pub cpu_pct: f32,
    pub utime_ms: u64,
    pub stime_ms: u64,
    pub policy: SchedPolicy,
    pub rt_prio: u8,
    pub nice: i8,
    /// Cpus_allowed_list text, for example "0-7,16-23".
    pub cpus: Arc<str>,
    pub last_cpu: u32,
    pub vol_ctx: u64,
    pub invol_ctx: u64,
    pub wchan: Arc<str>,
    pub validity: Validity,
}

// filled by E03-058
#[derive(Clone, Debug, Default)]
pub struct FdDetail {}

// filled by E03-058
#[derive(Clone, Debug, Default)]
pub struct MapsDetail {}

// filled by E03-058
#[derive(Clone, Debug, Default)]
pub struct SecurityDetail {}

// filled by E03-058
#[derive(Clone, Debug, Default)]
pub struct EnvDetail {}

// filled by E03-059
#[derive(Clone, Debug, Default)]
pub struct StringScanState {}

// filled by E03-059
#[derive(Clone, Debug, Default)]
pub struct AppGeneralDetail {}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::*;
    use crate::time::BootId;

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn detail_tab_index_order() {
        for (i, tab) in DetailTab::ALL.iter().enumerate() {
            assert_eq!(tab.index(), i);
        }
        assert_eq!(DetailTab::AppProcesses.index(), 8);
    }

    #[test]
    fn target_detail_default_slots() {
        let detail = TargetDetail::default();
        for tab in DetailTab::ALL {
            let slot = detail.tab(tab);
            assert!(slot.payload.is_none());
            assert_eq!(slot.version, 0);
            assert_eq!(slot.validity, Validity::WarmingUp);
        }
    }

    #[test]
    fn hash_and_package_defaults() {
        assert_eq!(HashState::default(), HashState::Pending);
        assert_eq!(PackageCheck::default(), PackageCheck::NotVerified);
    }

    #[test]
    fn detail_target_eq_hash() {
        let key = ProcessKey {
            pid: 42,
            start_ticks: 1000,
            boot_id: BootId(7),
        };
        let a = DetailTarget::Process(key);
        let b = DetailTarget::Process(key);
        assert_eq!(a, b);
        assert_eq!(hash_of(&a), hash_of(&b));
        assert_ne!(a, DetailTarget::App(GroupKey(1000)));
    }
}
