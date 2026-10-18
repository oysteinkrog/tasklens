//! Properties payloads, affinity masks and handle search state (ADR-003).
//!
//! The `ProcessDetail` worker fills one `TargetDetail` per target in `Demand.details`; the
//! Properties windows read it. Live numbers (CPU, memory, I/O and network rates, GPU) are not
//! here: the window reads them from `ProcessRecord`. Each tab payload is a whole `Arc`, and an
//! unchanged tab reuses its `Arc`, so the UI compares pointers.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::actions::SchedPolicy;
use crate::ids::{GroupKey, ProcessKey};
use crate::process::ProcState;
use crate::snapshot::ComponentHeader;
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
    /// Personality ADDR_NO_RANDOMIZE. `Failed` with a valid `aslr_system` means the personality of
    /// a process of the user's own could not be read because of the kernel ptrace setting.
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
    /// `Valid(None)` when there is no cgroup v2 path; `Restricted` on EACCES.
    pub cgroup: Total<Option<Arc<str>>>,
    /// `Valid(None)` when the cgroup path names no unit. It has the validity of `cgroup`.
    pub unit: Total<Option<UnitRef>>,
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

// ---- File descriptors (UI-PWN-012) ----

/// The Type column of the File descriptors tab.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FdKind {
    File,
    Directory,
    Tcp,
    Udp,
    Unix,
    Netlink,
    Pipe,
    Fifo,
    Eventfd,
    Eventpoll,
    Timerfd,
    Signalfd,
    Inotify,
    Fanotify,
    Memfd,
    Dmabuf,
    Drm,
    Pidfd,
    IoUring,
    AnonInode,
    Device,
}

impl FdKind {
    /// "file", "directory", "TCP", "UDP", "unix", "netlink", "pipe", "FIFO", "eventfd", "eventpoll",
    /// "timerfd", "signalfd", "inotify", "fanotify", "memfd", "dmabuf", "DRM", "pidfd", "io_uring",
    /// "anon_inode", "device"
    pub const fn label(self) -> &'static str {
        match self {
            FdKind::File => "file",
            FdKind::Directory => "directory",
            FdKind::Tcp => "TCP",
            FdKind::Udp => "UDP",
            FdKind::Unix => "unix",
            FdKind::Netlink => "netlink",
            FdKind::Pipe => "pipe",
            FdKind::Fifo => "FIFO",
            FdKind::Eventfd => "eventfd",
            FdKind::Eventpoll => "eventpoll",
            FdKind::Timerfd => "timerfd",
            FdKind::Signalfd => "signalfd",
            FdKind::Inotify => "inotify",
            FdKind::Fanotify => "fanotify",
            FdKind::Memfd => "memfd",
            FdKind::Dmabuf => "dmabuf",
            FdKind::Drm => "DRM",
            FdKind::Pidfd => "pidfd",
            FdKind::IoUring => "io_uring",
            FdKind::AnonInode => "anon_inode",
            FdKind::Device => "device",
        }
    }
}

/// One open file descriptor.
#[derive(Clone, Debug)]
pub struct FdRow {
    pub fd: i32,
    pub kind: FdKind,
    /// Readlink target or socket text, sanitized.
    pub name: Arc<str>,
    /// anon_inode and other unnamed handles ("Hide unnamed handles").
    pub unnamed: bool,
    /// fdinfo pos. `None` for sockets and pipes ("-").
    pub pos: Option<u64>,
    /// fdinfo flags, shown in octal as "Raw flags". `None` when fdinfo is unreadable.
    pub flags: Option<u32>,
    pub mnt_id: Option<u32>,
    pub inode: Option<u64>,
    /// fdinfo eventfd-count.
    pub eventfd_count: Option<u64>,
    /// fdinfo drm-client-id.
    pub drm_client_id: Option<u64>,
    /// fdinfo drm-driver.
    pub drm_driver: Option<Arc<str>>,
    /// For "Go to connection" on TCP, UDP and unix rows.
    pub socket_inode: Option<u64>,
}

/// The File descriptors tab. `fdinfo_available` false is "Handle detail query unavailable."
#[derive(Clone, Debug, Default)]
pub struct FdDetail {
    pub rows: Arc<[FdRow]>,
    pub fdinfo_available: bool,
}

// ---- Memory maps (UI-PWN-013) ----

/// One mapping from /proc/<pid>/maps.
#[derive(Clone, Debug)]
pub struct MapRegion {
    pub start: u64,
    pub end: u64,
    /// For example `*b"r-xp"`.
    pub perms: [u8; 4],
    pub offset: u64,
    /// From smaps at most once per second. `WarmingUp` before the first read.
    pub rss_bytes: Total<u64>,
    /// "[heap]", "[stack]", "[vdso]", "[vvar]", "anonymous" or "memfd:<name>".
    pub label: Option<Arc<str>>,
}

/// One module node of the Memory maps tree.
#[derive(Clone, Debug)]
pub struct MapNode {
    /// File name. The UI adds " (Primary)" or shows "Outside loaded modules".
    pub name: Arc<str>,
    pub path: Option<Arc<str>>,
    /// The main executable.
    pub primary: bool,
    /// The "Outside loaded modules" node.
    pub outside: bool,
    /// Counts for "Shared libraries only", including Wine .dll mappings.
    pub shared_object: bool,
    /// Lowest region start.
    pub base: u64,
    /// Sum of region sizes.
    pub size_bytes: u64,
    /// Sum of region RSS.
    pub rss_bytes: Total<u64>,
    /// Package %DESC%.
    pub description: Option<Arc<str>>,
    /// For example "glibc 2.42-1".
    pub package: Option<Arc<str>>,
    /// Indexes into `MapsDetail.regions`, in address order.
    pub regions: std::ops::Range<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct MapsDetail {
    pub nodes: Arc<[MapNode]>,
    pub regions: Arc<[MapRegion]>,
}

// ---- Security (UI-PWN-014, UI-PWN-015) ----

/// UI words: "Yes (root)", "Yes (capabilities)", "Setuid", "No".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Elevated {
    Root,
    Capabilities,
    Setuid,
    No,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NsKind {
    User,
    Pid,
    Net,
    Mnt,
    Ipc,
    Uts,
    Cgroup,
    Time,
}

/// UI words: "Shared with init", "Isolated (4026532211)", "Same as tasklens", "Isolated (inode)".
/// The last two are used without access to PID 1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NsState {
    SharedWithInit,
    Isolated { inode: u64 },
    SameAsTasklens,
    IsolatedFromTasklens { inode: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NsRow {
    pub kind: NsKind,
    pub state: NsState,
    pub validity: Validity,
}

/// One capability row. Names and descriptions are a UI table keyed by `cap`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CapRow {
    /// Linux capability number.
    pub cap: u8,
    pub effective: bool,
    pub permitted: bool,
    pub inheritable: bool,
    pub bounding: bool,
    pub ambient: bool,
    pub risky: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupMembershipRow {
    pub name: Arc<str>,
    pub gid: u32,
    pub primary: bool,
    /// docker, libvirt, lxd, disk.
    pub root_equivalent: bool,
    /// wheel, sudo, root, adm.
    pub admin: bool,
}

/// The Security tab.
#[derive(Clone, Debug)]
pub struct SecurityDetail {
    pub uids: Total<IdTriple>,
    /// Real, effective, saved and fs user names.
    pub uid_names: Total<[Arc<str>; 4]>,
    pub gids: Total<IdTriple>,
    /// Real, effective, saved and fs group names.
    pub gid_names: Total<[Arc<str>; 4]>,
    pub elevated: Total<Elevated>,
    pub no_new_privs: Total<bool>,
    pub seccomp: Total<Seccomp>,
    pub dumpable: Total<bool>,
    /// attr/current. `Unsupported` when no LSM exposes it.
    pub security_label: Total<Arc<str>>,
    /// logind session id. `None` is "None".
    pub session: Option<Arc<str>>,
    /// `None` for 4294967295.
    pub audit_session: Option<u32>,
    /// `None` for 4294967295.
    pub login_uid: Option<u32>,
    /// In `NsKind` declaration order.
    pub namespaces: [NsRow; 8],
    /// Speculation_Store_Bypass text as given.
    pub store_bypass: Total<Arc<str>>,
    /// SpeculationIndirectBranch text as given.
    pub indirect_branch: Total<Arc<str>>,
    /// Capabilities set in Effective, Permitted, Inheritable or Ambient.
    pub caps: Arc<[CapRow]>,
    pub groups: Arc<[GroupMembershipRow]>,
}

// ---- Environment (UI-PWN-016) ----

/// The Environment tab. Values are secrets in practice: never log this or write it to disk.
/// `Debug` prints only the number of variables and `truncated`.
#[derive(Clone, Default)]
pub struct EnvDetail {
    /// Name and value. Read once on open and on DetailRefresh.
    pub vars: Arc<[(Box<str>, Box<str>)]>,
    /// Cut at 1 MiB.
    pub truncated: bool,
    /// A "(read failed)" row with the errno text.
    pub read_errno: Option<i32>,
    /// Zombie or kernel thread: "Environment block unavailable".
    pub unavailable: bool,
}

impl std::fmt::Debug for EnvDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvDetail")
            .field("vars", &self.vars.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

/// R-099: names masked by default, case-insensitive substring match.
pub const ENV_MASK_PATTERNS: [&str; 7] = [
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "KEY",
    "COOKIE",
    "CREDENTIAL",
];

/// True when `name` contains any of `ENV_MASK_PATTERNS`, ignoring ASCII case.
pub fn env_name_is_masked(name: &str) -> bool {
    let name = name.as_bytes();
    ENV_MASK_PATTERNS.iter().any(|pattern| {
        let pattern = pattern.as_bytes();
        name.windows(pattern.len())
            .any(|window| window.eq_ignore_ascii_case(pattern))
    })
}

/// One Strings scan. Allocated by the UI, starting at 1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ScanId(pub u64);

/// One handle search. Allocated by the UI, starting at 1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SearchId(pub u64);

// ---- Strings (UI-PWN-017, PROC-084) ----

/// Options of the Strings tab. Default: `min_len` 4, all three encodings on, file-backed
/// regions off.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StringScanOpts {
    pub min_len: u8,
    pub ascii: bool,
    pub utf8: bool,
    pub utf16le: bool,
    pub include_file_backed: bool,
}

impl StringScanOpts {
    pub const MIN_LEN_RANGE: std::ops::RangeInclusive<u8> = 4..=64;

    /// False disables Scan ("Choose at least one encoding").
    pub fn any_encoding(&self) -> bool {
        self.ascii || self.utf8 || self.utf16le
    }
}

impl Default for StringScanOpts {
    fn default() -> Self {
        Self {
            min_len: 4,
            ascii: true,
            utf8: true,
            utf16le: true,
            include_file_backed: false,
        }
    }
}

/// A scan stops with `ScanPhase::StoppedLimit` at this many hits.
pub const STRING_SCAN_MAX_HITS: u32 = 1_000_000;
/// A scan stops with `ScanPhase::StoppedLimit` at this much string text.
pub const STRING_SCAN_MAX_TEXT_BYTES: u64 = 256 * 1024 * 1024;

/// UI words: "ASCII", "UTF-8", "UTF-16LE".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum StringEncoding {
    Ascii,
    Utf8,
    Utf16Le,
}

/// One string found in process memory. `value` may hold secrets: never log it above debug level.
#[derive(Clone, Debug)]
pub struct StringHit {
    pub address: u64,
    pub encoding: StringEncoding,
    pub length: u32,
    pub value: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanPhase {
    Scanning,
    Finished,
    Cancelled,
    /// Stopped at `STRING_SCAN_MAX_HITS` or `STRING_SCAN_MAX_TEXT_BYTES`.
    StoppedLimit,
    /// "Scan stopped: the process exited."
    StoppedExited,
    /// "Failed to read process memory." The text lives in the UI. The text
    /// [`SCAN_PTRACE_BLOCKED`] means the kernel ptrace setting refused a process of the same user.
    Failed(Arc<str>),
}

/// The `ScanPhase::Failed` text for a process of the user's own that Yama `ptrace_scope` keeps
/// tasklens from reading. The UI shows its own words for it.
pub const SCAN_PTRACE_BLOCKED: &str = "ptrace-blocked";

/// The Strings tab.
#[derive(Clone, Debug)]
pub struct StringScanState {
    pub scan: ScanId,
    pub progress_pct: u8,
    pub found: u32,
    /// Append-only. Each new payload reuses the old chunk `Arc`s, so hits are never copied.
    pub chunks: Arc<[Arc<[StringHit]>]>,
    pub phase: ScanPhase,
}

// ---- App Properties (UI-PWN-020) ----

/// UI words: "App", "Unit".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppKind {
    App,
    Unit,
}

/// The General tab of an app group.
#[derive(Clone, Debug, PartialEq)]
pub struct AppGeneralDetail {
    pub name: Arc<str>,
    pub kind: AppKind,
    pub icon_path: Option<Arc<str>>,
    pub app_id: Option<Arc<str>>,
    pub desktop_file: Option<Arc<str>>,
    pub unit: Option<UnitRef>,
    pub cgroup: Option<Arc<str>>,
    pub processes: u32,
    /// cgroup memory.stat anon.
    pub memory_anon_bytes: Total<u64>,
    /// memory.swap.current.
    pub swap_bytes: Total<u64>,
    /// io.stat rbytes delta.
    pub disk_read_bps: Total<f64>,
    /// io.stat wbytes delta.
    pub disk_write_bps: Total<f64>,
    /// "Command line (primary process)".
    pub primary_cmdline: Total<Arc<str>>,
}

// ---- Set Affinity (UI-PWN-021) ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AffinityInfo {
    pub mask: CpuSet,
    pub cpuset_limit: CpuSet,
}

// ---- Search File Handles (UI-PWN-030, UI-PWN-031) ----

/// UI words: "Initializing... please wait.", "Retrieving file handles... 42% (18 234 total
/// handles)", "Searching...", "Found 12 matches" / "Found 1 match" / "No results found.",
/// "Search stopped. Found 12 matches", "Handle search failed".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HandlePhase {
    Initializing,
    Retrieving,
    Searching,
    Done,
    Stopped,
    Failed,
}

/// UI words: "fd", "mmap", "cwd", "exe", "root".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HandleKind {
    Fd,
    Mmap,
    Cwd,
    Exe,
    Root,
}

/// One match. `path` may be private: never log it above debug level.
#[derive(Clone, Debug)]
pub struct HandleHit {
    pub key: ProcessKey,
    pub name: Arc<str>,
    pub user: Arc<str>,
    pub kind: HandleKind,
    pub fd: Option<i32>,
    pub path: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct HandleSearchState {
    pub search: SearchId,
    pub phase: HandlePhase,
    pub progress_pct: u8,
    /// "(18 234 total handles)".
    pub total_handles: u64,
    /// Append-only, never copied.
    pub chunks: Arc<[Arc<[HandleHit]>]>,
    /// Processes searched.
    pub searched: u32,
    /// Processes in total.
    pub total: u32,
    /// Processes that need administrator rights.
    pub skipped: u32,
}

// ---- The component ----

#[derive(Clone, Debug, Default)]
pub struct DetailsSnapshot {
    pub header: ComponentHeader,
    /// At most 16 (`Demand.details`).
    pub targets: HashMap<DetailTarget, Arc<TargetDetail>>,
    /// Keys of `Demand.affinity_targets`, at most 256.
    pub affinity: HashMap<ProcessKey, AffinityInfo>,
    pub handle_search: Option<Arc<HandleSearchState>>,
}

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

    #[test]
    fn fd_kind_labels() {
        let expected = [
            (FdKind::File, "file"),
            (FdKind::Directory, "directory"),
            (FdKind::Tcp, "TCP"),
            (FdKind::Udp, "UDP"),
            (FdKind::Unix, "unix"),
            (FdKind::Netlink, "netlink"),
            (FdKind::Pipe, "pipe"),
            (FdKind::Fifo, "FIFO"),
            (FdKind::Eventfd, "eventfd"),
            (FdKind::Eventpoll, "eventpoll"),
            (FdKind::Timerfd, "timerfd"),
            (FdKind::Signalfd, "signalfd"),
            (FdKind::Inotify, "inotify"),
            (FdKind::Fanotify, "fanotify"),
            (FdKind::Memfd, "memfd"),
            (FdKind::Dmabuf, "dmabuf"),
            (FdKind::Drm, "DRM"),
            (FdKind::Pidfd, "pidfd"),
            (FdKind::IoUring, "io_uring"),
            (FdKind::AnonInode, "anon_inode"),
            (FdKind::Device, "device"),
        ];
        for (kind, label) in expected {
            assert_eq!(kind.label(), label);
        }
    }

    #[test]
    fn env_mask_patterns_case_insensitive() {
        for name in [
            "api_key",
            "GH_TOKEN",
            "MyPassword",
            "SESSION_COOKIE",
            "AWS_SECRET_ACCESS_KEY",
            "DB_PASSWD",
            "GOOGLE_CREDENTIALS",
        ] {
            assert!(env_name_is_masked(name), "{name} should be masked");
        }
        for name in ["PATH", "HOME", "DISPLAY", "LANG"] {
            assert!(!env_name_is_masked(name), "{name} should not be masked");
        }
    }

    #[test]
    fn env_detail_default() {
        let env = EnvDetail::default();
        assert!(env.vars.is_empty());
        assert!(!env.truncated);
        assert_eq!(env.read_errno, None);
        assert!(!env.unavailable);
    }

    #[test]
    fn env_detail_debug_hides_values() {
        let env = EnvDetail {
            vars: Arc::from(vec![("GH_TOKEN".into(), "hunter2-value".into())]),
            ..EnvDetail::default()
        };
        let text = format!("{env:?}");
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("GH_TOKEN"));
        assert!(text.contains("vars: 1"));
    }

    #[test]
    fn string_scan_opts_default() {
        let opts = StringScanOpts::default();
        assert_eq!(opts.min_len, 4);
        assert!(opts.ascii && opts.utf8 && opts.utf16le);
        assert!(!opts.include_file_backed);
        assert!(opts.any_encoding());
        assert!(StringScanOpts::MIN_LEN_RANGE.contains(&opts.min_len));
    }

    #[test]
    fn string_scan_no_encoding() {
        let opts = StringScanOpts {
            ascii: false,
            utf8: false,
            utf16le: false,
            ..StringScanOpts::default()
        };
        assert!(!opts.any_encoding());
    }

    #[test]
    fn details_snapshot_default_empty() {
        let snap = DetailsSnapshot::default();
        assert!(snap.targets.is_empty());
        assert!(snap.affinity.is_empty());
        assert!(snap.handle_search.is_none());
    }

    #[test]
    fn string_scan_limits() {
        assert_eq!(STRING_SCAN_MAX_HITS, 1_000_000);
        assert_eq!(STRING_SCAN_MAX_TEXT_BYTES, 268_435_456);
    }
}
