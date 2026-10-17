//! The `ProcessDetail` worker (ADR-008, ADR-016): PSS, USS and swap for `detail_keys`, and the
//! Properties tab payloads of `Demand.details`.
//!
//! `/proc/<pid>/smaps_rollup` is read at most once per `PSS_PERIOD` of clock time, only while a
//! PSS or USS column is shown and `detail_keys` (visible rows plus one viewport of overscan,
//! at most 512) is not empty. The `tl-collect` `PssReader` keeps up to 128 fds open between
//! samples (ADR-016), checks `start_ticks` when it opens a file and closes the fds of keys that
//! left the demand.
//!
//! Tabs (E10-004): the demanded tab of each demanded process target (at most `DETAILS_MAX`) is
//! read at `processes.interval_ms`: Threads, File descriptors, Memory maps and Security. General
//! (PROC-078, tasklens-dwy.13) is read at the same rate for every demanded process target,
//! whatever its tab, because it feeds the window title and the exited state. Memory
//! maps reads `smaps` for the RSS at most once per `SMAPS_MIN_PERIOD` per process, and keeps the
//! last RSS in between. The PSS and `smaps` limits use the clock passed to `sample_at`, not the
//! sample count, so a late sample, a period change or a reopened tab never reads early.
//! Environment is read when its tab is first demanded and again only on `DetailRefresh`. A tab
//! is sent only when the reader output changed, so an unchanged tab keeps its `Arc` in the
//! reducer. EACCES gives an empty `Restricted` payload. A gone process resends the last payload
//! as `Missing`, another read error resends it as `Failed`.
//!
//! `period` sees every demand: it forgets targets and tabs that left it, and gives a tab that
//! appears a new state marked `open_pending`. A new tab is read at the next sample. A tab that
//! closes and opens again between two polls is not seen here, so the reducer sends a
//! `DetailOpened` for every tab that opens. For a tab marked `open_pending` that notice only
//! resends the payload, because the worker already read the tab for this open. For any other
//! tab it drops the state and reads at once, which sends the tab again. A user `DetailRefresh`
//! always drops the state and reads at once; `open_pending` never applies to it, and it keeps the
//! flag for the notice still to come. General stays in this worker's demand while its target
//! does, so the worker never sees it open: every open notice for a target resends its General
//! payload and never reads it. Tab and affinity updates go out through `take_extra`. When PSS is
//! not due the sample itself is the last of them or `Health { Running }`, never an empty `Pss`.
//!
//! Hashes (PROC-074, E10-005): `ComputeHashes(key)` hashes the executable with
//! `hash_process_exe` in the next sample, where the target has a General tab state, and the
//! General payload then carries the `HashState`. A request for a target not in the demand yet
//! waits for it. The `HashCache` keys on (dev, inode, mtime), so a second request for the same
//! file reads nothing.
//!
//! Package check (PROC-078): `VerifyPackage(key)` runs `PackageIndex::verify` on the owner of the
//! General read in the next sample, once the package is known, and the General payload carries
//! the `PackageCheck`. A process that is not packaged is not verified and the check stays as is.
//!
//! Package (PROC-078, R-163 item 1): a General read looks the executable's path and mtime up in
//! the session's shared package index. A path the index does not know yet is queued for the
//! `Processes` worker's next batch and shows `WarmingUp`; a later read sends the name. A deleted
//! executable is not looked up. Without an index the field is `Unsupported`.
//!
//! Affinity (UI-PWN-021): while `affinity_targets` is not empty, every `AFFINITY_PERIOD` worth
//! of samples sends `DetailsUpdate::Affinity` with the `Cpus_allowed_list` mask and the cgroup's
//! `cpuset.cpus.effective` of every target. A gone process is left out; an unread cpuset gives an
//! empty limit, which means no limit. Beside tabs the period is the greatest common divisor of
//! the interval and 1 s, at least 250 ms, so a 1500 ms interval samples every 500 ms and a
//! 1600 ms one every 250 ms, and affinity reads at exactly 1 Hz.
//!
//! Strings (PROC-084, tasklens-dwy.9): `StringScan` queues a scan that the next sample starts.
//! A kernel thread gets "Strings unavailable for this process.". A target that Yama
//! `ptrace_scope` does not let tasklens attach to (scope 1 allows only descendants of tasklens,
//! 2 and 3 allow none) gets one `Restricted` payload, and its memory is never opened (see
//! Helper reads below for the scan through the helper). A running scan reads `/proc/<pid>/mem`
//! one `strings::CHUNK` per step, at most `SCAN_STEPS_PER_SAMPLE` steps per sample over all
//! scans in turn (each sample goes on where the last one stopped, so every scan gets reads), and
//! the period is `SCAN_PERIOD` while any scan runs or waits. Tabs and affinity then read by the
//! clock, so they keep their rates. A running scan is published at most every
//! `SCAN_PUBLISH_PERIOD`, and its end at once. `CancelStringScan` drops the scan and publishes
//! `Cancelled`, and no chunk is read after it. A scan whose process exits or turns zombie stops
//! with `StoppedExited` and keeps the rows found so far ("Scan stopped: the process exited",
//! UI-PWN-017). A scan whose process leaves the demand stops without a payload: its window
//! closed.
//!
//! Handle search (PROC-096, tasklens-dwy.3): `HandleSearch` queues one search over the open
//! handles of every process, and the next sample starts it with `HandleSearch::new`, so the proc
//! root and the passwd file are never read on the async loop. A newer request replaces the one
//! that waits, and when it starts, the running search is dropped without another payload, so the
//! old search id is never published again. While a search runs or waits the period is
//! `SCAN_PERIOD`, and each sample walks `HANDLE_SEARCH_BUDGET` pids and publishes one
//! `DetailsUpdate::HandleSearch`. The search is dropped after its `Done`, `Stopped` or `Failed`
//! state. `CancelHandleSearch` with the running id stops the walk before its next pid, and the
//! next sample publishes `Stopped`; a cancelled request that has not started publishes `Stopped`
//! with nothing searched. `privileged` is passed to the search; without the helper
//! (tasklens-qfx.19) the processes whose handles cannot be read count in `skipped`.
//!
//! Helper reads (E29-015, PROC-076, UI-PWN-018): `run` puts `HelperDetailReads` beside the
//! sample loop when the session has a helper. While the helper is `Unlocked`, a direct read that
//! gives `Restricted` queues one `ReadProcessFiles` through `HelperHandle::read`, which never
//! starts a session, so no polkit prompt comes from this worker. Environment reads `ENVIRON`
//! (only when the tab is read: on open and on `DetailRefresh`), Memory maps `MAPS | SMAPS`,
//! General the `EXE`, `CWD`, `ROOT`, `IO` and `STATUS` of its Restricted fields, and PSS and USS
//! `SMAPS_ROLLUP` for at most `HELPER_READS_MAX` keys per second. File descriptors and Security
//! stay Restricted (tasklens-qfx.39). Each (target, tab) and each PSS key gets at most one helper
//! read per `HELPER_PERIOD`, and none while another one waits: `HelperDetailReads` sends a read
//! no sooner than `HELPER_PERIOD` after the last read of the same slot returned, and drops a
//! queued read whose tab or column left the demand while it waited. The next sample parses the
//! reply with the same tl-collect parsers as the direct path and sends the tab. While a helper
//! payload is shown, a Restricted direct read keeps it. While the helper is `Locked` or `Missing`
//! no helper read is queued and Restricted stays. After the Grant access button the UI sends
//! `DetailRefresh` once, and that read goes through the helper (R-170 item 13). A file the
//! helper could not read gives Restricted (EACCES, EPERM), Missing (ENOENT, ESRCH) or Failed,
//! per General field too; EFBIG also sends `last_error` "too large to read through the helper".
//! A helper `IdentityChanged` or `NoSuchProcess` marks the target exited: the tab goes Missing
//! with its last data, and the PSS entry is dropped as for a gone process. A Strings scan whose
//! direct start gives Restricted runs as a helper `ScanStrings` stream
//! (`HelperHandle::read_stream`); `CancelStringScan` drops the stream, which sends `CancelScan`.
//!
//! Per-key errors become `Validity`, never `WorkerError`. Hash failures are a `HashState`.
//! Environment values, command lines and paths are never logged (ADR-015), nor are scan
//! options, found strings or their addresses, nor are search patterns or the paths found.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::{Cx, Outcome, Time};
use rustc_hash::{FxHashMap, FxHashSet};
use rustix::fd::{AsFd, BorrowedFd};
use rustix::fs::{AtFlags, Dir, Mode, OFlags, StatxFlags};
use smallvec::SmallVec;
use tl_collect::apps::AppPaths;
use tl_collect::apps::cgroup_app::parse_app_unit;
use tl_collect::apps::classify::{ClassifyInput, MatchInput, ProcClass, classify, match_app};
use tl_collect::apps::desktop::{DesktopIndex, DesktopWatch, data_dirs_from_env};
use tl_collect::hash::{self, HashCache, hash_process_exe};
use tl_collect::parse::topology::parse_cpu_list;
use tl_collect::pkg::{PackageIndex, VerifyOutcome};
use tl_collect::proc::cgroup::{
    UserPlacement, app_slice_unit, parse_cgroup_file, placement, unescape_unit,
};
use tl_collect::proc::detail::{
    self as rd, DetailRead, EnvInfo, EnvRead, FdInfo, FdType, LinkRead, LocationCtx, LsmLabel,
    MapNodeInfo, MapRegionInfo, SecurityInfo, ShadowStack, ThreadInfo, ThreadReader, WchanValue,
};
use tl_collect::proc::fdcount::{FdCount, fd_count};
use tl_collect::proc::io::{IoRead, PidIo, parse_io, read_io};
use tl_collect::proc::meta::{PasswdUserNames, ReadBufs, UserNames};
use tl_collect::proc::pid_stat::{PF_KTHREAD, PidStat, parse_pid_stat};
use tl_collect::proc::smaps::{
    PssReader, Rollup, RollupRead, parse_rollup, parse_smaps, rss_by_start,
};
use tl_collect::proc::status::{
    PidStatus, STATUS_READ_MAX, commit_bytes, parse_status, ws_shared_bytes,
};
use tl_collect::proc::strings::{
    Encoding, ProcMem, ScanEnd, ScanOpts, StringHitInfo, StringScanner, select_regions,
};
use tl_collect::sanitize::{
    CMDLINE_MAX, EXE_LINK_MAX, sanitize_cmdline, sanitize_link, sanitize_text,
};
use tl_collect::sysroot::{SMALL_FILE_LIMIT, read_at, read_file};
use tl_collect::{CollectError, SysRoot};
use tl_model::actions::SchedPolicy;
use tl_model::demand::{AFFINITY_TARGETS_MAX, DETAILS_MAX, Demand, DetailDemand};
use tl_model::details::{
    AffinityInfo, CapRow, DetailTab, DetailTarget, Elevated, ElfArch, EnvDetail, ExeLocation,
    FdDetail, FdKind, FdRow, GeneralDetail, GroupMembershipRow, HashError, HashState, IdTriple,
    MapNode, MapRegion, MapsDetail, NsKind, NsRow, NsState, PackageCheck, PackageInfo, Relro,
    ScanId, ScanPhase, SearchId, Seccomp, SecurityDetail, StringEncoding, StringHit,
    StringScanOpts, StringScanState, TabPayload, ThreadRow,
};
use tl_model::health::{HelperState, SourceId, SourceState};
use tl_model::helper_proto::{HelperError, HelperReply, HelperReplyBody, HelperRequest, ProcFiles};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::process::ProcState;
use tl_model::settings::EngineSettings;
use tl_model::system::CpuSet;
use tl_model::table::{Manager, Total, UnitRef};
use tl_model::time::{TimeNs, now_boot_ns};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::helper::{HelperCallError, HelperHandle, HelperReadError, HelperStream};
use crate::source::{self, Source, WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{DetailsUpdate, PssEntry, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::handle_search::HandleSearch;
use crate::workers::processes::{HELPER_READS_MAX, MAX_INTERVAL_MS, MIN_INTERVAL_MS};

/// PSS and USS are read once a second (ADR-008).
pub const PSS_PERIOD: Duration = Duration::from_secs(1);

/// Memory maps reads `smaps` (per-region RSS) at most this often (ADR-008).
pub const SMAPS_MIN_PERIOD: Duration = Duration::from_secs(1);

/// Affinity masks are read once a second while a Set Affinity dialog is open (ADR-008). It is
/// the PSS rate, so both share the sample period.
pub const AFFINITY_PERIOD: Duration = PSS_PERIOD;

/// The sample period while a Strings scan runs.
const SCAN_PERIOD: Duration = Duration::from_millis(50);

/// `strings::CHUNK` reads per sample over all scans, 8 MiB, so the other detail reads keep
/// running between them.
const SCAN_STEPS_PER_SAMPLE: u32 = 8;

/// Pids walked per sample by a handle search: 530 pids take 11 samples, about 0.6 s.
const HANDLE_SEARCH_BUDGET: usize = 50;

/// A running scan is published at most 4 times a second (docs/ui/process-windows.md).
const SCAN_PUBLISH_PERIOD: Duration = Duration::from_millis(250);

/// The Strings tab shows its Enable button with this text.
const SCAN_RESTRICTED: &str = "Strings unavailable: needs administrator rights";
const SCAN_KERNEL_THREAD: &str = "Strings unavailable for this process.";
const SCAN_READ_FAILED: &str = "Failed to read process memory.";

/// Parents checked for the Yama scope 1 rule.
const ANCESTOR_HOPS: u32 = 64;

/// Read limit for `ptrace_scope`.
const PTRACE_SCOPE_MAX: usize = 64;

/// Read limit for `<root>/etc/group`.
const GROUP_FILE_MAX: usize = 1024 * 1024;

/// Read limit for `/proc/stat`, whose `intr` line comes before `btime` and can be long.
const PROC_STAT_MAX: usize = 1024 * 1024;

/// Read limit for `user-dirs.dirs`.
const USER_DIRS_MAX: usize = 64 * 1024;

/// Read limit for the first command-line token, the same as for `RawMeta::argv`.
const ARGV0_MAX: usize = 4096;

/// The `/proc/<pid>/exe` link of a deleted executable ends with this (PROC-027).
const DELETED_SUFFIX: &str = " (deleted)";

/// Namespace link names in `NsKind` declaration order.
const NAMESPACES: [(NsKind, &str); 8] = [
    (NsKind::User, "user"),
    (NsKind::Pid, "pid"),
    (NsKind::Net, "net"),
    (NsKind::Mnt, "mnt"),
    (NsKind::Ipc, "ipc"),
    (NsKind::Uts, "uts"),
    (NsKind::Cgroup, "cgroup"),
    (NsKind::Time, "time"),
];

/// Each (target, tab) and each PSS key gets at most one helper read this often (PROC-076).
const HELPER_PERIOD: Duration = Duration::from_secs(1);

/// The sample side queues a read of a slot up to this much early, so sample jitter does not
/// halve the rate. It only paces the queue: `HelperDetailReads` sends a read no sooner than
/// `HELPER_PERIOD` after the last read of the same slot returned.
const HELPER_SLACK: Duration = Duration::from_millis(50);

/// Deadline of one helper `ReadProcessFiles`.
const HELPER_DEADLINE: Duration = Duration::from_secs(2);

/// Deadline of a helper Strings scan. The helper runs one call at a time, so other helper calls
/// wait behind a scan.
const HELPER_SCAN_DEADLINE: Duration = Duration::from_secs(120);

/// EFBIG: the file did not fit the helper's 4 MiB reply (E29-004).
const EFBIG: i32 = 27;

/// `last_error` of a tab whose file the helper could not send.
const HELPER_TOO_LARGE: &str = "too large to read through the helper";

/// One helper read: the PSS of a key, or one tab of a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum HelperSlot {
    Pss(ProcessKey),
    Tab(ProcessKey, DetailTab),
}

impl HelperSlot {
    fn key(self) -> ProcessKey {
        match self {
            HelperSlot::Pss(key) | HelperSlot::Tab(key, _) => key,
        }
    }
}

/// The result of one helper `ReadProcessFiles`.
type HelperResult = Result<Vec<HelperReply>, HelperReadError>;

/// One file of a `ReadProcessFiles` reply: its data, or the errno the helper gave.
type HelperFile = (ProcFiles, Result<Vec<u8>, i32>);

/// Work for `HelperDetailReads`.
enum HelperJob {
    Read {
        slot: HelperSlot,
        files: ProcFiles,
    },
    /// A Strings scan. `stop` closes when the sample side drops the scan.
    Scan {
        scan: ScanId,
        key: ProcessKey,
        opts: StringScanOpts,
        stop: async_channel::Receiver<()>,
    },
}

/// How a helper Strings scan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelperScanEnd {
    Finished,
    /// The helper's scan limits (`Err(TooLarge)`).
    Limit,
    Exited,
    Restricted,
    Failed,
}

/// What a running helper scan found since the last sample.
#[derive(Default)]
struct ScanFeed {
    hits: Vec<StringHit>,
    end: Option<HelperScanEnd>,
}

/// One queued helper read after `HelperDetailReads` took it.
struct HelperDone {
    slot: HelperSlot,
    files: ProcFiles,
    /// `None` when the read was not sent because its slot left the demand while it waited.
    result: Option<HelperResult>,
}

/// The slots the demand shows now. `period` sets it on every demand change.
#[derive(Default)]
struct Wanted {
    details: Arc<[DetailDemand]>,
    /// PSS keys, empty while no PSS or USS column is shown.
    pss: Arc<[ProcessKey]>,
}

impl Wanted {
    /// True while `slot` is demanded: General while any tab of its target is, as in `period`.
    fn contains(&self, slot: HelperSlot) -> bool {
        match slot {
            HelperSlot::Pss(key) => self.pss.contains(&key),
            HelperSlot::Tab(key, DetailTab::General) => self
                .details
                .iter()
                .any(|d| d.target == DetailTarget::Process(key)),
            HelperSlot::Tab(key, tab) => self.details.contains(&DetailDemand {
                target: DetailTarget::Process(key),
                tab,
            }),
        }
    }
}

/// Written by `HelperDetailReads`, read by the sample.
#[derive(Default)]
struct HelperStore {
    reads: Vec<HelperDone>,
    /// One entry per helper scan the sample side runs. A scan without an entry was dropped.
    scans: FxHashMap<ScanId, ScanFeed>,
    /// Checked just before each read is sent, so a read queued for a tab that closed since,
    /// for example while it waited behind a scan, is never sent.
    wanted: Wanted,
}

fn lock(store: &Mutex<HelperStore>) -> std::sync::MutexGuard<'_, HelperStore> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The sample's side of the helper reads.
struct HelperSide {
    handle: HelperHandle,
    jobs: async_channel::Sender<HelperJob>,
    store: Arc<Mutex<HelperStore>>,
    /// Slots with a read queued and not answered yet.
    pending: FxHashSet<HelperSlot>,
    /// Clock time of the last read queued per slot, for `HELPER_PERIOD`.
    last: FxHashMap<HelperSlot, TimeNs>,
    /// The last PSS key read: the next round starts after it, so keys past the cap get a turn.
    pss_cursor: Option<ProcessKey>,
    /// PSS, USS and swap from the helper, by key.
    pss: FxHashMap<ProcessKey, HelperPss>,
}

/// The PSS of a key from a helper `SMAPS_ROLLUP`.
enum HelperPss {
    Ok(Rollup),
    /// The process exited or its pid was reused (`IdentityChanged`, `NoSuchProcess`): the
    /// entry is dropped, as for `RollupRead::Gone`.
    Gone,
    /// The file could not be read or parsed: Missing or Failed.
    Invalid(Validity),
}

impl HelperSide {
    fn unlocked(&self) -> bool {
        self.handle.state() == HelperState::Unlocked
    }

    /// Queues one read of `files` for `slot`. Refused while the helper is not `Unlocked`, while
    /// a read of the slot waits, and within `HELPER_PERIOD - HELPER_SLACK` of the last one
    /// queued. `HelperDetailReads` holds the full `HELPER_PERIOD` between sent reads.
    fn request(&mut self, slot: HelperSlot, files: ProcFiles, now: TimeNs) -> bool {
        let last = self.last.get(&slot).copied();
        if !self.unlocked()
            || self.pending.contains(&slot)
            || !elapsed(last, now, HELPER_PERIOD - HELPER_SLACK)
        {
            return false;
        }
        if self.jobs.try_send(HelperJob::Read { slot, files }).is_err() {
            return false;
        }
        self.pending.insert(slot);
        self.last.insert(slot, now);
        true
    }

    /// The answered reads since the last call.
    fn take_reads(&mut self, now: TimeNs) -> Vec<HelperDone> {
        let reads = std::mem::take(&mut lock(&self.store).reads);
        for done in &reads {
            self.pending.remove(&done.slot);
        }
        self.last
            .retain(|_, at| !elapsed(Some(*at), now, HELPER_PERIOD));
        reads
    }

    /// Queues `SMAPS_ROLLUP` reads for the Restricted keys of a PSS round, at most
    /// `HELPER_READS_MAX`, starting after `pss_cursor`. Drops the values of other keys.
    fn pss_round(&mut self, reads: &[(ProcessKey, RollupRead)], now: TimeNs) {
        if !self.unlocked() {
            self.pss.clear();
            return;
        }
        let mut restricted: Vec<ProcessKey> = reads
            .iter()
            .filter(|(_, read)| matches!(read, RollupRead::Restricted))
            .map(|&(key, _)| key)
            .collect();
        restricted.sort_unstable();
        self.pss
            .retain(|key, _| restricted.binary_search(key).is_ok());
        let split = match self.pss_cursor {
            Some(cursor) => restricted.partition_point(|key| *key <= cursor),
            None => 0,
        };
        let (before, after) = restricted.split_at(split);
        let mut sent = 0;
        for &key in after.iter().chain(before) {
            if sent == HELPER_READS_MAX {
                break;
            }
            if self.request(HelperSlot::Pss(key), ProcFiles::SMAPS_ROLLUP, now) {
                sent += 1;
                self.pss_cursor = Some(key);
            }
        }
    }

    /// Starts a helper scan. `None` when the job cannot be queued.
    fn start_scan(
        &mut self,
        scan: ScanId,
        key: ProcessKey,
        opts: StringScanOpts,
    ) -> Option<HelperScan> {
        let (stop_tx, stop) = async_channel::bounded(1);
        lock(&self.store).scans.insert(scan, ScanFeed::default());
        let job = HelperJob::Scan {
            scan,
            key,
            opts,
            stop,
        };
        if self.jobs.try_send(job).is_err() {
            lock(&self.store).scans.remove(&scan);
            return None;
        }
        Some(HelperScan {
            scan,
            key,
            _stop: stop_tx,
            chunks: Vec::new(),
            found: 0,
            last_publish: None,
        })
    }

    /// Forgets helper scan `scan`; its stream stops when the `HelperScan` drops.
    fn drop_scan(&self, scan: ScanId) {
        lock(&self.store).scans.remove(&scan);
    }
}

/// One Strings scan that runs through the helper.
struct HelperScan {
    scan: ScanId,
    key: ProcessKey,
    /// Dropping it closes the job's `stop`, which drops the helper stream.
    _stop: async_channel::Sender<()>,
    /// One chunk per batch that found strings. Every payload shares these `Arc`s.
    chunks: Vec<Arc<[StringHit]>>,
    found: u32,
    /// Clock time of the last `Scanning` payload.
    last_publish: Option<TimeNs>,
}

impl HelperScan {
    fn update(&self, phase: ScanPhase, validity: Validity) -> SourceUpdate {
        let progress = if phase == ScanPhase::Finished { 100 } else { 0 };
        strings_update(
            self.key,
            self.scan,
            progress,
            self.found,
            &self.chunks,
            phase,
            validity,
        )
    }
}

/// The helper side of the detail reads: runs the queued `ReadProcessFiles` and Strings scans one
/// at a time and stores their replies for the next sample.
pub struct HelperDetailReads {
    handle: HelperHandle,
    jobs: async_channel::Receiver<HelperJob>,
    store: Arc<Mutex<HelperStore>>,
    /// When the last read of each slot returned, for `HELPER_PERIOD`.
    returned: FxHashMap<HelperSlot, Time>,
}

impl HelperDetailReads {
    /// Runs until `cx` is cancelled or the sample side is dropped.
    pub async fn run(mut self, cx: &Cx) {
        while let Ok(job) = self.jobs.recv().await {
            if cx.checkpoint().is_err() {
                return;
            }
            match job {
                HelperJob::Read { slot, files } => {
                    if self.read(cx, slot, files).await.is_break() {
                        return;
                    }
                }
                HelperJob::Scan {
                    scan,
                    key,
                    opts,
                    stop,
                } => {
                    // A scan cancelled or replaced while it waited is never sent.
                    if !lock(&self.store).scans.contains_key(&scan) {
                        continue;
                    }
                    let req = HelperRequest::ScanStrings { key, opts };
                    let end = match self.handle.read_stream(cx, req, HELPER_SCAN_DEADLINE).await {
                        Ok(stream) => self.feed(cx, scan, stream, &stop).await,
                        Err(HelperReadError::NotRunning) => Some(HelperScanEnd::Restricted),
                        Err(_) => Some(HelperScanEnd::Failed),
                    };
                    if let Some(end) = end
                        && let Some(feed) = lock(&self.store).scans.get_mut(&scan)
                    {
                        feed.end = Some(end);
                    }
                }
            }
        }
    }

    /// Sends one queued read, at least `HELPER_PERIOD` after the last read of `slot` returned, and
    /// only while `slot` is still demanded. `Break` when `cx` was cancelled.
    async fn read(&mut self, cx: &Cx, slot: HelperSlot, files: ProcFiles) -> ControlFlow<()> {
        let now = cx.now();
        self.returned
            .retain(|_, at| now.as_nanos().saturating_sub(at.as_nanos()) < nanos(HELPER_PERIOD));
        if let Some(at) = self.returned.get(&slot) {
            let wait = nanos(HELPER_PERIOD) - now.as_nanos().saturating_sub(at.as_nanos());
            time::sleep(now, Duration::from_nanos(wait)).await;
            if cx.checkpoint().is_err() {
                return ControlFlow::Break(());
            }
        }
        let result = if lock(&self.store).wanted.contains(slot) {
            let req = HelperRequest::ReadProcessFiles {
                key: slot.key(),
                files,
            };
            // `read` never starts a session: a helper that exited gives `NotRunning`, never a
            // polkit prompt.
            let result = self.handle.read(cx, req, HELPER_DEADLINE).await;
            if matches!(
                result,
                Err(HelperReadError::Call(HelperCallError::Cancelled))
            ) {
                return ControlFlow::Break(());
            }
            self.returned.insert(slot, cx.now());
            Some(result)
        } else {
            None
        };
        lock(&self.store).reads.push(HelperDone {
            slot,
            files,
            result,
        });
        ControlFlow::Continue(())
    }

    /// Moves the batches of `stream` into the feed of `scan` until its end. `None` when `stop`
    /// closed first: `stream` drops and the helper gets `CancelScan`.
    async fn feed(
        &self,
        cx: &Cx,
        scan: ScanId,
        stream: HelperStream,
        stop: &async_channel::Receiver<()>,
    ) -> Option<HelperScanEnd> {
        loop {
            // `stop` goes first, so a scan dropped while replies wait reads none of them.
            let next = futures_lite::future::or(
                async {
                    let _ = stop.recv().await;
                    None
                },
                async { Some(stream.replies.recv().await) },
            )
            .await;
            let reply = match next? {
                Ok(reply) => reply,
                // Closed without a final reply.
                Err(_) => {
                    return Some(match stream.result(cx).await {
                        Err(HelperReadError::NotRunning) => HelperScanEnd::Restricted,
                        _ => HelperScanEnd::Failed,
                    });
                }
            };
            let end = match reply.body {
                HelperReplyBody::Strings(batch) => {
                    if let Some(feed) = lock(&self.store).scans.get_mut(&scan) {
                        feed.hits.extend(batch.into_iter().map(helper_hit));
                    }
                    reply.last.then_some(HelperScanEnd::Finished)
                }
                HelperReplyBody::Err(e) => Some(match e {
                    HelperError::TooLarge => HelperScanEnd::Limit,
                    HelperError::NoSuchProcess | HelperError::IdentityChanged => {
                        HelperScanEnd::Exited
                    }
                    HelperError::PermissionDenied => HelperScanEnd::Restricted,
                    _ => HelperScanEnd::Failed,
                }),
                _ => reply.last.then_some(HelperScanEnd::Failed),
            };
            if end.is_some() {
                return end;
            }
        }
    }
}

/// A found string from the helper. `length` counts the characters of the text as sent.
fn helper_hit((address, encoding, value): (u64, StringEncoding, String)) -> StringHit {
    StringHit {
        address,
        encoding,
        length: u32::try_from(value.chars().count()).unwrap_or(u32::MAX),
        value: Arc::from(value),
    }
}

/// True while a PSS or USS column is shown.
pub fn wants_pss(d: &Demand) -> bool {
    d.process_columns.contains(ColumnId::Pss) || d.process_columns.contains(ColumnId::Uss)
}

/// The tabs read from demand. General is also read for every process target whatever its tab.
/// Strings is never read from demand, the reducer builds AppGeneral, and AppProcesses has no
/// payload.
fn reads_from_demand(tab: DetailTab) -> bool {
    matches!(
        tab,
        DetailTab::General
            | DetailTab::Threads
            | DetailTab::Fds
            | DetailTab::Maps
            | DetailTab::Security
            | DetailTab::Environment
    )
}

/// True when `details` holds a process target: its General tab is always read.
fn wants_tabs(details: &[DetailDemand]) -> bool {
    details
        .iter()
        .any(|d| matches!(d.target, DetailTarget::Process(_)))
}

/// `processes.interval_ms`, clamped as the Processes worker clamps it.
fn tab_interval(s: &EngineSettings) -> Duration {
    Duration::from_millis(u64::from(
        s.interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS),
    ))
}

/// Samples per tab read when PSS and tabs both run: the fewest equal parts of the interval
/// that are at most `PSS_PERIOD` long.
fn shared_parts(interval: Duration) -> u32 {
    let parts = interval.as_nanos().div_ceil(PSS_PERIOD.as_nanos()).max(1);
    u32::try_from(parts).unwrap_or(u32::MAX)
}

/// The period when affinity runs beside tabs: the greatest common divisor of the interval and
/// `AFFINITY_PERIOD`, at least `MIN_INTERVAL_MS`. Returns the period, the samples per tab read
/// and the samples per affinity read. Both values the period can take divide 1 s, so affinity
/// reads at exactly 1 Hz. Tabs read exactly at the interval when it is a multiple of the period,
/// as every menu value is. A hand-edited interval that is not gets the nearest whole number of
/// samples, at least 1: 1600 ms gives 6 samples of 250 ms.
fn affinity_shared_period(interval: Duration) -> (Duration, u32, u32) {
    let interval_ms = interval.as_millis();
    let affinity_ms = AFFINITY_PERIOD.as_millis();
    let (mut a, mut b) = (interval_ms, affinity_ms);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let period_ms = a.max(u128::from(MIN_INTERVAL_MS));
    let count = |n: u128| u32::try_from(n.max(1)).unwrap_or(u32::MAX);
    (
        Duration::from_millis(u64::try_from(period_ms).unwrap_or(u64::MAX)),
        count((interval_ms + period_ms / 2) / period_ms),
        count(affinity_ms / period_ms),
    )
}

/// `d` in nanoseconds.
fn nanos(d: Duration) -> TimeNs {
    TimeNs::try_from(d.as_nanos()).unwrap_or(TimeNs::MAX)
}

/// True when no read happened at `last`, or `min` has passed since it.
fn elapsed(last: Option<TimeNs>, now: TimeNs, min: Duration) -> bool {
    last.is_none_or(|at| now.saturating_sub(at) >= nanos(min))
}

/// `elapsed` for a read every `every` while a scan runs. Half a `SCAN_PERIOD` is taken off, so
/// a sample a little early or late still reads and the rate does not drift.
fn scan_elapsed(last: Option<TimeNs>, now: TimeNs, every: Duration) -> bool {
    elapsed(last, now, every.saturating_sub(SCAN_PERIOD / 2))
}

/// A read that runs every `stride` samples. `ready` is true on the first sample and then on
/// every `stride`-th sample.
#[derive(Clone, Copy, Debug, Default)]
struct Stride {
    every: u32,
    wait: u32,
}

impl Stride {
    /// A new stride starts at once.
    fn set(&mut self, every: u32) {
        if every != self.every {
            *self = Stride { every, wait: 0 };
        }
    }

    fn ready(&mut self) -> bool {
        if self.wait == 0 {
            self.wait = self.every.saturating_sub(1);
            true
        } else {
            self.wait -= 1;
            false
        }
    }
}

/// One tab's reader output, kept to detect a change. The tl-model payloads have no
/// `PartialEq`, and the `Debug` of the environment hides the values.
#[derive(PartialEq)]
enum TabRead {
    General(Box<GeneralRead>),
    Threads(Vec<ThreadInfo>),
    Fds(Vec<FdInfo>),
    Maps {
        regions: Vec<MapRegionInfo>,
        nodes: Vec<MapNodeInfo>,
    },
    Security(Box<SecurityInfo>),
    Environ {
        info: EnvInfo,
        read: EnvRead,
    },
}

impl TabRead {
    fn validity(&self) -> Validity {
        match self {
            TabRead::Environ {
                read: EnvRead::Failed(_),
                ..
            } => Validity::Failed,
            _ => Validity::Valid,
        }
    }
}

/// The General tab values of one process, compared to detect a change. User names are looked
/// up when the payload is built. GPU fields are not read here, `package` is set from the package
/// index after the read, and `hashes` is the target's last `ComputeHashes` result.
#[derive(Clone, PartialEq)]
struct GeneralRead {
    started_s: Total<i64>,
    /// Real, effective, saved and filesystem uid.
    uids: Total<[u32; 4]>,
    policy: SchedPolicy,
    rt_prio: u8,
    kernel_prio: Total<i32>,
    autogroup: Total<(Arc<str>, i8)>,
    affinity: Total<CpuSet>,
    affinity_threads_differ: bool,
    pie: Total<bool>,
    nx_stack: Total<bool>,
    relro: Total<Relro>,
    shadow_stack: Total<bool>,
    aslr_system: Total<u8>,
    aslr_disabled_for_process: Total<bool>,
    no_new_privs: Total<bool>,
    seccomp: Total<Seccomp>,
    architecture: Total<ElfArch>,
    /// The sanitized link text, with the kernel's " (deleted)" suffix kept.
    exe: Total<Arc<str>>,
    exe_deleted: bool,
    /// `statx` mtime of the file `exe` points to, the package index key with the path.
    exe_mtime_s: Option<i64>,
    location: Total<ExeLocation>,
    /// `Valid(None)` is "Not packaged", `WarmingUp` while the index has not looked the path up.
    package: Total<Option<PackageInfo>>,
    cwd: Total<Arc<str>>,
    root: Total<Arc<str>>,
    cgroup: Total<Option<Arc<str>>>,
    unit: Total<Option<UnitRef>>,
    /// The matched desktop id, set only when the process table would show it as an App.
    app_id: Option<Arc<str>>,
    cmdline: Total<Arc<str>>,
    handles: Total<u32>,
    rss_shared_bytes: Total<u64>,
    commit_bytes: Total<u64>,
    cpu_time_ms: Total<u64>,
    io: Total<PidIo>,
    hashes: HashState,
    /// The last `VerifyPackage` result, `NotVerified` before one.
    package_check: PackageCheck,
}

/// What the General reader keeps between reads.
#[derive(Default)]
struct GeneralReader {
    /// `btime` from `/proc/stat`. It does not change within a boot, so it is read once.
    btime: Option<u64>,
    /// The tasklens user's directories, made at the first read. Tests set it.
    location: Option<LocationCtx>,
    /// The desktop entries the App ID is matched against, made at the first read. Tests set it.
    apps: Option<AppIndex>,
    /// `getuid()`, for the same App classification as the process table. Tests set it.
    self_uid: u32,
    stat: PidStat,
    status: PidStatus,
    /// The `status` of one thread, for the affinity check.
    thread_status: PidStatus,
    cpus: SmallVec<[u16; 64]>,
    /// The sanitized first command-line token, for the Wine step of the app match.
    argv0: String,
}

/// The desktop index for the App ID row and the watch that rebuilds it when desktop files
/// change, the same as the process sweep's `AppResolver`.
struct AppIndex {
    index: DesktopIndex,
    watch: Option<DesktopWatch>,
}

/// The state of one tab of one target. Made when the tab enters the demand, dropped when it
/// leaves.
#[derive(Default)]
struct TabState {
    /// The reader output behind the last payload sent. `None` after a read without data.
    last: Option<TabRead>,
    /// The last payload sent and its validity. `None` until the first read.
    published: Option<(Arc<TabPayload>, Validity)>,
    /// Environment: read once; later reads only on `DetailRefresh`.
    env_loaded: bool,
    /// Set when `period` made this state because it saw the tab appear. The reducer's
    /// `DetailOpened` for this open then only resends the payload.
    open_pending: bool,
    /// The last payload came from the helper. A Restricted direct read then keeps it.
    via_helper: bool,
}

/// Everything kept for one demanded process. Dropped when the target leaves the demand.
#[derive(Default)]
struct TargetState {
    /// Indexed by `DetailTab::index()`. `None` while the tab is not demanded.
    tabs: [Option<TabState>; 9],
    /// The Threads tab CPU baselines.
    threads: Option<ThreadReader>,
    /// Memory maps: RSS by region start from the last `smaps` read.
    rss: FxHashMap<u64, u64>,
    /// File descriptors: protocol by socket inode. A socket keeps its protocol.
    sockets: FxHashMap<u64, FdType>,
    /// The last `ComputeHashes` result, sent in the General payload.
    hashes: HashState,
    /// The last `VerifyPackage` result, sent in the General payload.
    package_check: PackageCheck,
    /// The General values of the last helper read, set on the Restricted fields of each General
    /// read while the helper is `Unlocked`.
    general_patch: Option<GeneralPatch>,
}

/// General values read through the helper (PROC-078). Each field is `None` when it was not
/// read, or the value or the validity of the helper's file: Restricted (EACCES, EPERM), Missing
/// (ENOENT, ESRCH) or Failed.
#[derive(Clone, Default)]
struct GeneralPatch {
    exe: Option<Result<HelperExe, Validity>>,
    cwd: Option<Result<Arc<str>, Validity>>,
    root: Option<Result<Arc<str>, Validity>>,
    io: Option<Result<PidIo, Validity>>,
    status: Option<Result<StatusFields, Validity>>,
}

/// The General fields that come from `status`, built from the helper's `STATUS` as
/// `read_general` builds them. The thread affinity check and the App ID need more than the
/// leader's `status` and stay as the direct read left them.
#[derive(Clone, Copy)]
struct StatusFields {
    uids: Total<[u32; 4]>,
    affinity: Total<CpuSet>,
    shadow_stack: Total<bool>,
    no_new_privs: Total<bool>,
    seccomp: Total<Seccomp>,
    rss_shared_bytes: Total<u64>,
    commit_bytes: Total<u64>,
}

impl StatusFields {
    fn new(status: &PidStatus, cpus: &mut SmallVec<[u16; 64]>) -> StatusFields {
        let bytes = |value: Option<u64>| value.map_or(total(0, Validity::Unsupported), valid);
        StatusFields {
            uids: valid(status.uid),
            affinity: match cpu_set(&status.cpus_allowed_list, cpus) {
                Some(set) => valid(set),
                None => total(CpuSet::empty(), Validity::Unsupported),
            },
            shadow_stack: match rd::shadow_stack(status.x86_thread_features.as_deref()) {
                ShadowStack::Enabled => valid(true),
                ShadowStack::Disabled => valid(false),
                ShadowStack::NotSupported => total(false, Validity::Unsupported),
            },
            no_new_privs: match status.no_new_privs {
                Some(on) => valid(on),
                None => total(false, Validity::Unsupported),
            },
            seccomp: seccomp_total(status.seccomp, status.seccomp_filters),
            rss_shared_bytes: bytes(ws_shared_bytes(status)),
            commit_bytes: bytes(commit_bytes(status)),
        }
    }

    /// Sets these fields on `read`, whose `status` the direct read could not read.
    fn apply(&self, read: &mut GeneralRead) {
        read.uids = self.uids;
        read.affinity = self.affinity;
        read.shadow_stack = self.shadow_stack;
        read.no_new_privs = self.no_new_privs;
        read.seccomp = self.seccomp;
        read.rss_shared_bytes = self.rss_shared_bytes;
        read.commit_bytes = self.commit_bytes;
    }
}

/// The executable from the helper: sanitized link text, deleted, location.
type HelperExe = (Arc<str>, bool, ExeLocation);

/// Sets `value` or its validity on `field` when the direct read left it Restricted.
fn patch_field<T: Clone>(field: &mut Total<T>, value: &Option<Result<T, Validity>>) {
    if field.validity != Validity::Restricted {
        return;
    }
    match value {
        Some(Ok(value)) => *field = valid(value.clone()),
        Some(Err(validity)) => field.validity = *validity,
        None => {}
    }
}

impl GeneralPatch {
    /// Sets the values, or the helper's validity, on the fields of `read` that are Restricted.
    fn apply(&self, read: &mut GeneralRead) {
        if read.exe.validity == Validity::Restricted {
            match &self.exe {
                Some(Ok((exe, deleted, location))) => {
                    read.exe = valid(Arc::clone(exe));
                    read.exe_deleted = *deleted;
                    read.location = valid(*location);
                }
                Some(Err(validity)) => {
                    read.exe.validity = *validity;
                    read.location.validity = *validity;
                }
                None => {}
            }
        }
        patch_field(&mut read.cwd, &self.cwd);
        patch_field(&mut read.root, &self.root);
        patch_field(&mut read.io, &self.io);
        // `uids` comes only from `status`, so it is Restricted exactly when `status` was.
        if read.uids.validity == Validity::Restricted {
            match &self.status {
                Some(Ok(fields)) => fields.apply(read),
                Some(Err(validity)) => {
                    for v in [
                        &mut read.uids.validity,
                        &mut read.affinity.validity,
                        &mut read.shadow_stack.validity,
                        &mut read.no_new_privs.validity,
                        &mut read.seccomp.validity,
                        &mut read.rss_shared_bytes.validity,
                        &mut read.commit_bytes.validity,
                    ] {
                        *v = *validity;
                    }
                }
                None => {}
            }
        }
    }
}

/// The helper files for the Restricted fields of a General read.
fn general_helper_files(read: &GeneralRead) -> ProcFiles {
    let mut files = ProcFiles::empty();
    for (validity, file) in [
        (read.exe.validity, ProcFiles::EXE),
        (read.cwd.validity, ProcFiles::CWD),
        (read.root.validity, ProcFiles::ROOT),
        (read.io.validity, ProcFiles::IO),
        (read.uids.validity, ProcFiles::STATUS),
    ] {
        if validity == Validity::Restricted {
            files |= file;
        }
    }
    files
}

/// The helper files of a tab whose direct read gave `Restricted`. File descriptors and Security
/// have no byte parsers yet (tasklens-qfx.39) and Threads has no helper file.
fn tab_helper_files(tab: DetailTab) -> Option<ProcFiles> {
    match tab {
        DetailTab::Environment => Some(ProcFiles::ENVIRON),
        DetailTab::Maps => Some(ProcFiles::MAPS | ProcFiles::SMAPS),
        _ => None,
    }
}

/// One running Strings scan.
struct ScanState {
    scan: ScanId,
    key: ProcessKey,
    scanner: StringScanner,
    mem: ProcMem,
    /// One chunk per step that found strings. Every payload shares these `Arc`s.
    chunks: Vec<Arc<[StringHit]>>,
    /// Clock time of the last `Scanning` payload.
    last_publish: Option<TimeNs>,
}

impl ScanState {
    fn update(&self, phase: ScanPhase, validity: Validity) -> SourceUpdate {
        strings_update(
            self.key,
            self.scan,
            self.scanner.progress_pct(),
            self.scanner.found(),
            &self.chunks,
            phase,
            validity,
        )
    }
}

/// A `HandleSearch` command the next sample starts.
struct SearchRequest {
    id: SearchId,
    pattern: Arc<str>,
    include_maps: bool,
    privileged: bool,
    /// Set by a `CancelHandleSearch` that came before the start.
    cancelled: bool,
}

pub struct ProcessDetailSource {
    cfg: Arc<EngineConfig>,
    pss: PssReader,
    /// Set by `period`: the keys of the latest demand.
    keys: RefCell<Arc<[ProcessKey]>>,
    /// Reused every sample.
    buf: Vec<u8>,
    reads: Vec<(ProcessKey, RollupRead)>,
    /// The newest `HandleSearch` not started yet. The next sample starts it in place of
    /// `search`.
    search_request: Option<SearchRequest>,
    /// The running handle search, dropped after its last state is queued.
    search: Option<HandleSearch>,
    /// `StringScan` commands not started yet, oldest first. The next sample starts them.
    scan_requests: Vec<(ScanId, ProcessKey, StringScanOpts)>,
    /// Running Strings scans, oldest first. With `scan_requests` at most `DETAILS_MAX`, and at
    /// most one per process.
    scans: Vec<ScanState>,
    /// Index in `scans` of the scan that reads first in the next sample, so with more scans than
    /// `SCAN_STEPS_PER_SAMPLE` each one still gets its turn.
    scan_next: usize,
    /// The chunk buffer and hit list of the scans. Freed when no scan runs.
    scan_buf: Vec<u8>,
    scan_hits: Vec<StringHitInfo>,
    /// Set by `period`: true while a Strings scan or a handle search runs or waits, so tabs and
    /// affinity read by the clock.
    scanning: Cell<bool>,
    /// Set by `period`: `processes.interval_ms`.
    interval: Cell<Duration>,
    /// Clock time of the last periodic tab read and of the last affinity read.
    last_tabs: Option<TimeNs>,
    last_affinity: Option<TimeNs>,
    /// `ComputeHashes` keys, oldest first, at most `DETAILS_MAX`. A key waits here until its
    /// target has a General tab state.
    hash_requests: Vec<ProcessKey>,
    hash_cache: HashCache,
    /// `VerifyPackage` keys, oldest first, at most `DETAILS_MAX`. A key waits here until its
    /// target has a General read whose package is known.
    verify_requests: Vec<ProcessKey>,
    /// Set by `period`: the first `AFFINITY_TARGETS_MAX` entries of `Demand.affinity_targets`.
    affinity_keys: RefCell<Arc<[ProcessKey]>>,
    /// Set by `period`: samples per affinity read.
    affinity_every: Cell<u32>,
    /// Reset while no dialog is open, so a new one reads at once.
    affinity_stride: Stride,
    /// Set by `period`: the first `DETAILS_MAX` entries of `Demand.details`.
    details: RefCell<Arc<[DetailDemand]>>,
    /// Set by `period`: samples per tab read.
    tabs_every: Cell<u32>,
    tabs_stride: Stride,
    /// Clock time of the last PSS read. Kept across period changes.
    last_pss: Option<TimeNs>,
    /// Clock time of the last `smaps` read per process. Kept for `SMAPS_MIN_PERIOD`, also when
    /// the tab or the target leaves the demand.
    smaps_at: FxHashMap<ProcessKey, TimeNs>,
    /// True while the demand holds more than `DETAILS_MAX` targets, so the warning is logged
    /// once per overflow.
    over_cap: Cell<bool>,
    cap_warnings: Cell<u32>,
    /// `DetailRefresh` (false) and `DetailOpened` (true) commands for the next sample.
    refresh: Vec<(DetailTarget, DetailTab, bool)>,
    /// Pruned by `period`, which runs on every loop turn, also while no sample is due.
    targets: RefCell<FxHashMap<ProcessKey, TargetState>>,
    /// Tab updates for `take_extra`.
    extra: VecDeque<SourceUpdate>,
    bufs: ReadBufs,
    general: GeneralReader,
    /// `<root>/etc/passwd`, loaded when the first General or Security payload is built.
    users: Option<PasswdUserNames>,
    group_file: Vec<u8>,
    ticks_per_sec: u64,
    online_cpus: u32,
    /// Test hook for the Threads CPU clock.
    thread_clock: Option<fn() -> (TimeNs, u64)>,
    /// Reader calls per tab and `smaps` reads, for tests.
    tab_reads: [u32; 9],
    smaps_reads: u32,
    /// The session's shared package index. `None` when no backend is set up.
    pkg: Option<PackageIndex>,
    /// Set by `helper_reads` when the session has a helper.
    helper: Option<HelperSide>,
    /// Strings scans that run through the helper, oldest first.
    helper_scans: Vec<HelperScan>,
    /// The sample clock, CLOCK_BOOTTIME ns. Tests set it.
    clock: fn() -> TimeNs,
}

/// Only in full mode: the low-memory build has no detail panels (ADR-004).
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::ProcessDetail,
        name: "process_detail",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// `ProcessDetailSource::new` does no I/O, so it is built in place.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let cfg = cfg.clone();
        let src = ProcessDetailSource::new(cfg).with_package_index(io.shared.pkg.clone());
        async move { run(cx, io, src).await }
    })
}

/// `source::run` for the detail reads, with `HelperDetailReads` beside it when the session has a
/// helper.
pub async fn run(cx: Cx, io: WorkerIo, mut src: ProcessDetailSource) -> Outcome<(), WorkerError> {
    let Some(handle) = io.shared.helper.clone() else {
        return source::run(cx, io, src).await;
    };
    let reads = src.helper_reads(handle);
    let side_cx = cx.clone();
    let samples = source::run(cx, io, src);
    let side = async move {
        reads.run(&side_cx).await;
        // Only a cancel ends the side; the sample loop returns for it.
        std::future::pending().await
    };
    futures_lite::future::or(samples, side).await
}

impl ProcessDetailSource {
    pub fn new(cfg: Arc<EngineConfig>) -> Self {
        Self {
            cfg,
            pss: PssReader::new(),
            keys: RefCell::new(Arc::from([])),
            buf: Vec::new(),
            reads: Vec::new(),
            search_request: None,
            search: None,
            scan_requests: Vec::new(),
            scans: Vec::new(),
            scan_next: 0,
            scan_buf: Vec::new(),
            scan_hits: Vec::new(),
            scanning: Cell::new(false),
            interval: Cell::new(Duration::from_millis(u64::from(MIN_INTERVAL_MS))),
            last_tabs: None,
            last_affinity: None,
            hash_requests: Vec::new(),
            hash_cache: HashCache::new(),
            verify_requests: Vec::new(),
            affinity_keys: RefCell::new(Arc::from([])),
            affinity_every: Cell::new(1),
            affinity_stride: Stride::default(),
            details: RefCell::new(Arc::from([])),
            tabs_every: Cell::new(1),
            tabs_stride: Stride::default(),
            last_pss: None,
            smaps_at: FxHashMap::default(),
            over_cap: Cell::new(false),
            cap_warnings: Cell::new(0),
            refresh: Vec::new(),
            targets: RefCell::default(),
            extra: VecDeque::new(),
            bufs: ReadBufs::default(),
            general: GeneralReader {
                self_uid: rustix::process::getuid().as_raw(),
                ..GeneralReader::default()
            },
            users: None,
            group_file: Vec::new(),
            ticks_per_sec: rustix::param::clock_ticks_per_second(),
            online_cpus: 1,
            thread_clock: None,
            tab_reads: [0; 9],
            smaps_reads: 0,
            pkg: None,
            helper: None,
            helper_scans: Vec::new(),
            clock: now_boot_ns,
        }
    }

    /// Samples with `clock` (CLOCK_BOOTTIME ns), for tests.
    pub fn with_clock(mut self, clock: fn() -> TimeNs) -> Self {
        self.clock = clock;
        self
    }

    /// Connects the reads to `handle`. Run the returned `HelperDetailReads` next to
    /// `source::run`, as `run` does.
    pub fn helper_reads(&mut self, handle: HelperHandle) -> HelperDetailReads {
        let store = Arc::new(Mutex::new(HelperStore::default()));
        let (jobs_tx, jobs) = async_channel::unbounded();
        self.helper = Some(HelperSide {
            handle: handle.clone(),
            jobs: jobs_tx,
            store: Arc::clone(&store),
            pending: FxHashSet::default(),
            last: FxHashMap::default(),
            pss_cursor: None,
            pss: FxHashMap::default(),
        });
        HelperDetailReads {
            handle,
            jobs,
            store,
            returned: FxHashMap::default(),
        }
    }

    /// General reads look packages up in `index`, the session's shared index.
    pub fn with_package_index(mut self, index: Option<PackageIndex>) -> Self {
        self.pkg = index;
        self
    }

    /// Number of cached `smaps_rollup` fds (at most 128).
    pub fn cached_fds(&self) -> usize {
        self.pss.cached()
    }

    /// Reader calls for `tab` since the source was made.
    pub fn tab_reads(&self, tab: DetailTab) -> u32 {
        self.tab_reads[tab.index()]
    }

    /// `smaps` reads of the Memory maps tab since the source was made.
    pub fn smaps_reads(&self) -> u32 {
        self.smaps_reads
    }

    /// Process targets with tab state.
    pub fn targets_len(&self) -> usize {
        self.targets.borrow().len()
    }
    /// `sample()` is `self.sample_at(now_boot_ns())`; tests call this with an injected clock.
    /// `now` stamps the PSS and health updates and times the PSS and `smaps` limits.
    pub fn sample_at(mut self, now: TimeNs) -> (Self, SourceUpdate) {
        self.tabs_stride.set(self.tabs_every.get());
        self.smaps_at
            .retain(|_, &mut at| !elapsed(Some(at), now, SMAPS_MIN_PERIOD));
        self.apply_helper_reads(now);
        self.read_tabs(now);
        self.read_affinity(now);
        self.step_scans(now);
        self.step_helper_scans(now);
        self.step_search();

        let keys = self.keys.borrow().clone();
        // With no keys the reader still runs while fds are cached, so hidden columns close them.
        let pss_due = if keys.is_empty() {
            self.pss.cached() > 0
        } else if elapsed(self.last_pss, now, PSS_PERIOD) {
            self.last_pss = Some(now);
            true
        } else {
            false
        };
        if !pss_due {
            let update = self.extra.pop_back().unwrap_or(SourceUpdate::Health {
                id: SourceId::ProcessDetail,
                state: SourceState::Running,
                last_error: None,
                t_ns: now,
            });
            return (self, update);
        }
        self.pss
            .read(&self.cfg.sys_root, &keys, &mut self.buf, &mut self.reads);
        if let Some(side) = &mut self.helper {
            side.pss_round(&self.reads, now);
        }
        let helper_pss = self.helper.as_ref().map(|side| &side.pss);
        let entries = self
            .reads
            .iter()
            .filter_map(|&(key, read)| {
                let (pss, uss, swap, validity) = match read {
                    RollupRead::Ok(r) => (r.pss, r.uss, r.swap, Validity::Valid),
                    RollupRead::Restricted => match helper_pss.and_then(|pss| pss.get(&key)) {
                        Some(HelperPss::Ok(r)) => (r.pss, r.uss, r.swap, Validity::Valid),
                        Some(HelperPss::Gone) => return None,
                        Some(HelperPss::Invalid(validity)) => (0, 0, 0, *validity),
                        None => (0, 0, 0, Validity::Restricted),
                    },
                    RollupRead::Failed => (0, 0, 0, Validity::Failed),
                    RollupRead::Gone => return None,
                };
                Some(PssEntry {
                    key,
                    pss_bytes: pss,
                    uss_bytes: uss,
                    swap_bytes: swap,
                    validity,
                })
            })
            .collect();
        let update = SourceUpdate::Details(DetailsUpdate::Pss { t_ns: now, entries });
        (self, update)
    }

    /// The first `DETAILS_MAX` entries of `d.details`. A longer list is logged once, until the
    /// demand is back under the cap.
    fn cap_details(&self, d: &Demand) -> Arc<[DetailDemand]> {
        if d.details.len() <= DETAILS_MAX {
            self.over_cap.set(false);
            return d.details.clone();
        }
        if !self.over_cap.replace(true) {
            self.cap_warnings.set(self.cap_warnings.get() + 1);
            tracing::warn!(
                demanded = d.details.len(),
                served = DETAILS_MAX,
                "too many Properties targets, only the first ones are read"
            );
        }
        Arc::from(&d.details[..DETAILS_MAX])
    }

    /// Applies the `DetailRefresh` and `DetailOpened` commands, then reads the demanded tabs
    /// that are due and queues the changed ones in `extra`.
    fn read_tabs(&mut self, now: TimeNs) {
        for (target, tab, opened) in std::mem::take(&mut self.refresh) {
            let DetailTarget::Process(key) = target else {
                continue;
            };
            if opened {
                // The reducer may have dropped the General payload with its target. General is
                // read at every interval anyway, so a notice only sends it again.
                let general = self
                    .targets
                    .get_mut()
                    .get(&key)
                    .and_then(|t| t.tabs[DetailTab::General.index()].as_ref())
                    .and_then(|s| s.published.as_ref());
                if let Some((payload, validity)) = general {
                    self.extra
                        .push_back(SourceUpdate::Details(DetailsUpdate::Tab {
                            target,
                            tab: DetailTab::General,
                            payload: Arc::clone(payload),
                            validity: *validity,
                        }));
                }
                if tab == DetailTab::General {
                    continue;
                }
            }
            if !reads_from_demand(tab) {
                continue;
            }
            let slot = &mut self.targets.get_mut().entry(key).or_default().tabs[tab.index()];
            match slot {
                // The reducer's notice for an open this worker saw. The tab was read for that
                // open, so its payload is only sent again.
                Some(state) if opened && state.open_pending => {
                    state.open_pending = false;
                    if let Some((payload, validity)) = &state.published {
                        self.extra
                            .push_back(SourceUpdate::Details(DetailsUpdate::Tab {
                                target,
                                tab,
                                payload: Arc::clone(payload),
                                validity: *validity,
                            }));
                    }
                }
                // A user refresh, or a tab that closed and opened again between two polls. The
                // reducer may have dropped the payload, so the tab is read and sent again. A user
                // refresh keeps `open_pending`, so the notice for this open that comes after it
                // only resends instead of reading a third time.
                Some(state) => {
                    *state = TabState {
                        open_pending: !opened && state.open_pending,
                        ..TabState::default()
                    }
                }
                // The tab is not in the demand this worker saw yet. Its first read serves this
                // command, so the state is made without `open_pending`.
                None => *slot = Some(TabState::default()),
            }
        }

        // Hash requests whose target is demanded. Their General tab is read below to send the
        // result.
        let mut hashed: SmallVec<[ProcessKey; DETAILS_MAX]> = SmallVec::new();
        for key in std::mem::take(&mut self.hash_requests) {
            let demanded = self
                .targets
                .get_mut()
                .get(&key)
                .is_some_and(|t| t.tabs[DetailTab::General.index()].is_some());
            if !demanded {
                self.hash_requests.push(key);
                continue;
            }
            let hashes = self.compute_hashes(key);
            if let Some(target) = self.targets.get_mut().get_mut(&key) {
                target.hashes = hashes;
            }
            hashed.push(key);
        }

        // Verify requests whose target has a General read with a settled package. A package
        // still warming up waits; no package drops the request and leaves the check as is.
        for key in std::mem::take(&mut self.verify_requests) {
            let owner = self
                .targets
                .get_mut()
                .get(&key)
                .and_then(|t| t.tabs[DetailTab::General.index()].as_ref())
                .and_then(|s| match s.last.as_ref() {
                    Some(TabRead::General(read)) => Some(read.package.clone()),
                    _ => None,
                });
            let Some(package) = owner else {
                self.verify_requests.push(key);
                continue;
            };
            if package.validity == Validity::WarmingUp {
                self.verify_requests.push(key);
                continue;
            }
            let (Validity::Valid, Some(info)) = (package.validity, package.value) else {
                continue;
            };
            let check = self.verify_package(&info.name);
            if let Some(target) = self.targets.get_mut().get_mut(&key) {
                target.package_check = check;
            }
            hashed.push(key);
        }

        let details = self.details.borrow().clone();
        let periodic = wants_tabs(&details) && self.tabs_due(now);
        // General first and once per target, then the demanded tab.
        let mut reads: SmallVec<[(ProcessKey, DetailTab); 2 * DETAILS_MAX]> = SmallVec::new();
        for d in details.iter() {
            let DetailTarget::Process(key) = d.target else {
                continue;
            };
            for tab in [DetailTab::General, d.tab] {
                if reads_from_demand(tab) && !reads.contains(&(key, tab)) {
                    reads.push((key, tab));
                }
            }
        }
        for (key, tab) in reads {
            let state = self
                .targets
                .get_mut()
                .get(&key)
                .and_then(|t| t.tabs[tab.index()].as_ref());
            // A tab not read yet is read at once.
            let wanted = match state {
                None => true,
                Some(s) if tab == DetailTab::Environment => !s.env_loaded,
                Some(_) if tab == DetailTab::General && hashed.contains(&key) => true,
                Some(s) => periodic || s.published.is_none(),
            };
            if wanted && let Some(update) = self.read_tab(key, tab, now) {
                self.extra.push_back(SourceUpdate::Details(update));
            }
        }
    }

    /// Reads one tab of `key`. `Some` when the payload or its validity changed.
    fn read_tab(&mut self, key: ProcessKey, tab: DetailTab, now: TimeNs) -> Option<DetailsUpdate> {
        self.tab_reads[tab.index()] += 1;
        if tab == DetailTab::Threads {
            self.online_cpus = read_online_cpus(&self.cfg.sys_root, self.online_cpus);
        }
        if tab == DetailTab::Security
            && read_file(
                &root_base(&self.cfg.sys_root).join("etc/group"),
                &mut self.group_file,
                GROUP_FILE_MAX,
            )
            .is_err()
        {
            self.group_file.clear();
        }
        let root = &self.cfg.sys_root;
        let bufs = &mut self.bufs;
        let pkg = self.pkg.as_ref();
        let state = self.targets.get_mut().entry(key).or_default();
        let read = match tab {
            DetailTab::General => read_general(
                root,
                self.cfg.app_paths.as_ref(),
                key,
                self.ticks_per_sec,
                now,
                &mut self.general,
                bufs,
            )
            .map(|mut read| {
                read.hashes = state.hashes.clone();
                read.package_check = state.package_check;
                read.package = package_of(pkg, &read, Instant::now());
                TabRead::General(Box::new(read))
            }),
            DetailTab::Threads => {
                let clock = self.thread_clock;
                let reader = state.threads.get_or_insert_with(|| match clock {
                    Some(c) => ThreadReader::new().with_clock(c),
                    None => ThreadReader::new(),
                });
                let mut rows = Vec::new();
                let read = reader.read(
                    root,
                    key,
                    self.ticks_per_sec,
                    self.online_cpus,
                    bufs,
                    &mut rows,
                );
                detail_result(read, tab).map(|()| TabRead::Threads(rows))
            }
            DetailTab::Fds => {
                let mut rows = Vec::new();
                detail_result(rd::read_fds(root, key, bufs, &mut rows), tab).map(|()| {
                    resolve_sockets(root, key.pid, &mut state.sockets, &mut rows);
                    TabRead::Fds(rows)
                })
            }
            DetailTab::Maps => {
                let with_rss = elapsed(self.smaps_at.get(&key).copied(), now, SMAPS_MIN_PERIOD);
                let mut regions = Vec::new();
                let mut nodes = Vec::new();
                let read = rd::read_maps(root, key, with_rss, bufs, &mut regions, &mut nodes);
                detail_result(read, tab).map(|()| {
                    if with_rss {
                        self.smaps_reads += 1;
                        self.smaps_at.insert(key, now);
                        state.rss = regions
                            .iter()
                            .filter_map(|r| r.rss.map(|rss| (r.start, rss)))
                            .collect();
                    } else {
                        apply_rss(&state.rss, &mut regions, &mut nodes);
                    }
                    TabRead::Maps { regions, nodes }
                })
            }
            DetailTab::Security => {
                let mut info = SecurityInfo::default();
                let read = rd::read_security(root, key, &self.group_file, bufs, &mut info);
                detail_result(read, tab).map(|()| TabRead::Security(Box::new(info)))
            }
            DetailTab::Environment => {
                state.tabs[tab.index()]
                    .get_or_insert_with(TabState::default)
                    .env_loaded = true;
                let mut info = EnvInfo::default();
                match rd::read_environ(root, key, &mut bufs.small, &mut info) {
                    Ok(EnvRead::Restricted) => Err(Validity::Restricted),
                    Ok(EnvRead::Gone) => Err(Validity::Missing),
                    Ok(read) => Ok(TabRead::Environ { info, read }),
                    Err(e) => {
                        tracing::debug!(error = %e, "environment not read");
                        Err(Validity::Failed)
                    }
                }
            }
            _ => return None,
        };

        // While the helper is `Unlocked`, Restricted data is read through it (qfx.18).
        let slot_id = HelperSlot::Tab(key, tab);
        let read = match &mut self.helper {
            Some(side) if side.unlocked() => match read {
                Ok(TabRead::General(mut general)) => {
                    let files = general_helper_files(&general);
                    if !files.is_empty() {
                        side.request(slot_id, files, now);
                    }
                    if let Some(patch) = &state.general_patch {
                        patch.apply(&mut general);
                    }
                    Ok(TabRead::General(general))
                }
                Err(Validity::Restricted) => match tab_helper_files(tab) {
                    Some(files) => {
                        side.request(slot_id, files, now);
                        let via_helper = state.tabs[tab.index()]
                            .as_ref()
                            .is_some_and(|s| s.via_helper);
                        // A helper payload stays, and a tab that waits for the helper sends
                        // nothing until the reply.
                        if via_helper || side.pending.contains(&slot_id) {
                            return None;
                        }
                        Err(Validity::Restricted)
                    }
                    None => Err(Validity::Restricted),
                },
                read => read,
            },
            _ => {
                state.general_patch = None;
                read
            }
        };
        let slot = state.tabs[tab.index()].get_or_insert_with(TabState::default);
        slot.via_helper = false;
        publish_tab(
            slot,
            key,
            tab,
            read,
            &mut self.users,
            &self.group_file,
            root,
        )
    }

    /// Takes the answered helper reads: PSS values for the next PSS round, and tab replies sent
    /// at once.
    fn apply_helper_reads(&mut self, now: TimeNs) {
        let Some(side) = &mut self.helper else {
            return;
        };
        let reads = side.take_reads(now);
        for HelperDone {
            slot,
            files,
            result,
        } in reads
        {
            // A read not sent because its slot left the demand.
            let Some(result) = result else {
                continue;
            };
            match slot {
                HelperSlot::Pss(key) => {
                    let mut too_large = false;
                    let pss = pss_from_helper(files, result, &mut too_large);
                    let Some(side) = &mut self.helper else {
                        return;
                    };
                    match pss {
                        Some(pss) => side.pss.insert(key, pss),
                        None => side.pss.remove(&key),
                    };
                    if too_large {
                        self.helper_too_large(now);
                    }
                }
                HelperSlot::Tab(key, tab) => self.apply_helper_tab(key, tab, files, result, now),
            }
        }
    }

    /// Sends `last_error` "too large to read through the helper".
    fn helper_too_large(&mut self, now: TimeNs) {
        self.extra.push_back(SourceUpdate::Health {
            id: SourceId::ProcessDetail,
            state: SourceState::Running,
            last_error: Some(HELPER_TOO_LARGE.into()),
            t_ns: now,
        });
    }

    /// Sends one tab from a helper reply, parsed with the direct path's parsers. A tab that left
    /// the demand is dropped.
    fn apply_helper_tab(
        &mut self,
        key: ProcessKey,
        tab: DetailTab,
        files: ProcFiles,
        result: HelperResult,
        now: TimeNs,
    ) {
        let Some(target) = self.targets.get_mut().get_mut(&key) else {
            return;
        };
        if target.tabs[tab.index()].is_none() {
            return;
        }
        let mut too_large = false;
        let requested = files;
        let files = match result {
            Ok(replies) => helper_files(requested, &replies, &mut too_large),
            Err(e) => {
                if let HelperReadError::Call(error) = e {
                    tracing::debug!(?tab, ?error, "helper detail read failed");
                }
                Err(Validity::Restricted)
            }
        };
        let read = match tab {
            DetailTab::General => match files {
                Ok(files) => {
                    let ctx = self.general.location.as_ref();
                    let patch = general_patch(requested, &files, ctx, &mut too_large);
                    let last = match target.tabs[tab.index()]
                        .as_ref()
                        .and_then(|s| s.last.as_ref())
                    {
                        Some(TabRead::General(last)) => Some(last.clone()),
                        _ => None,
                    };
                    let applied = last.map(|mut general| {
                        patch.apply(&mut general);
                        general
                    });
                    target.general_patch = Some(patch);
                    // With no direct read to patch yet, the next one gets the patch.
                    applied.map(|general| Ok(TabRead::General(general)))
                }
                // `IdentityChanged` or `NoSuchProcess`: the target exited, the existing exited
                // path.
                Err(Validity::Missing) => Some(Err(Validity::Missing)),
                // The fields stay Restricted.
                Err(_) => {
                    target.general_patch = None;
                    None
                }
            },
            DetailTab::Environment => {
                Some(files.and_then(|files| environ_from_helper(&files, &mut too_large)))
            }
            DetailTab::Maps => {
                let exe = match target.tabs[DetailTab::General.index()]
                    .as_ref()
                    .and_then(|s| s.last.as_ref())
                {
                    Some(TabRead::General(g)) if g.exe.validity == Validity::Valid => {
                        Some(Arc::clone(&g.exe.value))
                    }
                    _ => None,
                };
                Some(
                    files
                        .and_then(|files| maps_from_helper(&files, exe.as_deref(), &mut too_large)),
                )
            }
            _ => None,
        };
        if let Some(read) = read
            && let Some(slot) = target.tabs[tab.index()].as_mut()
        {
            slot.via_helper = read.is_ok();
            let update = publish_tab(
                slot,
                key,
                tab,
                read,
                &mut self.users,
                &self.group_file,
                &self.cfg.sys_root,
            );
            if let Some(update) = update {
                self.extra.push_back(SourceUpdate::Details(update));
            }
        }
        if too_large {
            self.helper_too_large(now);
        }
    }

    /// Runs the PROC-078 integrity check of package `name` on the blocking path.
    fn verify_package(&self, name: &str) -> PackageCheck {
        let Some(index) = &self.pkg else {
            return PackageCheck::NotVerified;
        };
        match index.verify(name) {
            Ok(VerifyOutcome::Unmodified) => PackageCheck::Matches,
            Ok(VerifyOutcome::Modified { .. }) => PackageCheck::DoesNotMatch,
            Ok(VerifyOutcome::Failed(_)) | Err(_) => PackageCheck::Failed,
        }
    }

    /// SHA-256 and MD5 of the executable of `key`, from the cache when the file is unchanged.
    /// The first command-line token stands in when the `exe` link is denied.
    fn compute_hashes(&mut self, key: ProcessKey) -> HashState {
        let root = &self.cfg.sys_root;
        let argv0 = read_argv0(root, key.pid, &mut self.bufs.cmdline);
        match hash_process_exe(root, key, argv0, &mut self.hash_cache, &mut self.buf) {
            Ok(h) => HashState::Done {
                sha256: h.sha256,
                md5: h.md5,
            },
            Err(hash::HashError::ExeUnavailable) => {
                HashState::Failed(HashError::ExePathUnavailable)
            }
            Err(hash::HashError::Read { errno }) => {
                HashState::Failed(HashError::ReadFailed { errno })
            }
        }
    }

    /// Queues `DetailsUpdate::Affinity` for every target of a Set Affinity dialog, once every
    /// `affinity_every` samples.
    fn read_affinity(&mut self, now: TimeNs) {
        let keys = self.affinity_keys.borrow().clone();
        if keys.is_empty() {
            self.affinity_stride = Stride::default();
            self.last_affinity = None;
            return;
        }
        let due = if self.scanning.get() {
            scan_elapsed(self.last_affinity, now, AFFINITY_PERIOD)
        } else {
            self.affinity_stride.set(self.affinity_every.get());
            self.affinity_stride.ready()
        };
        if !due {
            return;
        }
        self.last_affinity = Some(now);
        let g = &mut self.general;
        let entries = keys
            .iter()
            .filter_map(|&key| {
                let info = affinity_info(
                    &self.cfg.sys_root,
                    key,
                    &mut self.bufs.small,
                    &mut g.stat,
                    &mut g.status,
                    &mut g.cpus,
                )?;
                Some((key, info))
            })
            .collect();
        self.extra
            .push_back(SourceUpdate::Details(DetailsUpdate::Affinity {
                t_ns: now,
                entries,
            }));
    }
}

impl ProcessDetailSource {
    /// True when the periodic tab reads are due: every `tabs_every` samples, or by the clock
    /// while a scan runs, as `SCAN_PERIOD` does not divide every interval.
    fn tabs_due(&mut self, now: TimeNs) -> bool {
        let due = if self.scanning.get() {
            scan_elapsed(self.last_tabs, now, self.interval.get())
        } else {
            self.tabs_stride.ready()
        };
        if due {
            self.last_tabs = Some(now);
        }
        due
    }

    /// Starts the requested scans, reads up to `SCAN_STEPS_PER_SAMPLE` chunks over the running
    /// ones in turn and queues their payloads. A scan whose process left the demand stops
    /// without a payload.
    fn step_scans(&mut self, now: TimeNs) {
        let details = self.details.borrow().clone();
        let shown = |key: ProcessKey| {
            details
                .iter()
                .any(|d| d.target == DetailTarget::Process(key))
        };
        self.scan_requests.retain(|&(_, key, _)| shown(key));
        self.scans.retain(|s| shown(s.key));
        for (scan, key, opts) in std::mem::take(&mut self.scan_requests) {
            let started = start_scan(
                &self.cfg.sys_root,
                key,
                opts,
                &mut self.bufs,
                &mut self.general.stat,
            );
            match started {
                Ok((scanner, mem)) => self.scans.push(ScanState {
                    scan,
                    key,
                    scanner,
                    mem,
                    chunks: Vec::new(),
                    last_publish: None,
                }),
                Err((phase, validity)) => {
                    // A target tasklens may not read is scanned through the helper while it is
                    // `Unlocked` (PROC-084).
                    let helper = match &mut self.helper {
                        Some(side) if validity == Validity::Restricted && side.unlocked() => {
                            side.start_scan(scan, key, opts)
                        }
                        _ => None,
                    };
                    match helper {
                        Some(state) => self.helper_scans.push(state),
                        None => self.extra.push_back(strings_update(
                            key,
                            scan,
                            0,
                            0,
                            &[],
                            phase,
                            validity,
                        )),
                    }
                }
            }
        }
        if self.scans.is_empty() {
            self.scan_buf = Vec::new();
            self.scan_hits = Vec::new();
            return;
        }

        let mut ends: SmallVec<[Option<ScanEnd>; DETAILS_MAX]> =
            smallvec::smallvec![None; self.scans.len()];
        let n = self.scans.len();
        let mut i = self.scan_next % n;
        let mut budget = SCAN_STEPS_PER_SAMPLE;
        while budget > 0 && ends.iter().any(Option::is_none) {
            if ends[i].is_none() {
                budget -= 1;
                let state = &mut self.scans[i];
                self.scan_hits.clear();
                ends[i] =
                    state
                        .scanner
                        .step(&mut state.mem, &mut self.scan_buf, &mut self.scan_hits);
                if !self.scan_hits.is_empty() {
                    state
                        .chunks
                        .push(self.scan_hits.drain(..).map(string_hit).collect());
                }
            }
            i = (i + 1) % n;
        }

        for (state, end) in self.scans.iter_mut().zip(ends.iter_mut()) {
            // The memory of a process that exited reads as holes, so without this check its
            // scan would end as Finished or Failed. A limit that was reached stays.
            if *end != Some(ScanEnd::Stopped)
                && scan_target_gone(
                    &self.cfg.sys_root,
                    state.key,
                    &mut self.bufs.small,
                    &mut self.general.stat,
                )
            {
                self.extra
                    .push_back(state.update(ScanPhase::StoppedExited, Validity::Valid));
                // Any end drops the scan below.
                *end = Some(ScanEnd::Cancelled);
                continue;
            }
            let (phase, validity) = match end {
                None if elapsed(state.last_publish, now, SCAN_PUBLISH_PERIOD) => {
                    state.last_publish = Some(now);
                    (ScanPhase::Scanning, Validity::Valid)
                }
                None => continue,
                Some(ScanEnd::Finished) => (ScanPhase::Finished, Validity::Valid),
                Some(ScanEnd::Stopped) => (ScanPhase::StoppedLimit, Validity::Valid),
                Some(ScanEnd::Cancelled) => (ScanPhase::Cancelled, Validity::Valid),
                Some(ScanEnd::Failed) => {
                    (ScanPhase::Failed(SCAN_READ_FAILED.into()), Validity::Failed)
                }
            };
            self.extra.push_back(state.update(phase, validity));
        }
        // The next sample goes on after the last scan that read, counted among the scans left.
        let ended_before = ends[..i].iter().filter(|e| e.is_some()).count();
        self.scan_next = i - ended_before;
        let mut ends = ends.iter();
        self.scans
            .retain(|_| ends.next().is_some_and(Option::is_none));
    }

    /// Takes what the helper scans found since the last sample and queues their payloads: at
    /// most every `SCAN_PUBLISH_PERIOD` while one runs, and its end at once. A scan whose
    /// process left the demand stops without a payload.
    fn step_helper_scans(&mut self, now: TimeNs) {
        if self.helper_scans.is_empty() {
            return;
        }
        let Some(side) = &self.helper else {
            return;
        };
        let details = self.details.borrow().clone();
        let mut store = lock(&side.store);
        self.helper_scans.retain_mut(|state| {
            let shown = details
                .iter()
                .any(|d| d.target == DetailTarget::Process(state.key));
            let Some(feed) = store.scans.get_mut(&state.scan).filter(|_| shown) else {
                store.scans.remove(&state.scan);
                return false;
            };
            if !feed.hits.is_empty() {
                state.found = state
                    .found
                    .saturating_add(u32::try_from(feed.hits.len()).unwrap_or(u32::MAX));
                state.chunks.push(feed.hits.drain(..).collect());
            }
            let (phase, validity) = match feed.end {
                None if elapsed(state.last_publish, now, SCAN_PUBLISH_PERIOD) => {
                    state.last_publish = Some(now);
                    (ScanPhase::Scanning, Validity::Valid)
                }
                None => return true,
                Some(HelperScanEnd::Finished) => (ScanPhase::Finished, Validity::Valid),
                Some(HelperScanEnd::Limit) => (ScanPhase::StoppedLimit, Validity::Valid),
                Some(HelperScanEnd::Exited) => (ScanPhase::StoppedExited, Validity::Valid),
                Some(HelperScanEnd::Restricted) => (
                    ScanPhase::Failed(SCAN_RESTRICTED.into()),
                    Validity::Restricted,
                ),
                Some(HelperScanEnd::Failed) => {
                    (ScanPhase::Failed(SCAN_READ_FAILED.into()), Validity::Failed)
                }
            };
            let ended = feed.end.is_some();
            if ended {
                store.scans.remove(&state.scan);
            }
            self.extra.push_back(state.update(phase, validity));
            !ended
        });
    }

    /// Drops the helper scans of `key`, without a payload.
    fn drop_helper_scans(&mut self, key: ProcessKey) {
        let side = self.helper.as_ref();
        self.helper_scans.retain(|s| {
            let keep = s.key != key;
            if !keep && let Some(side) = side {
                side.drop_scan(s.scan);
            }
            keep
        });
    }

    /// Starts the requested handle search in place of the running one, which is dropped without
    /// another payload. Then walks one budget of pids and queues the state, one per sample.
    fn step_search(&mut self) {
        if let Some(req) = self.search_request.take() {
            let search = HandleSearch::new(
                req.id,
                &req.pattern,
                req.include_maps,
                req.privileged,
                &self.cfg.sys_root,
            );
            if req.cancelled {
                search.cancel();
            }
            self.search = Some(search);
        }
        let Some(search) = &mut self.search else {
            return;
        };
        let state = search.step(&self.cfg.sys_root, HANDLE_SEARCH_BUDGET);
        if search.done() {
            self.search = None;
        }
        self.extra
            .push_back(SourceUpdate::Details(DetailsUpdate::HandleSearch(state)));
    }

    /// Stops handle search `id`: the next sample publishes `Stopped`. False when no such search
    /// waits or runs.
    fn cancel_search(&mut self, id: SearchId) -> bool {
        if let Some(req) = self.search_request.as_mut().filter(|r| r.id == id) {
            req.cancelled = true;
            return true;
        }
        match &self.search {
            Some(search) if search.id == id => {
                search.cancel();
                true
            }
            _ => false,
        }
    }

    /// Stops scan `id` without another read and queues its `Cancelled` payload. False when no
    /// such scan waits or runs.
    fn cancel_scan(&mut self, id: ScanId) -> bool {
        if let Some(i) = self.scan_requests.iter().position(|r| r.0 == id) {
            let (scan, key, _) = self.scan_requests.remove(i);
            self.extra.push_back(strings_update(
                key,
                scan,
                0,
                0,
                &[],
                ScanPhase::Cancelled,
                Validity::Valid,
            ));
            return true;
        }
        if let Some(i) = self.helper_scans.iter().position(|s| s.scan == id) {
            // The drop closes the job's `stop`: the helper stream drops and sends `CancelScan`.
            let state = self.helper_scans.remove(i);
            if let Some(side) = &self.helper {
                side.drop_scan(id);
            }
            self.extra
                .push_back(state.update(ScanPhase::Cancelled, Validity::Valid));
            return true;
        }
        let Some(i) = self.scans.iter().position(|s| s.scan == id) else {
            return false;
        };
        let state = self.scans.remove(i);
        self.extra
            .push_back(state.update(ScanPhase::Cancelled, Validity::Valid));
        true
    }
}

impl Source for ProcessDetailSource {
    fn id(&self) -> SourceId {
        SourceId::ProcessDetail
    }

    /// PSS and affinity alone run at 1 Hz and tabs alone at `processes.interval_ms`. PSS beside
    /// tabs splits the interval into `shared_parts` equal parts of at most 1 s: tabs read exactly
    /// at the interval, and PSS at the first sample 1 s or more after its last read. Affinity
    /// beside tabs uses `affinity_shared_period` instead, with or without PSS, so affinity reads
    /// every 1 s worth of samples. The period depends only on the demand and the settings, so it
    /// changes only when they do, or when a Strings scan or a handle search starts or ends: while
    /// one runs or waits the period is `SCAN_PERIOD`, and tabs and affinity read by the clock.
    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        // A forced `RefreshNow` samples even without a period, so hidden columns must leave
        // no keys behind.
        let shown = wants_pss(d);
        *self.keys.borrow_mut() = if shown {
            d.detail_keys.clone()
        } else {
            Arc::from([])
        };
        let details = self.cap_details(d);
        // Done here, not in `sample`: with no demand left no sample runs, and a window that
        // opens again must get its tabs, not "unchanged" from the old state. A tab that appears
        // gets a state marked `open_pending`. Only the reducer's `DetailOpened` clears it, so
        // after a worker restart, when no notice comes for tabs open already, a user refresh
        // still reads.
        {
            let mut targets = self.targets.borrow_mut();
            targets.retain(|key, target| {
                let process = DetailTarget::Process(*key);
                let shown = details.iter().any(|d| d.target == process);
                for tab in DetailTab::ALL {
                    // General stays while any tab of its target is demanded.
                    let demanded = if tab == DetailTab::General {
                        shown
                    } else {
                        details.contains(&DetailDemand {
                            target: process,
                            tab,
                        })
                    };
                    if !demanded {
                        target.tabs[tab.index()] = None;
                    }
                }
                target.tabs.iter().any(Option::is_some)
            });
            for d in details.iter() {
                let DetailTarget::Process(key) = d.target else {
                    continue;
                };
                let tabs = &mut targets.entry(key).or_default().tabs;
                // General never waits for an open notice: every notice resends it.
                tabs[DetailTab::General.index()].get_or_insert_with(TabState::default);
                if d.tab != DetailTab::General && reads_from_demand(d.tab) {
                    tabs[d.tab.index()].get_or_insert_with(|| TabState {
                        open_pending: true,
                        ..TabState::default()
                    });
                }
            }
        }
        let affinity = &d.affinity_targets[..d.affinity_targets.len().min(AFFINITY_TARGETS_MAX)];
        *self.affinity_keys.borrow_mut() = Arc::from(affinity);
        // PSS and affinity both run at 1 Hz.
        let fast = (shown && !d.detail_keys.is_empty()) || !affinity.is_empty();
        let tabs = wants_tabs(&details);
        if let Some(side) = &self.helper {
            lock(&side.store).wanted = Wanted {
                details: Arc::clone(&details),
                pss: Arc::clone(&self.keys.borrow()),
            };
        }
        *self.details.borrow_mut() = details;
        let interval = tab_interval(s);
        let (period, tabs_every, affinity_every) = match (fast, tabs) {
            (true, true) if !affinity.is_empty() => {
                let (period, tabs_every, affinity_every) = affinity_shared_period(interval);
                (Some(period), tabs_every, affinity_every)
            }
            (true, true) => {
                let parts = shared_parts(interval);
                (Some(interval / parts), parts, 1)
            }
            (true, false) => (Some(PSS_PERIOD), 1, 1),
            (false, true) => (Some(interval), 1, 1),
            (false, false) => (None, 1, 1),
        };
        self.tabs_every.set(tabs_every);
        self.affinity_every.set(affinity_every);
        self.interval.set(interval);
        let scanning = !self.scans.is_empty()
            || !self.scan_requests.is_empty()
            || !self.helper_scans.is_empty()
            || self.search.is_some()
            || self.search_request.is_some();
        self.scanning.set(scanning);
        if scanning {
            return Some(SCAN_PERIOD);
        }
        period
    }

    /// The Threads tab CPU baselines start over.
    fn reset_baselines(&mut self) {
        for target in self.targets.get_mut().values_mut() {
            target.threads = None;
        }
    }

    fn command(&mut self, cmd: &WorkerCommand) -> bool {
        match cmd {
            WorkerCommand::DetailRefresh { target, tab } => {
                self.refresh.push((*target, *tab, false));
                true
            }
            WorkerCommand::DetailOpened { target, tab } => {
                self.refresh.push((*target, *tab, true));
                true
            }
            WorkerCommand::ComputeHashes(key) => {
                if !self.hash_requests.contains(key) {
                    if self.hash_requests.len() >= DETAILS_MAX {
                        self.hash_requests.remove(0);
                    }
                    self.hash_requests.push(*key);
                }
                true
            }
            WorkerCommand::VerifyPackage(key) => {
                if !self.verify_requests.contains(key) {
                    if self.verify_requests.len() >= DETAILS_MAX {
                        self.verify_requests.remove(0);
                    }
                    self.verify_requests.push(*key);
                }
                true
            }
            WorkerCommand::StringScan { scan, key, opts } => {
                // A new scan of a process replaces the one it has.
                self.scan_requests.retain(|r| r.1 != *key);
                self.scans.retain(|s| s.key != *key);
                self.drop_helper_scans(*key);
                let running = self.scan_requests.len() + self.scans.len() + self.helper_scans.len();
                if running >= DETAILS_MAX {
                    if !self.scans.is_empty() {
                        self.scans.remove(0);
                    } else if let Some(oldest) = self.helper_scans.first().map(|s| s.key) {
                        self.drop_helper_scans(oldest);
                    } else {
                        self.scan_requests.remove(0);
                    }
                }
                self.scan_requests.push((*scan, *key, *opts));
                true
            }
            WorkerCommand::CancelStringScan(scan) => self.cancel_scan(*scan),
            WorkerCommand::HandleSearch {
                search,
                pattern,
                include_maps,
                privileged,
            } => {
                // A newer request replaces one that has not started.
                self.search_request = Some(SearchRequest {
                    id: *search,
                    pattern: pattern.clone(),
                    include_maps: *include_maps,
                    privileged: *privileged,
                    cancelled: false,
                });
                true
            }
            WorkerCommand::CancelHandleSearch(search) => self.cancel_search(*search),
            _ => false,
        }
    }

    fn sample(self) -> (Self, SourceUpdate) {
        let now = (self.clock)();
        self.sample_at(now)
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.extra.pop_front()
    }
}

/// A Strings tab update. Every payload carries its `ScanId`, as the UI drops the payloads of
/// other scans.
fn strings_update(
    key: ProcessKey,
    scan: ScanId,
    progress_pct: u8,
    found: u32,
    chunks: &[Arc<[StringHit]>],
    phase: ScanPhase,
    validity: Validity,
) -> SourceUpdate {
    let state = StringScanState {
        scan,
        progress_pct,
        found,
        chunks: Arc::from(chunks),
        phase,
    };
    SourceUpdate::Details(DetailsUpdate::Tab {
        target: DetailTarget::Process(key),
        tab: DetailTab::Strings,
        payload: Arc::new(TabPayload::Strings(Arc::new(state))),
        validity,
    })
}

fn string_hit(hit: StringHitInfo) -> StringHit {
    StringHit {
        address: hit.address,
        encoding: match hit.encoding {
            Encoding::Ascii => StringEncoding::Ascii,
            Encoding::Utf8 => StringEncoding::Utf8,
            Encoding::Utf16Le => StringEncoding::Utf16Le,
        },
        length: hit.length,
        value: Arc::from(hit.value),
    }
}

/// Opens the memory of `key` for a Strings scan. A kernel thread, and a target Yama would not
/// let tasklens attach to, are refused before anything is opened. The error is the phase and
/// validity of the scan's only payload.
fn start_scan(
    root: &SysRoot,
    key: ProcessKey,
    opts: StringScanOpts,
    bufs: &mut ReadBufs,
    stat: &mut PidStat,
) -> Result<(StringScanner, ProcMem), (ScanPhase, Validity)> {
    let failed = || (ScanPhase::Failed(SCAN_READ_FAILED.into()), Validity::Failed);
    let exited = || (ScanPhase::StoppedExited, Validity::Missing);
    let restricted = || {
        (
            ScanPhase::Failed(SCAN_RESTRICTED.into()),
            Validity::Restricted,
        )
    };
    match read_pid_stat(root, key.pid, &mut bufs.small, stat) {
        Ok(()) if stat.start_ticks == key.start_ticks => {}
        Ok(()) | Err(CollectError::NotFound) => return Err(exited()),
        Err(e) => {
            tracing::debug!(error = %e, "strings scan: stat not read");
            return Err(failed());
        }
    }
    if stat.flags & PF_KTHREAD != 0 {
        return Err((
            ScanPhase::Failed(SCAN_KERNEL_THREAD.into()),
            Validity::Unsupported,
        ));
    }
    let ppid = stat.ppid;
    if !ptrace_allowed(root, key.pid, ppid, &mut bufs.small, stat) {
        return Err(restricted());
    }
    let mut regions = Vec::new();
    let mut nodes = Vec::new();
    match rd::read_maps(root, key, false, bufs, &mut regions, &mut nodes) {
        Ok(DetailRead::Ok) => {}
        Ok(DetailRead::Restricted) => return Err(restricted()),
        Ok(DetailRead::Gone) => return Err(exited()),
        Err(e) => {
            tracing::debug!(error = %e, "strings scan: maps not read");
            return Err(failed());
        }
    }
    let mut ranges = Vec::new();
    select_regions(&regions, opts.include_file_backed, &mut ranges);
    let mem = match ProcMem::open(root, key) {
        Ok(mem) => mem,
        Err(CollectError::PermissionDenied) => return Err(restricted()),
        Err(CollectError::ProcessGone { .. }) => return Err(exited()),
        Err(e) => {
            tracing::debug!(error = %e, "strings scan: mem not opened");
            return Err(failed());
        }
    };
    let opts = ScanOpts {
        min_len: opts.min_len,
        ascii: opts.ascii,
        utf8: opts.utf8,
        utf16le: opts.utf16le,
        include_file_backed: opts.include_file_backed,
    };
    Ok((StringScanner::new(opts, ranges), mem))
}

/// True when the process of a running scan exited: its pid is gone or reused, or it is a
/// zombie. A stat that cannot be read for another reason counts as running.
fn scan_target_gone(
    root: &SysRoot,
    key: ProcessKey,
    buf: &mut Vec<u8>,
    stat: &mut PidStat,
) -> bool {
    match read_pid_stat(root, key.pid, buf, stat) {
        Ok(()) => stat.start_ticks != key.start_ticks || matches!(stat.state, b'Z' | b'X'),
        Err(CollectError::NotFound | CollectError::ProcessGone { .. }) => true,
        Err(e) => {
            tracing::debug!(error = %e, "strings scan: stat not read");
            false
        }
    }
}

/// Reads and parses `<proc>/<pid>/stat`.
fn read_pid_stat(
    root: &SysRoot,
    pid: u32,
    buf: &mut Vec<u8>,
    stat: &mut PidStat,
) -> Result<(), CollectError> {
    read_file(
        &root.proc.join(pid.to_string()).join("stat"),
        buf,
        SMALL_FILE_LIMIT,
    )?;
    parse_pid_stat(buf, stat)
}

/// True when Yama lets tasklens attach to `pid`, whose parent is `ppid`: scope 0, or scope 1
/// with tasklens among the first `ANCESTOR_HOPS` parents of `pid`. No `ptrace_scope` file means
/// no Yama, which counts as scope 0. Scope 2 or 3, or a file that cannot be read, refuses.
/// Opening `mem` checks the rest of the ptrace rules.
fn ptrace_allowed(
    root: &SysRoot,
    pid: u32,
    mut ppid: u32,
    buf: &mut Vec<u8>,
    stat: &mut PidStat,
) -> bool {
    let scope = match read_file(
        &root.proc.join("sys/kernel/yama/ptrace_scope"),
        buf,
        PTRACE_SCOPE_MAX,
    ) {
        Ok(_) => std::str::from_utf8(buf)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok()),
        Err(CollectError::NotFound) => Some(0),
        Err(_) => None,
    };
    match scope {
        Some(0) => return true,
        Some(1) => {}
        _ => return false,
    }
    let me = rustix::process::getpid()
        .as_raw_nonzero()
        .get()
        .unsigned_abs();
    if pid == me {
        return true;
    }
    for _ in 0..ANCESTOR_HOPS {
        if ppid == me {
            return true;
        }
        if ppid <= 1 || read_pid_stat(root, ppid, buf, stat).is_err() {
            return false;
        }
        ppid = stat.ppid;
    }
    false
}

/// Maps a reader result to `Ok` or the validity of a tab without data. Errors are logged
/// without paths.
fn detail_result(
    read: Result<DetailRead, tl_collect::CollectError>,
    tab: DetailTab,
) -> Result<(), Validity> {
    match read {
        Ok(DetailRead::Ok) => Ok(()),
        Ok(DetailRead::Restricted) => Err(Validity::Restricted),
        Ok(DetailRead::Gone) => Err(Validity::Missing),
        Err(e) => {
            tracing::debug!(?tab, error = %e, "process detail tab not read");
            Err(Validity::Failed)
        }
    }
}

/// Sends `read` for one tab when its payload or validity changed, and records it in `slot`.
/// `Err` resends the last data with that validity, except Restricted, which sends an empty
/// payload.
fn publish_tab(
    slot: &mut TabState,
    key: ProcessKey,
    tab: DetailTab,
    read: Result<TabRead, Validity>,
    users: &mut Option<PasswdUserNames>,
    group_file: &[u8],
    root: &SysRoot,
) -> Option<DetailsUpdate> {
    let (payload, validity) = match read {
        Ok(read) => {
            let validity = read.validity();
            let same_validity = slot.published.as_ref().is_some_and(|(_, v)| *v == validity);
            if same_validity && slot.last.as_ref() == Some(&read) {
                return None;
            }
            let payload = Arc::new(build_payload(&read, users, group_file, root));
            slot.last = Some(read);
            (payload, validity)
        }
        Err(validity) => {
            let payload = match &slot.published {
                Some((_, v)) if *v == validity => return None,
                // A gone process or a failed read keeps the last data.
                Some((p, _)) if validity != Validity::Restricted => Arc::clone(p),
                _ => Arc::new(empty_payload(tab, validity)),
            };
            slot.last = None;
            (payload, validity)
        }
    };
    slot.published = Some((Arc::clone(&payload), validity));
    Some(DetailsUpdate::Tab {
        target: DetailTarget::Process(key),
        tab,
        payload,
        validity,
    })
}

/// Each requested file of a `ReadProcessFiles` reply with its data or errno. The helper sends
/// each file in ascending bit order as one or more `File` bodies or one `Err(Os(_))` body
/// (E29-004). Any other `Err` body is an error of the whole read and gives its validity: an
/// exited or replaced process is Missing.
fn helper_files(
    files: ProcFiles,
    replies: &[HelperReply],
    too_large: &mut bool,
) -> Result<Vec<HelperFile>, Validity> {
    let order: SmallVec<[ProcFiles; 4]> = files.iter().collect();
    let mut next = 0;
    let mut out: Vec<HelperFile> = Vec::new();
    // The file of the last `File` body, which a following chunk continues.
    let mut current = None;
    for reply in replies {
        match &reply.body {
            HelperReplyBody::File { file, data } if current == Some(*file) => {
                if let Some((_, Ok(buf))) = out.last_mut() {
                    buf.extend_from_slice(data);
                }
            }
            HelperReplyBody::File { file, data } => {
                let rest = order.get(next..).unwrap_or_default();
                let Some(at) = rest.iter().position(|f| f == file) else {
                    return Err(Validity::Failed);
                };
                next += at + 1;
                current = Some(*file);
                out.push((*file, Ok(data.clone())));
            }
            HelperReplyBody::Err(HelperError::Os(errno)) => {
                let Some(&file) = order.get(next) else {
                    return Err(Validity::Failed);
                };
                next += 1;
                current = None;
                out.push((file, Err(*errno)));
            }
            HelperReplyBody::Err(e) => {
                return Err(match e {
                    HelperError::IdentityChanged | HelperError::NoSuchProcess => Validity::Missing,
                    HelperError::PermissionDenied | HelperError::NotAllowed => Validity::Restricted,
                    // The files together passed the 4 MiB reply cap (R-157).
                    HelperError::TooLarge => {
                        *too_large = true;
                        Validity::Failed
                    }
                    _ => Validity::Failed,
                });
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The data or errno of `file` in a helper reply.
fn helper_file(files: &[HelperFile], file: ProcFiles) -> Option<&Result<Vec<u8>, i32>> {
    files.iter().find(|(f, _)| *f == file).map(|(_, data)| data)
}

/// The validity of a file the helper could not read. EFBIG also sets `too_large`.
fn errno_validity(errno: i32, too_large: &mut bool) -> Validity {
    match errno {
        // EPERM, EACCES.
        1 | 13 => Validity::Restricted,
        // ENOENT, ESRCH.
        2 | 3 => Validity::Missing,
        EFBIG => {
            *too_large = true;
            Validity::Failed
        }
        _ => Validity::Failed,
    }
}

/// The Environment tab from a helper `ENVIRON`, parsed as `read_environ` does.
fn environ_from_helper(files: &[HelperFile], too_large: &mut bool) -> Result<TabRead, Validity> {
    let data = match helper_file(files, ProcFiles::ENVIRON) {
        Some(Ok(data)) => data,
        Some(Err(errno)) => return Err(errno_validity(*errno, too_large)),
        None => return Err(Validity::Failed),
    };
    let mut info = EnvInfo::default();
    if data.is_empty() {
        return Ok(TabRead::Environ {
            info,
            read: EnvRead::Empty,
        });
    }
    let truncated = data.len() > rd::ENVIRON_MAX;
    rd::parse_environ(
        &data[..data.len().min(rd::ENVIRON_MAX)],
        truncated,
        &mut info,
    );
    Ok(TabRead::Environ {
        info,
        read: EnvRead::Ok,
    })
}

/// The Memory maps tab from a helper `MAPS` and `SMAPS`, grouped as `read_maps` does. `exe` is
/// the General executable path, for the primary node. An `smaps` the helper could not read gives
/// the tab its errno's validity, as for `maps`: Restricted, Missing (the target exited) or
/// Failed (EFBIG also sets `too_large`). An `smaps` that does not parse leaves RSS unread.
fn maps_from_helper(
    files: &[HelperFile],
    exe: Option<&str>,
    too_large: &mut bool,
) -> Result<TabRead, Validity> {
    let data = match helper_file(files, ProcFiles::MAPS) {
        Some(Ok(data)) => data,
        Some(Err(errno)) => return Err(errno_validity(*errno, too_large)),
        None => return Err(Validity::Failed),
    };
    let rss = match helper_file(files, ProcFiles::SMAPS) {
        Some(Ok(smaps)) => {
            let mut regions = Vec::new();
            parse_smaps(smaps, &mut regions)
                .ok()
                .map(|()| rss_by_start(&regions))
        }
        Some(Err(errno)) => return Err(errno_validity(*errno, too_large)),
        None => return Err(Validity::Failed),
    };
    let mut regions = Vec::new();
    if rd::parse_maps(data, &mut regions).is_err() {
        tracing::debug!("helper maps not parsed");
        return Err(Validity::Failed);
    }
    let mut nodes = Vec::new();
    rd::group_maps(&mut regions, exe, rss.as_ref(), &mut nodes);
    Ok(TabRead::Maps { regions, nodes })
}

/// The General values of a helper reply for the `requested` files. A file the helper could not
/// read gets its errno's validity (EFBIG also sets `too_large`); a file missing from the reply
/// or that does not parse is Failed.
fn general_patch(
    requested: ProcFiles,
    files: &[HelperFile],
    ctx: Option<&LocationCtx>,
    too_large: &mut bool,
) -> GeneralPatch {
    let mut patch = GeneralPatch::default();
    let mut text = String::new();
    // At most one byte past the limit, so `sanitize_link` sees the cut and adds `…`, as the
    // direct read does.
    let link = |data: &[u8], text: &mut String| -> Result<Arc<str>, Validity> {
        sanitize_link(&data[..data.len().min(EXE_LINK_MAX + 1)], text);
        if text.is_empty() {
            return Err(Validity::Failed);
        }
        Ok(Arc::from(text.as_str()))
    };
    for file in requested.iter() {
        let data = match helper_file(files, file) {
            Some(Ok(data)) => Ok(data.as_slice()),
            Some(Err(errno)) => Err(errno_validity(*errno, too_large)),
            None => Err(Validity::Failed),
        };
        match file {
            ProcFiles::EXE => {
                patch.exe = Some(data.and_then(|data| link(data, &mut text)).map(|exe| {
                    let deleted = exe.ends_with(DELETED_SUFFIX);
                    let location = ctx.map_or(ExeLocation::Other, |ctx| {
                        exe_location(rd::location(&exe, deleted, ctx))
                    });
                    (exe, deleted, location)
                }));
            }
            ProcFiles::CWD => patch.cwd = Some(data.and_then(|data| link(data, &mut text))),
            ProcFiles::ROOT => patch.root = Some(data.and_then(|data| link(data, &mut text))),
            ProcFiles::IO => {
                patch.io = Some(data.and_then(|data| {
                    let mut io = PidIo::default();
                    parse_io(data, &mut io).map_err(|_| Validity::Failed)?;
                    Ok(io)
                }));
            }
            ProcFiles::STATUS => {
                patch.status = Some(data.and_then(|data| {
                    let mut status = PidStatus::default();
                    parse_status(&data[..data.len().min(STATUS_READ_MAX)], &mut status)
                        .map_err(|_| Validity::Failed)?;
                    Ok(StatusFields::new(&status, &mut SmallVec::new()))
                }));
            }
            _ => {}
        }
    }
    patch
}

/// PSS, USS and swap from a helper `SMAPS_ROLLUP`. `None` while the value stays Restricted:
/// the helper was not `Unlocked`, the read failed, or the file gave EACCES or EPERM.
fn pss_from_helper(
    files: ProcFiles,
    result: HelperResult,
    too_large: &mut bool,
) -> Option<HelperPss> {
    let replies = result.ok()?;
    let files = match helper_files(files, &replies, too_large) {
        Ok(files) => files,
        // `IdentityChanged` or `NoSuchProcess`.
        Err(Validity::Missing) => return Some(HelperPss::Gone),
        Err(Validity::Restricted) => return None,
        Err(validity) => return Some(HelperPss::Invalid(validity)),
    };
    let data = match helper_file(&files, ProcFiles::SMAPS_ROLLUP) {
        Some(Ok(data)) => data,
        Some(Err(errno)) => {
            return match errno_validity(*errno, too_large) {
                Validity::Restricted => None,
                validity => Some(HelperPss::Invalid(validity)),
            };
        }
        None => return Some(HelperPss::Invalid(Validity::Failed)),
    };
    let mut rollup = Rollup::default();
    if parse_rollup(data, &mut rollup).is_err() {
        return Some(HelperPss::Invalid(Validity::Failed));
    }
    Some(HelperPss::Ok(rollup))
}

/// The parent of the proc root: `/` on the host, the fixture root in tests.
fn root_base(root: &SysRoot) -> &Path {
    root.proc.parent().unwrap_or(Path::new("/"))
}

/// `<sys>/devices/system/cpu/online`, or `last` when it cannot be read.
fn read_online_cpus(root: &SysRoot, last: u32) -> u32 {
    let mut cpus: SmallVec<[u16; 64]> = SmallVec::new();
    match std::fs::read(root.sys.join("devices/system/cpu/online")) {
        Ok(bytes) if parse_cpu_list(&bytes, &mut cpus).is_ok() && !cpus.is_empty() => {
            u32::try_from(cpus.len()).unwrap_or(u32::MAX)
        }
        _ => last,
    }
}

/// Sets the RSS of the last `smaps` read on regions and nodes read without it.
fn apply_rss(rss: &FxHashMap<u64, u64>, regions: &mut [MapRegionInfo], nodes: &mut [MapNodeInfo]) {
    if rss.is_empty() {
        return;
    }
    for region in regions.iter_mut() {
        region.rss = rss.get(&region.start).copied();
    }
    for node in nodes.iter_mut() {
        let range = node.regions.start as usize..node.regions.end as usize;
        let sum = regions
            .get(range)
            .unwrap_or(&[])
            .iter()
            .filter_map(|r| r.rss)
            .fold(0u64, u64::saturating_add);
        node.rss = Some(sum);
    }
}

/// Sets the protocol of each unresolved socket row from the `system.sockprotoname` xattr of its
/// `fd/<n>` link. `known` caches the protocol by inode and keeps only the inodes still open. A
/// row whose xattr cannot be read stays `Socket`.
fn resolve_sockets(
    root: &SysRoot,
    pid: u32,
    known: &mut FxHashMap<u64, FdType>,
    rows: &mut [FdInfo],
) {
    let mut next = FxHashMap::default();
    let fd_dir = root.proc.join(pid.to_string()).join("fd");
    for row in rows.iter_mut().filter(|r| r.kind == FdType::Socket) {
        let Some(inode) = row.inode else {
            continue;
        };
        let kind = match known.get(&inode) {
            Some(&kind) => kind,
            None => socket_protocol(&fd_dir.join(row.fd.to_string())),
        };
        row.kind = kind;
        if kind != FdType::Socket {
            next.insert(inode, kind);
        }
    }
    *known = next;
}

/// The fd type of a socket from the protocol name the kernel gives its inode.
fn socket_protocol(link: &Path) -> FdType {
    let mut name = [0u8; 32];
    match rustix::fs::getxattr(link, "system.sockprotoname", &mut name[..]) {
        Ok(len) => protocol_type(&name[..len]),
        Err(_) => FdType::Socket,
    }
}

/// "TCP", "UDPv6", "UNIX-STREAM" and the like, with or without the trailing NUL.
fn protocol_type(name: &[u8]) -> FdType {
    match name.strip_suffix(b"\0").unwrap_or(name) {
        b"TCP" | b"TCPv6" | b"MPTCP" | b"MPTCPv6" => FdType::Tcp,
        b"UDP" | b"UDPv6" | b"UDP-Lite" | b"UDPLITEv6" => FdType::Udp,
        b"UNIX" | b"UNIX-STREAM" => FdType::Unix,
        b"NETLINK" => FdType::Netlink,
        _ => FdType::Socket,
    }
}

/// The tl-model payload of one reader output.
/// `users` is loaded from `<root>/etc/passwd` the first time a Security payload is built.
fn build_payload(
    read: &TabRead,
    users: &mut Option<PasswdUserNames>,
    group_file: &[u8],
    root: &SysRoot,
) -> TabPayload {
    match read {
        TabRead::General(read) => {
            let users = users
                .get_or_insert_with(|| PasswdUserNames::load(&root_base(root).join("etc/passwd")));
            TabPayload::General(Arc::new(general_detail(read, users)))
        }
        TabRead::Threads(rows) => TabPayload::Threads(rows.iter().map(thread_row).collect()),
        TabRead::Fds(rows) => TabPayload::Fds(Arc::new(fd_detail(rows))),
        TabRead::Maps { regions, nodes } => TabPayload::Maps(Arc::new(maps_detail(regions, nodes))),
        TabRead::Security(info) => {
            let users = users
                .get_or_insert_with(|| PasswdUserNames::load(&root_base(root).join("etc/passwd")));
            TabPayload::Security(Arc::new(security_detail(info, users, group_file, root)))
        }
        TabRead::Environ { info, read } => TabPayload::Environ(Arc::new(EnvDetail {
            vars: Arc::from(info.vars.clone()),
            truncated: info.truncated,
            read_errno: match read {
                EnvRead::Failed(errno) => Some(*errno),
                _ => None,
            },
            unavailable: *read == EnvRead::Empty,
        })),
    }
}

/// A payload with no data, for a tab that was never read with data.
fn empty_payload(tab: DetailTab, validity: Validity) -> TabPayload {
    match tab {
        DetailTab::General => TabPayload::General(Arc::new(empty_general(validity))),
        DetailTab::Threads => TabPayload::Threads(Arc::from([])),
        DetailTab::Fds => TabPayload::Fds(Arc::default()),
        DetailTab::Maps => TabPayload::Maps(Arc::default()),
        DetailTab::Security => TabPayload::Security(Arc::new(empty_security(validity))),
        _ => TabPayload::Environ(Arc::default()),
    }
}

/// `SchedPolicy` from stat field 41. Unknown values show as Other.
fn sched_policy(policy: u32) -> SchedPolicy {
    match policy {
        1 => SchedPolicy::Fifo,
        2 => SchedPolicy::Rr,
        3 => SchedPolicy::Batch,
        5 => SchedPolicy::Idle,
        6 => SchedPolicy::Deadline,
        _ => SchedPolicy::Other,
    }
}

/// A restricted wait channel makes the row `Restricted`; the UI then hides only that cell.
fn thread_row(t: &ThreadInfo) -> ThreadRow {
    let (wchan, validity) = match &t.wchan {
        WchanValue::Name(name) => (Arc::clone(name), t.cpu_validity),
        WchanValue::Running => (Arc::from("0"), t.cpu_validity),
        WchanValue::Restricted => (Arc::from(""), Validity::Restricted),
    };
    ThreadRow {
        tid: t.tid,
        comm: Arc::clone(&t.comm),
        state: ProcState::from_stat_char(t.state),
        cpu_pct: t.cpu_pct,
        utime_ms: t.utime_ms,
        stime_ms: t.stime_ms,
        policy: sched_policy(t.policy),
        rt_prio: u8::try_from(t.rt_prio).unwrap_or(u8::MAX),
        nice: t.nice,
        cpus: Arc::clone(&t.cpus),
        last_cpu: u32::try_from(t.last_cpu).unwrap_or(0),
        vol_ctx: t.vol_ctx.unwrap_or(0),
        invol_ctx: t.invol_ctx.unwrap_or(0),
        wchan,
        validity,
    }
}

/// tl-model has no plain socket kind. A socket whose protocol could not be read shows as
/// `anon_inode`, the "other" type (PROC-080); its name still reads `socket:[<inode>]`.
fn fd_kind(kind: FdType) -> FdKind {
    match kind {
        FdType::File => FdKind::File,
        FdType::Directory => FdKind::Directory,
        FdType::Tcp => FdKind::Tcp,
        FdType::Udp => FdKind::Udp,
        FdType::Unix => FdKind::Unix,
        FdType::Netlink => FdKind::Netlink,
        FdType::Pipe => FdKind::Pipe,
        FdType::Fifo => FdKind::Fifo,
        FdType::EventFd => FdKind::Eventfd,
        FdType::EventPoll => FdKind::Eventpoll,
        FdType::TimerFd => FdKind::Timerfd,
        FdType::SignalFd => FdKind::Signalfd,
        FdType::Inotify => FdKind::Inotify,
        FdType::Fanotify => FdKind::Fanotify,
        FdType::MemFd => FdKind::Memfd,
        FdType::DmaBuf => FdKind::Dmabuf,
        FdType::Drm => FdKind::Drm,
        FdType::PidFd => FdKind::Pidfd,
        FdType::IoUring => FdKind::IoUring,
        FdType::AnonInode | FdType::Socket => FdKind::AnonInode,
        FdType::Device => FdKind::Device,
    }
}

/// `fdinfo_available` is false only when no fdinfo file could be read.
fn fd_detail(infos: &[FdInfo]) -> FdDetail {
    let rows: Arc<[FdRow]> = infos
        .iter()
        .map(|r| {
            let socket = matches!(
                r.kind,
                FdType::Socket | FdType::Tcp | FdType::Udp | FdType::Unix | FdType::Netlink
            );
            FdRow {
                fd: i32::try_from(r.fd).unwrap_or(i32::MAX),
                kind: fd_kind(r.kind),
                name: Arc::clone(&r.target),
                unnamed: r.target.is_empty() || r.target.starts_with("anon_inode:"),
                // Shown as "-" for sockets and pipes.
                pos: if socket || r.kind == FdType::Pipe {
                    None
                } else {
                    r.pos
                },
                flags: r.flags,
                mnt_id: r.mnt_id,
                inode: r.inode,
                eventfd_count: r.eventfd_count,
                drm_client_id: r.drm_client_id,
                drm_driver: r.drm_driver.clone(),
                socket_inode: if socket { r.inode } else { None },
            }
        })
        .collect();
    let fdinfo_available = infos.is_empty() || infos.iter().any(|r| r.fdinfo_ok);
    FdDetail {
        rows,
        fdinfo_available,
    }
}

/// `Valid` with a value, `WarmingUp` before the first `smaps` read.
fn rss_total(rss: Option<u64>) -> Total<u64> {
    match rss {
        Some(value) => Total {
            value,
            validity: Validity::Valid,
        },
        None => Total {
            value: 0,
            validity: Validity::WarmingUp,
        },
    }
}

/// "[heap]" and other pseudo-paths as given, "anonymous", "memfd:<name>", or `None` for a file.
fn region_label(path: Option<&Arc<str>>) -> Option<Arc<str>> {
    let Some(path) = path else {
        return Some(Arc::from("anonymous"));
    };
    if path.starts_with('[') {
        return Some(Arc::clone(path));
    }
    let memfd = path.strip_prefix('/').filter(|p| p.starts_with("memfd:"))?;
    Some(Arc::from(memfd.strip_suffix(" (deleted)").unwrap_or(memfd)))
}

fn maps_detail(regions: &[MapRegionInfo], nodes: &[MapNodeInfo]) -> MapsDetail {
    MapsDetail {
        nodes: nodes
            .iter()
            .map(|n| MapNode {
                name: Arc::clone(&n.name),
                path: n.path.clone(),
                primary: n.primary,
                outside: n.outside,
                shared_object: n.shared_object,
                base: n.base,
                size_bytes: n.size,
                rss_bytes: rss_total(n.rss),
                // The package index (PROC-027) fills these in a later bead.
                description: None,
                package: None,
                regions: n.regions.clone(),
            })
            .collect(),
        regions: regions
            .iter()
            .map(|r| MapRegion {
                start: r.start,
                end: r.end,
                perms: r.perms,
                offset: r.offset,
                rss_bytes: rss_total(r.rss),
                label: region_label(r.path.as_ref()),
            })
            .collect(),
    }
}

fn valid<T>(value: T) -> Total<T> {
    Total {
        value,
        validity: Validity::Valid,
    }
}

fn total<T>(value: T, validity: Validity) -> Total<T> {
    Total { value, validity }
}

/// `Valid` for non-empty text, else `Unsupported`.
fn text_total(text: &Arc<str>) -> Total<Arc<str>> {
    let validity = if text.is_empty() {
        Validity::Unsupported
    } else {
        Validity::Valid
    };
    total(Arc::clone(text), validity)
}

fn triple(ids: [u32; 4]) -> IdTriple {
    IdTriple {
        real: ids[0],
        effective: ids[1],
        saved: ids[2],
        fs: ids[3],
    }
}

/// The `/etc/group` name of `gid`, or its number.
fn group_label(gid: u32, group_file: &[u8]) -> Arc<str> {
    rd::group_name(gid, group_file).unwrap_or_else(|| Arc::from(gid.to_string()))
}

/// True when PID 1's link for namespace `name` can be read. Without it the reader compared
/// against tasklens.
fn init_ns_readable(root: &SysRoot, name: &str) -> bool {
    std::fs::read_link(root.proc.join("1/ns").join(name)).is_ok()
}

fn security_detail(
    info: &SecurityInfo,
    users: &mut dyn UserNames,
    group_file: &[u8],
    root: &SysRoot,
) -> SecurityDetail {
    let seccomp = seccomp_total(info.seccomp, info.seccomp_filters);
    let security_label = match &info.lsm_label {
        LsmLabel::Label(label) => valid(Arc::clone(label)),
        LsmLabel::NotApplicable => total(Arc::from(""), Validity::Unsupported),
        LsmLabel::Restricted => total(Arc::from(""), Validity::Restricted),
    };
    let elevated = match info.elevated {
        rd::Elevated::Root => Elevated::Root,
        rd::Elevated::Capabilities => Elevated::Capabilities,
        rd::Elevated::Setuid => Elevated::Setuid,
        rd::Elevated::No => Elevated::No,
    };
    let namespaces = std::array::from_fn(|i| {
        let (kind, name) = NAMESPACES[i];
        let (state, validity) = match info.namespaces[i] {
            rd::NsState::SharedWithInit => (NsState::SharedWithInit, Validity::Valid),
            rd::NsState::SameAsTasklens => (NsState::SameAsTasklens, Validity::Valid),
            rd::NsState::Isolated(inode) if init_ns_readable(root, name) => {
                (NsState::Isolated { inode }, Validity::Valid)
            }
            rd::NsState::Isolated(inode) => {
                (NsState::IsolatedFromTasklens { inode }, Validity::Valid)
            }
            // The state is not shown for a restricted row.
            rd::NsState::Restricted => (NsState::SharedWithInit, Validity::Restricted),
        };
        NsRow {
            kind,
            state,
            validity,
        }
    });
    let caps: Arc<[CapRow]> = (0..64u8)
        .filter_map(|cap| {
            let bit = 1u64 << cap;
            let effective = info.cap_eff & bit != 0;
            let permitted = info.cap_prm & bit != 0;
            let inheritable = info.cap_inh & bit != 0;
            let ambient = info.cap_amb & bit != 0;
            (effective || permitted || inheritable || ambient).then(|| CapRow {
                cap,
                effective,
                permitted,
                inheritable,
                bounding: info.cap_bnd & bit != 0,
                ambient,
                risky: (effective || ambient) && rd::RISKY_CAPS.contains(&cap),
            })
        })
        .collect();
    let primary = info.gid[0];
    let groups: Arc<[GroupMembershipRow]> = std::iter::once(primary)
        .chain(info.groups.iter().copied().filter(|&g| g != primary))
        .map(|gid| {
            let name = group_label(gid, group_file);
            let kind = rd::ADMIN_GROUPS
                .iter()
                .find(|(admin, _)| *admin == &*name)
                .map(|&(_, root_equivalent)| root_equivalent);
            GroupMembershipRow {
                name,
                gid,
                primary: gid == primary,
                root_equivalent: kind == Some(true),
                admin: kind == Some(false),
            }
        })
        .collect();
    SecurityDetail {
        uids: valid(triple(info.uid)),
        uid_names: valid(info.uid.map(|uid| users.name(uid))),
        gids: valid(triple(info.gid)),
        gid_names: valid(info.gid.map(|gid| group_label(gid, group_file))),
        elevated: valid(elevated),
        no_new_privs: match info.no_new_privs {
            Some(on) => valid(on),
            None => total(false, Validity::Unsupported),
        },
        seccomp,
        // R-163: `None` is "Not reported".
        dumpable: match info.dumpable {
            Some(on) => valid(on),
            None => total(true, Validity::Unsupported),
        },
        security_label,
        // The logind session is not read yet.
        session: None,
        audit_session: info.sessionid,
        login_uid: info.loginuid,
        namespaces,
        store_bypass: text_total(&info.speculation_store_bypass),
        indirect_branch: text_total(&info.speculation_indirect_branch),
        caps,
        groups,
    }
}

/// Seccomp from status `Seccomp:` and `Seccomp_filters:`.
fn seccomp_total(mode: Option<u8>, filters: Option<u32>) -> Total<Seccomp> {
    match mode {
        Some(0) => valid(Seccomp::Disabled),
        Some(1) => valid(Seccomp::Strict),
        Some(2) => valid(Seccomp::Filter {
            filters: filters.unwrap_or(0),
        }),
        _ => total(Seccomp::Disabled, Validity::Unsupported),
    }
}

/// The validity of a field or a tab whose read gave `e`.
fn error_validity(e: &CollectError) -> Validity {
    match e {
        CollectError::PermissionDenied => Validity::Restricted,
        CollectError::NotFound => Validity::Missing,
        _ => Validity::Failed,
    }
}

/// The validity of a field after `e`, for a file that may not exist: the `exe` of a kernel
/// thread or a zombie, or an `autogroup` or `randomize_va_space` file the kernel does not
/// have. `NotFound` is shown as "Not reported".
fn optional_validity(e: &CollectError) -> Validity {
    match e {
        CollectError::PermissionDenied => Validity::Restricted,
        CollectError::NotFound => Validity::Unsupported,
        _ => Validity::Failed,
    }
}

/// `field` with the validity of an unread `status` (`status_err`), else as it is.
fn from_status<T>(field: Total<T>, status_err: Option<Validity>) -> Total<T> {
    match status_err {
        Some(v) => total(field.value, v),
        None => field,
    }
}

fn link_total(read: LinkRead) -> Total<Arc<str>> {
    match read {
        LinkRead::Path(path) => valid(path),
        LinkRead::Restricted => total(Arc::from(""), Validity::Restricted),
        LinkRead::Gone => total(Arc::from(""), Validity::Missing),
    }
}

fn elf_arch(machine: u16) -> ElfArch {
    match machine {
        62 => ElfArch::X86_64,
        3 => ElfArch::X86,
        183 => ElfArch::AArch64,
        _ => ElfArch::Unknown,
    }
}

fn exe_location(location: rd::Location) -> ExeLocation {
    match location {
        rd::Location::TempDirectory => ExeLocation::TempDirectory,
        rd::Location::UserDownloads => ExeLocation::UserDownloads,
        rd::Location::UserData => ExeLocation::UserData,
        rd::Location::UserProfile => ExeLocation::UserProfile,
        rd::Location::System => ExeLocation::System,
        rd::Location::OptionalSoftware => ExeLocation::OptionalSoftware,
        rd::Location::Flatpak => ExeLocation::Flatpak,
        rd::Location::AppImage => ExeLocation::AppImage,
        rd::Location::WinePrefix => ExeLocation::WinePrefix,
        rd::Location::Deleted => ExeLocation::Deleted,
        rd::Location::Other => ExeLocation::Other,
    }
}

/// The value as text when it is set, not empty and absolute.
fn absolute_text(value: Option<std::ffi::OsString>) -> Option<String> {
    let text = value?.into_string().ok()?;
    text.starts_with('/').then_some(text)
}

/// `XDG_DOWNLOAD_DIR` from a `user-dirs.dirs` file, with `$HOME` expanded. `None` when it is
/// not set, not absolute, or the home directory itself (xdg-user-dirs writes `$HOME/` for a
/// disabled directory).
fn download_dir(file: &[u8], home: &str) -> Option<String> {
    let text = std::str::from_utf8(file).ok()?;
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("XDG_DOWNLOAD_DIR="))?;
    let value = value.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => format!("{home}{rest}"),
        None => value.to_owned(),
    };
    let dir = path.trim_end_matches('/');
    (path.starts_with('/') && dir != home.trim_end_matches('/')).then(|| dir.to_owned())
}

/// The tasklens user's home, runtime and download directories. `paths` overrides the home and
/// config directories from the environment.
fn location_ctx(paths: Option<&AppPaths>) -> LocationCtx {
    let home = match paths {
        Some(p) => p.home.to_str().map(str::to_owned),
        None => absolute_text(std::env::var_os("HOME")),
    }
    .unwrap_or_default();
    let config = match paths {
        Some(p) => p.xdg_config_home.clone(),
        None => std::env::var_os("XDG_CONFIG_HOME"),
    };
    let config = absolute_text(config).unwrap_or_else(|| format!("{home}/.config"));
    let mut buf = Vec::new();
    let downloads = (!home.is_empty())
        .then(|| {
            read_file(
                Path::new(&config).join("user-dirs.dirs").as_path(),
                &mut buf,
                USER_DIRS_MAX,
            )
            .ok()
        })
        .flatten()
        .and_then(|_| download_dir(&buf, &home));
    LocationCtx {
        home,
        runtime_dir: absolute_text(std::env::var_os("XDG_RUNTIME_DIR")),
        downloads,
    }
}

/// The desktop index from the XDG data directories, which `paths` overrides as in
/// `location_ctx`. A failed watch only means new desktop files are not seen until a restart.
fn app_index(paths: Option<&AppPaths>) -> AppIndex {
    let dirs = match paths {
        Some(p) => data_dirs_from_env(
            &p.home,
            p.xdg_data_home.as_deref(),
            p.xdg_data_dirs.as_deref(),
        ),
        None => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            data_dirs_from_env(
                Path::new(&home),
                std::env::var_os("XDG_DATA_HOME").as_deref(),
                std::env::var_os("XDG_DATA_DIRS").as_deref(),
            )
        }
    };
    let watch = match DesktopWatch::new(&dirs) {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::debug!(error = %e, "general tab: desktop file watch not started");
            None
        }
    };
    AppIndex {
        index: DesktopIndex::build(&dirs),
        watch,
    }
}

/// `btime` from `<proc>/stat`, read until it is found once.
fn boot_time(root: &SysRoot, g: &mut GeneralReader) -> Option<u64> {
    if g.btime.is_none() {
        let mut buf = Vec::new();
        if read_file(&root.proc.join("stat"), &mut buf, PROC_STAT_MAX).is_ok() {
            g.btime = buf
                .split(|&b| b == b'\n')
                .find_map(|line| line.strip_prefix(b"btime "))
                .and_then(|v| std::str::from_utf8(v).ok()?.trim().parse().ok());
        }
    }
    g.btime
}

/// The set of a kernel CPU list such as "0-7,16-23". `None` when the list does not parse or is
/// empty.
fn cpu_set(list: &[u8], cpus: &mut SmallVec<[u16; 64]>) -> Option<CpuSet> {
    if parse_cpu_list(list, cpus).is_err() || cpus.is_empty() {
        return None;
    }
    let mut set = CpuSet::empty();
    for &cpu in cpus.iter() {
        set.insert(cpu);
    }
    Some(set)
}

/// The first command-line token of `pid` when it is UTF-8 and not empty.
fn read_argv0<'a>(root: &SysRoot, pid: u32, buf: &'a mut Vec<u8>) -> Option<&'a str> {
    let path = root.proc.join(pid.to_string()).join("cmdline");
    read_file(&path, buf, ARGV0_MAX).ok()?;
    let first = buf.split(|&b| b == 0).next()?;
    std::str::from_utf8(first).ok().filter(|s| !s.is_empty())
}

/// The affinity mask of `key` and the CPUs its cgroup allows. `None` when the process is gone or
/// its `stat` or `status` cannot be read.
fn affinity_info(
    root: &SysRoot,
    key: ProcessKey,
    buf: &mut Vec<u8>,
    stat: &mut PidStat,
    status: &mut PidStatus,
    cpus: &mut SmallVec<[u16; 64]>,
) -> Option<AffinityInfo> {
    let dir = rustix::fs::open(
        root.proc.join(key.pid.to_string()),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let dir = dir.as_fd();
    // The directory fd pins the process, so a matching start time holds for the reads below.
    read_at(dir, "stat", buf, SMALL_FILE_LIMIT)
        .and_then(|_| parse_pid_stat(buf, stat))
        .ok()?;
    if stat.start_ticks != key.start_ticks {
        return None;
    }
    read_at(dir, "status", buf, STATUS_READ_MAX)
        .and_then(|_| parse_status(buf, status))
        .ok()?;
    let mask = cpu_set(&status.cpus_allowed_list, cpus)?;
    let cpuset_limit = match read_at(dir, "cgroup", buf, SMALL_FILE_LIMIT) {
        Ok((_, false)) => parse_cgroup_file(buf)
            .and_then(|path| cpuset_effective(root, &path.rel, buf, cpus))
            .unwrap_or(CpuSet::empty()),
        _ => CpuSet::empty(),
    };
    Some(AffinityInfo { mask, cpuset_limit })
}

/// `cpuset.cpus.effective` of the cgroup `rel` (relative to `<sys>/fs/cgroup`). A cgroup
/// without the cpuset controller has no such file and takes its parent's set, so the walk goes
/// up to the root. `None` when no level has a readable list, or when the path has a `..`
/// component: a process outside tasklens's cgroup namespace shows such a path.
fn cpuset_effective(
    root: &SysRoot,
    rel: &str,
    buf: &mut Vec<u8>,
    cpus: &mut SmallVec<[u16; 64]>,
) -> Option<CpuSet> {
    if rel.split('/').any(|c| c == "..") {
        return None;
    }
    let base = root.sys.join("fs/cgroup");
    let mut rel = rel;
    loop {
        let path = base.join(rel).join("cpuset.cpus.effective");
        if read_file(&path, buf, SMALL_FILE_LIMIT).is_ok() {
            return cpu_set(buf, cpus);
        }
        if rel.is_empty() {
            return None;
        }
        rel = rel.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

/// True when a thread other than the leader has a `Cpus_allowed_list` that differs from
/// `leader`. Threads that exit during the walk are skipped.
fn threads_differ(
    pid_dir: BorrowedFd<'_>,
    pid: u32,
    leader: &[u8],
    buf: &mut Vec<u8>,
    status: &mut PidStatus,
) -> bool {
    let Ok(task) = rustix::fs::openat(
        pid_dir,
        "task",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    let Ok(dir) = Dir::read_from(&task) else {
        return false;
    };
    for entry in dir {
        let Ok(entry) = entry else {
            break;
        };
        let Some(tid) = std::str::from_utf8(entry.file_name().to_bytes())
            .ok()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if tid == pid {
            continue;
        }
        let path = format!("{tid}/status");
        if read_at(task.as_fd(), &path, buf, STATUS_READ_MAX).is_ok()
            && parse_status(buf, status).is_ok()
            && status.cpus_allowed_list.trim_ascii() != leader.trim_ascii()
        {
            return true;
        }
    }
    false
}

/// Reads the General tab of `key` (PROC-078). `Err` is the validity of a tab without data: the
/// process is gone, or its directory or `stat` cannot be read. A field whose own file cannot be
/// read gets the validity of that error, so EACCES gives `Restricted` for that field only. The
/// fields from `status` all take its validity. `now` times the desktop index rebuild.
fn read_general(
    root: &SysRoot,
    paths: Option<&AppPaths>,
    key: ProcessKey,
    ticks_per_sec: u64,
    now: TimeNs,
    g: &mut GeneralReader,
    bufs: &mut ReadBufs,
) -> Result<GeneralRead, Validity> {
    let dir = rustix::fs::open(
        root.proc.join(key.pid.to_string()),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| error_validity(&CollectError::from(e)))?;
    let dir = dir.as_fd();
    // The directory fd pins the process, so a start time that matches here holds for every
    // file read below.
    let stat = read_at(dir, "stat", &mut bufs.small, SMALL_FILE_LIMIT)
        .and_then(|_| parse_pid_stat(&bufs.small, &mut g.stat));
    if let Err(e) = stat {
        tracing::debug!(error = %e, "general tab: stat not read");
        return Err(error_validity(&e));
    }
    if g.stat.start_ticks != key.start_ticks {
        return Err(Validity::Missing);
    }
    let status = read_at(dir, "status", &mut bufs.small, STATUS_READ_MAX)
        .and_then(|_| parse_status(&bufs.small, &mut g.status));
    // `None` when `status` was read; else the validity of every field that comes from it.
    let status_err = match status {
        Ok(()) => None,
        Err(e) => {
            tracing::debug!(error = %e, "general tab: status not read");
            match error_validity(&e) {
                Validity::Missing => return Err(Validity::Missing),
                v => {
                    g.status = PidStatus::default();
                    Some(v)
                }
            }
        }
    };
    let ticks = ticks_per_sec.max(1);
    let started_s = match boot_time(root, g) {
        Some(btime) => valid(
            i64::try_from(btime.saturating_add(g.stat.start_ticks / ticks)).unwrap_or(i64::MAX),
        ),
        None => total(0, Validity::Failed),
    };
    if g.location.is_none() {
        g.location = Some(location_ctx(paths));
    }

    // Affinity from the leader's `Cpus_allowed_list`.
    let affinity = match cpu_set(&g.status.cpus_allowed_list, &mut g.cpus) {
        Some(set) => valid(set),
        None => total(CpuSet::empty(), Validity::Unsupported),
    };
    let affinity = from_status(affinity, status_err);
    let affinity_threads_differ = affinity.validity == Validity::Valid
        && g.stat.num_threads > 1
        && threads_differ(
            dir,
            key.pid,
            &g.status.cpus_allowed_list,
            &mut bufs.small,
            &mut g.thread_status,
        );

    let (stat, status) = (&g.stat, &g.status);
    let kernel_prio = match i32::try_from(stat.priority.saturating_add(100)) {
        Ok(prio) => valid(prio),
        Err(_) => total(0, Validity::Failed),
    };
    let autogroup = match rd::read_autogroup(dir, &mut bufs.small) {
        Some((id, nice)) => valid((Arc::from(format!("autogroup-{id}")), nice)),
        // Read again only on this path, to tell EACCES from a kernel without autogroups.
        None => total(
            (Arc::from(""), 0),
            match read_at(dir, "autogroup", &mut bufs.small, SMALL_FILE_LIMIT) {
                Err(e) => optional_validity(&e),
                Ok(_) => Validity::Unsupported,
            },
        ),
    };

    // ELF hardening of the executable the process runs.
    let elf = rustix::fs::openat(dir, "exe", OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
        .map_err(CollectError::from)
        .and_then(|file| rd::read_elf(file.as_fd(), &mut bufs.small));
    let (pie, nx_stack, relro, architecture) = match elf {
        Ok(info) => (
            valid(info.pie),
            valid(info.nx_stack),
            valid(match info.relro {
                rd::Relro::Full => Relro::Full,
                rd::Relro::Partial => Relro::Partial,
                rd::Relro::None => Relro::None,
            }),
            valid(elf_arch(info.machine)),
        ),
        Err(CollectError::NotElf) => (
            total(false, Validity::Unsupported),
            total(false, Validity::Unsupported),
            total(Relro::None, Validity::Unsupported),
            valid(ElfArch::Unknown),
        ),
        Err(e) => {
            let v = optional_validity(&e);
            (
                total(false, v),
                total(false, v),
                total(Relro::None, v),
                total(ElfArch::Unknown, v),
            )
        }
    };
    let shadow_stack = match rd::shadow_stack(status.x86_thread_features.as_deref()) {
        ShadowStack::Enabled => valid(true),
        ShadowStack::Disabled => valid(false),
        ShadowStack::NotSupported => total(false, Validity::Unsupported),
    };
    let shadow_stack = from_status(shadow_stack, status_err);
    let aslr_system = match read_file(
        &root.proc.join("sys/kernel/randomize_va_space"),
        &mut bufs.small,
        SMALL_FILE_LIMIT,
    ) {
        Ok(_) => match bufs.small.trim_ascii() {
            b"0" => valid(0),
            b"1" => valid(1),
            b"2" => valid(2),
            _ => total(0, Validity::Unsupported),
        },
        Err(e) => total(0, optional_validity(&e)),
    };
    let aslr_disabled_for_process = match read_at(dir, "personality", &mut bufs.small, 64) {
        Ok(_) => std::str::from_utf8(bufs.small.trim_ascii())
            .ok()
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .map_or(total(false, Validity::Failed), |p| {
                valid(p & rd::ADDR_NO_RANDOMIZE != 0)
            }),
        Err(e) => total(false, error_validity(&e)),
    };
    let no_new_privs = match status.no_new_privs {
        Some(on) => valid(on),
        None => total(false, Validity::Unsupported),
    };
    let no_new_privs = from_status(no_new_privs, status_err);

    // The mtime of the file the process runs, also when it was deleted.
    let exe_mtime_s = rustix::fs::statx(dir, "exe", AtFlags::empty(), StatxFlags::MTIME)
        .ok()
        .map(|st| st.stx_mtime.tv_sec);

    // Executable path and its Location category.
    bufs.small.clear();
    bufs.small.resize(EXE_LINK_MAX + 1, 0);
    let (exe, exe_deleted, location) =
        match rustix::fs::readlinkat_raw(dir, "exe", &mut bufs.small[..]) {
            Ok(n) => {
                bufs.small.truncate(n);
                sanitize_link(&bufs.small, &mut bufs.text);
                let deleted = bufs.text.ends_with(DELETED_SUFFIX);
                let ctx = g.location.as_ref().expect("location context made above");
                let location = exe_location(rd::location(&bufs.text, deleted, ctx));
                (
                    valid(Arc::from(bufs.text.as_str())),
                    deleted,
                    valid(location),
                )
            }
            Err(e) => {
                let v = optional_validity(&CollectError::from(e));
                (total(Arc::from(""), v), false, total(ExeLocation::Other, v))
            }
        };

    let cwd = link_total(rd::read_cwd(dir, &mut bufs.small));
    let root_dir = link_total(rd::read_root(dir, &mut bufs.small));

    let cmdline = match read_at(dir, "cmdline", &mut bufs.cmdline, CMDLINE_MAX + 1) {
        Ok(_) => {
            let raw = bufs.cmdline.strip_suffix(b"\0").unwrap_or(&bufs.cmdline);
            let first = raw.split(|&b| b == 0).next().unwrap_or_default();
            sanitize_text(first, ARGV0_MAX, &mut g.argv0);
            sanitize_cmdline(&bufs.cmdline, &mut bufs.text);
            // Kernel threads have an empty command line.
            let validity = if bufs.text.is_empty() {
                Validity::Unsupported
            } else {
                Validity::Valid
            };
            total(Arc::from(bufs.text.as_str()), validity)
        }
        Err(e) => {
            g.argv0.clear();
            total(Arc::from(""), error_validity(&e))
        }
    };

    // Cgroup and systemd unit from the `0::` line. A file over the limit is not parsed.
    let (cgroup_path, cgroup_validity) =
        match read_at(dir, "cgroup", &mut bufs.small, SMALL_FILE_LIMIT) {
            Ok((_, false)) => (parse_cgroup_file(&bufs.small), Validity::Valid),
            Ok((_, true)) => (None, Validity::Failed),
            Err(e) => (None, optional_validity(&e)),
        };
    let cgroup_denied = cgroup_validity == Validity::Restricted;
    let (cgroup, unit) = match &cgroup_path {
        Some(path) => {
            let unit = path.unit.as_deref().map(|name| {
                unescape_unit(name, &mut bufs.text);
                UnitRef {
                    manager: match placement(path) {
                        UserPlacement::InUserManager { .. } => Manager::User,
                        _ => Manager::System,
                    },
                    name: Arc::from(bufs.text.as_str()),
                }
            });
            (
                valid(Some(Arc::from(format!("/{}", path.rel)))),
                valid(unit),
            )
        }
        None => (total(None, cgroup_validity), total(None, cgroup_validity)),
    };

    // App ID: the desktop match and App class of the process sweep's `AppResolver`. It needs
    // the real uid, so an unread `status` leaves it empty.
    let kernel_thread = stat.flags & PF_KTHREAD != 0;
    let apps = g.apps.get_or_insert_with(|| app_index(paths));
    if let Some(watch) = &mut apps.watch {
        apps.index.rebuild_if(watch, now);
    }
    let app_id = (status_err.is_none() && !kernel_thread)
        .then(|| {
            let unit_app = cgroup_path
                .as_ref()
                .and_then(|c| app_slice_unit(c, g.self_uid))
                .and_then(parse_app_unit);
            let m = MatchInput {
                unit_app: unit_app.as_ref(),
                exe: (exe.validity == Validity::Valid).then_some(&*exe.value),
                argv0: (!g.argv0.is_empty()).then_some(g.argv0.as_str()),
                cmdline: (cmdline.validity == Validity::Valid).then_some(&*cmdline.value),
                wm_class: None,
            };
            let entry = match_app(&m, &apps.index)?;
            let (class, _) = classify(
                &ClassifyInput {
                    pid: key.pid,
                    ruid: status.uid[0],
                    kernel_thread,
                    cgroup: cgroup_path.as_ref(),
                    cgroup_denied,
                    app_id: Some(&entry.id),
                    owns_window: false,
                },
                g.self_uid,
            );
            (class == ProcClass::App).then(|| entry.id.clone())
        })
        .flatten();

    let handles = match fd_count(dir) {
        FdCount::Count(n) => valid(n),
        FdCount::Restricted => total(0, Validity::Restricted),
        FdCount::Gone => total(0, Validity::Missing),
    };
    let io = match read_io(dir, &mut bufs.small) {
        Ok(IoRead::Ok(io)) => valid(io),
        Ok(IoRead::Restricted) => total(PidIo::default(), Validity::Restricted),
        Ok(IoRead::Gone) => total(PidIo::default(), Validity::Missing),
        Err(_) => total(PidIo::default(), Validity::Failed),
    };
    let bytes = |value: Option<u64>| value.map_or(total(0, Validity::Unsupported), valid);
    let cpu_ticks = stat.utime.saturating_add(stat.stime);

    Ok(GeneralRead {
        started_s,
        uids: from_status(valid(status.uid), status_err),
        policy: sched_policy(stat.policy),
        rt_prio: u8::try_from(stat.rt_priority).unwrap_or(u8::MAX),
        kernel_prio,
        autogroup,
        affinity,
        affinity_threads_differ,
        pie,
        nx_stack,
        relro,
        shadow_stack,
        aslr_system,
        aslr_disabled_for_process,
        no_new_privs,
        seccomp: from_status(
            seccomp_total(status.seccomp, status.seccomp_filters),
            status_err,
        ),
        architecture,
        exe,
        exe_deleted,
        exe_mtime_s,
        location,
        // `read_tab` sets it from the package index.
        package: total(None, Validity::Unsupported),
        cwd,
        root: root_dir,
        cgroup,
        unit,
        app_id,
        cmdline,
        handles,
        rss_shared_bytes: from_status(bytes(ws_shared_bytes(status)), status_err),
        commit_bytes: from_status(bytes(commit_bytes(status)), status_err),
        cpu_time_ms: valid(cpu_ticks.saturating_mul(1000) / ticks),
        io,
        // `read_tab` sets the target's hashes.
        hashes: HashState::Pending,
        package_check: PackageCheck::NotVerified,
    })
}

/// The General package of `read` from `index` (PROC-078). A path the index has not looked up
/// is queued and gives `WarmingUp`. No index gives `Unsupported`; so does a deleted executable,
/// whose path may now belong to another version. The lookup never waits for pacman.
fn package_of(
    index: Option<&PackageIndex>,
    read: &GeneralRead,
    now: Instant,
) -> Total<Option<PackageInfo>> {
    let Some(index) = index else {
        return total(None, Validity::Unsupported);
    };
    if read.exe.validity != Validity::Valid {
        return total(None, read.exe.validity);
    }
    if read.exe_deleted {
        return total(None, Validity::Unsupported);
    }
    let Some(mtime_s) = read.exe_mtime_s else {
        return total(None, Validity::Failed);
    };
    match index.get(&read.exe.value, mtime_s) {
        Some(Some(info)) => valid(Some(PackageInfo {
            name: Arc::clone(&info.name),
            version: Arc::clone(&info.version),
            packager: info.packager.clone(),
        })),
        Some(None) => valid(None),
        None => {
            index.request(&read.exe.value, mtime_s, now);
            total(None, Validity::WarmingUp)
        }
    }
}

/// The General payload of one read. `users` turns the uids into names.
fn general_detail(r: &GeneralRead, users: &mut dyn UserNames) -> GeneralDetail {
    let io = |value: fn(&PidIo) -> u64| total(value(&r.io.value), r.io.validity);
    GeneralDetail {
        started_s: r.started_s,
        uids: total(triple(r.uids.value), r.uids.validity),
        uid_names: match r.uids.validity {
            Validity::Valid => valid((
                users.name(r.uids.value[0]),
                users.name(r.uids.value[1]),
                users.name(r.uids.value[2]),
            )),
            v => total((Arc::from(""), Arc::from(""), Arc::from("")), v),
        },
        policy: valid(r.policy),
        rt_prio: r.rt_prio,
        kernel_prio: r.kernel_prio,
        autogroup: r.autogroup.clone(),
        affinity: r.affinity,
        affinity_threads_differ: r.affinity_threads_differ,
        pie: r.pie,
        nx_stack: r.nx_stack,
        relro: r.relro,
        shadow_stack: r.shadow_stack,
        aslr_system: r.aslr_system,
        aslr_disabled_for_process: r.aslr_disabled_for_process,
        no_new_privs: r.no_new_privs,
        seccomp: r.seccomp,
        architecture: r.architecture,
        exe: r.exe.clone(),
        exe_deleted: r.exe_deleted,
        location: r.location,
        package: r.package.clone(),
        description: total(Arc::from(""), Validity::Unsupported),
        cwd: r.cwd.clone(),
        root: r.root.clone(),
        cgroup: r.cgroup.clone(),
        unit: r.unit.clone(),
        app_id: r.app_id.clone(),
        cmdline: r.cmdline.clone(),
        hashes: r.hashes.clone(),
        package_check: r.package_check,
        handles: r.handles,
        rss_shared_bytes: r.rss_shared_bytes,
        commit_bytes: r.commit_bytes,
        cpu_time_ms: r.cpu_time_ms,
        io_rchar: io(|io| io.rchar),
        io_wchar: io(|io| io.wchar),
        io_syscr: io(|io| io.syscr),
        io_syscw: io(|io| io.syscw),
        disk_read_bytes: io(|io| io.read_bytes),
        disk_write_bytes: io(|io| io.write_bytes),
        cancelled_write_bytes: io(|io| io.cancelled_write_bytes),
        // GPU values are not read by this worker.
        gpu_mem_shared_bytes: total(0, Validity::Unsupported),
        gpu_engine: total(None, Validity::Unsupported),
    }
}

/// A General payload with no data: every field carries `validity`.
fn empty_general(validity: Validity) -> GeneralDetail {
    let text = || -> Total<Arc<str>> { total(Arc::from(""), validity) };
    let ids = IdTriple {
        real: 0,
        effective: 0,
        saved: 0,
        fs: 0,
    };
    GeneralDetail {
        started_s: total(0, validity),
        uids: total(ids, validity),
        uid_names: total((Arc::from(""), Arc::from(""), Arc::from("")), validity),
        policy: total(SchedPolicy::Other, validity),
        rt_prio: 0,
        kernel_prio: total(0, validity),
        autogroup: total((Arc::from(""), 0), validity),
        affinity: total(CpuSet::empty(), validity),
        affinity_threads_differ: false,
        pie: total(false, validity),
        nx_stack: total(false, validity),
        relro: total(Relro::None, validity),
        shadow_stack: total(false, validity),
        aslr_system: total(0, validity),
        aslr_disabled_for_process: total(false, validity),
        no_new_privs: total(false, validity),
        seccomp: total(Seccomp::Disabled, validity),
        architecture: total(ElfArch::Unknown, validity),
        exe: text(),
        exe_deleted: false,
        location: total(ExeLocation::Other, validity),
        package: total(None, validity),
        description: text(),
        cwd: text(),
        root: text(),
        cgroup: total(None, validity),
        unit: total(None, validity),
        app_id: None,
        cmdline: text(),
        hashes: HashState::Pending,
        package_check: PackageCheck::NotVerified,
        handles: total(0, validity),
        rss_shared_bytes: total(0, validity),
        commit_bytes: total(0, validity),
        cpu_time_ms: total(0, validity),
        io_rchar: total(0, validity),
        io_wchar: total(0, validity),
        io_syscr: total(0, validity),
        io_syscw: total(0, validity),
        disk_read_bytes: total(0, validity),
        disk_write_bytes: total(0, validity),
        cancelled_write_bytes: total(0, validity),
        gpu_mem_shared_bytes: total(0, validity),
        gpu_engine: total(None, validity),
    }
}
fn empty_security(validity: Validity) -> SecurityDetail {
    let ids = IdTriple {
        real: 0,
        effective: 0,
        saved: 0,
        fs: 0,
    };
    let names: [Arc<str>; 4] = std::array::from_fn(|_| Arc::from(""));
    SecurityDetail {
        uids: total(ids.clone(), validity),
        uid_names: total(names.clone(), validity),
        gids: total(ids, validity),
        gid_names: total(names, validity),
        elevated: total(Elevated::No, validity),
        no_new_privs: total(false, validity),
        seccomp: total(Seccomp::Disabled, validity),
        dumpable: total(true, validity),
        security_label: total(Arc::from(""), validity),
        session: None,
        audit_session: None,
        login_uid: None,
        namespaces: NAMESPACES.map(|(kind, _)| NsRow {
            kind,
            state: NsState::SharedWithInit,
            validity,
        }),
        store_bypass: total(Arc::from(""), validity),
        indirect_branch: total(Arc::from(""), validity),
        caps: Arc::from([]),
        groups: Arc::from([]),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use tl_collect::SysRoot;
    use tl_model::demand::ColumnSet;
    use tl_model::details::HandlePhase;
    use tl_model::time::BootId;

    use super::*;
    use crate::config::EngineMode;

    const STAT: &[u8] =
        include_bytes!("../../../tl-collect/tests/fixtures/process-edge/proc/4201/stat");
    const ROLLUP: &[u8] =
        include_bytes!("../../../tl-collect/tests/fixtures/process-edge/proc/4201/smaps_rollup");
    /// start_ticks of process-edge 4201.
    const START_TICKS: u64 = 351_220;
    const MS: TimeNs = 1_000_000;

    fn key(pid: u32, start_ticks: u64) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BootId(1),
            pid,
        }
    }

    /// A proc root with `n` processes (pids 10000..) that all have the stat and smaps_rollup of
    /// process-edge 4201.
    fn fake_procs(n: u32) -> (tempfile::TempDir, Arc<EngineConfig>, Vec<ProcessKey>) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut keys = Vec::new();
        for pid in 10_000..10_000 + n {
            let dir = tmp.path().join(format!("proc/{pid}"));
            fs::create_dir_all(&dir).expect("create proc dir");
            fs::write(dir.join("stat"), STAT).expect("write stat");
            fs::write(dir.join("smaps_rollup"), ROLLUP).expect("write smaps_rollup");
            keys.push(key(pid, START_TICKS));
        }
        let cfg = EngineConfig::new(EngineMode::Full, PathBuf::from("/nonexistent"))
            .with_sys_root(SysRoot::at(tmp.path()));
        (tmp, Arc::new(cfg), keys)
    }

    fn demand(columns: &[ColumnId], keys: &[ProcessKey]) -> Demand {
        let mut set = ColumnSet::default();
        for &c in columns {
            set.insert(c);
        }
        Demand {
            process_columns: set,
            detail_keys: keys.into(),
            ..Demand::default()
        }
    }

    fn entries(update: SourceUpdate) -> Vec<PssEntry> {
        match update {
            SourceUpdate::Details(DetailsUpdate::Pss { entries, .. }) => entries,
            other => panic!("expected a Pss update, got {other:?}"),
        }
    }

    /// `period`, then a sample 1 s after the last PSS read, as the source loop does at 1 Hz.
    fn run(src: ProcessDetailSource, d: &Demand) -> (ProcessDetailSource, Vec<PssEntry>) {
        src.period(d, &EngineSettings::default());
        let now = src.last_pss.map_or(1, |at| at + nanos(PSS_PERIOD));
        let (src, update) = src.sample_at(now);
        (src, entries(update))
    }

    #[test]
    fn command_routing() {
        let (_tmp, cfg, keys) = fake_procs(1);
        let mut src = ProcessDetailSource::new(cfg);
        // ComputeHashes is handled, and the same key is held once.
        assert!(src.command(&WorkerCommand::ComputeHashes(keys[0])));
        assert!(src.command(&WorkerCommand::ComputeHashes(keys[0])));
        assert_eq!(src.hash_requests, vec![keys[0]]);
        // A scan is queued for the next sample; a cancel of no scan has nothing to do.
        assert!(src.command(&WorkerCommand::StringScan {
            scan: ScanId(1),
            key: keys[0],
            opts: StringScanOpts::default(),
        }));
        assert_eq!(src.scan_requests.len(), 1);
        assert!(!src.command(&WorkerCommand::CancelStringScan(ScanId(2))));
        // The same for a handle search.
        assert!(!src.command(&WorkerCommand::CancelHandleSearch(SearchId(1))));
        assert!(src.command(&WorkerCommand::HandleSearch {
            search: SearchId(1),
            pattern: "x".into(),
            include_maps: false,
            privileged: false,
        }));
        assert!(src.search_request.is_some());
        assert!(!src.command(&WorkerCommand::CancelHandleSearch(SearchId(2))));
        assert!(src.command(&WorkerCommand::VerifyPackage(keys[0])));
        assert_eq!(src.verify_requests, vec![keys[0]]);
        assert!(!src.command(&WorkerCommand::RefreshNow));
        assert!(!src.command(&WorkerCommand::Reprobe));
        assert!(!src.command(&WorkerCommand::SetupStatus));
        // DetailRefresh is handled, not queued.
        assert!(src.command(&WorkerCommand::DetailRefresh {
            target: DetailTarget::Process(keys[0]),
            tab: DetailTab::Environment,
        }));
        assert_eq!(src.refresh.len(), 1);
        // The sample starts the search. Its pids have no `fd` dir, so it ends in one step.
        let (src, _) = src.sample();
        assert!(src.search_request.is_none());
        assert!(src.search.is_none());
    }

    /// tasklens-dwy.3: no search and no other demand gives no period. A search sets
    /// `SCAN_PERIOD` until the sample that publishes its `Done` state.
    #[test]
    fn handle_search_period() {
        let (_tmp, cfg, _keys) = fake_procs(1);
        let mut src = ProcessDetailSource::new(cfg);
        let d = Demand::default();
        let s = EngineSettings::default();
        assert_eq!(src.period(&d, &s), None);

        assert!(src.command(&WorkerCommand::HandleSearch {
            search: SearchId(7),
            pattern: "*.log".into(),
            include_maps: true,
            privileged: false,
        }));
        assert_eq!(src.period(&d, &s), Some(SCAN_PERIOD));

        let mut phases = Vec::new();
        for i in 1..=10 {
            let (next, update) = src.sample_at(i * 50 * MS);
            src = next;
            let SourceUpdate::Details(DetailsUpdate::HandleSearch(state)) = update else {
                panic!("expected a HandleSearch update, got {update:?}");
            };
            assert_eq!(state.search, SearchId(7));
            assert!(src.take_extra().is_none(), "one update per sample");
            phases.push(state.phase);
            if state.phase == HandlePhase::Done {
                break;
            }
            assert_eq!(src.period(&d, &s), Some(SCAN_PERIOD));
        }
        assert_eq!(phases.last(), Some(&HandlePhase::Done));
        assert_eq!(src.period(&d, &s), None);
        // No search: a sample queues no state.
        let (mut src, update) = src.sample_at(11 * 50 * MS);
        assert!(matches!(update, SourceUpdate::Health { .. }), "{update:?}");
        assert!(src.take_extra().is_none());
    }

    /// tasklens-dwy.9: with more scans than `SCAN_STEPS_PER_SAMPLE` (10 scans, 8 reads), each
    /// sample goes on where the last one stopped, so after two samples every scan has read.
    #[test]
    fn string_scans_take_turns() {
        const LEN: u64 = 16 << 20;
        let (tmp, cfg, keys) = fake_procs(10);
        for k in &keys {
            let dir = tmp.path().join(format!("proc/{}", k.pid));
            let maps = format!("00001000-{:08x} rw-p 00000000 00:00 0\n", 0x1000 + LEN);
            fs::write(dir.join("maps"), maps).expect("write maps");
            let mem = fs::File::create(dir.join("mem")).expect("create mem");
            mem.set_len(0x1000 + LEN).expect("size mem");
        }
        let d = tabs(
            &keys
                .iter()
                .map(|&k| (k, DetailTab::Strings))
                .collect::<Vec<_>>(),
        );
        let mut src = ProcessDetailSource::new(cfg);
        for (n, &key) in (1..).zip(&keys) {
            assert!(src.command(&WorkerCommand::StringScan {
                scan: ScanId(n),
                key,
                opts: StringScanOpts::default(),
            }));
        }
        let mut now = 1_000 * MS;
        for _ in 0..2 {
            src.period(&d, &every_ms(500));
            (src, _) = src.sample_at(now);
            while src.take_extra().is_some() {}
            now += 50 * MS;
        }
        assert_eq!(src.scans.len(), 10);
        let pcts: Vec<u8> = src.scans.iter().map(|s| s.scanner.progress_pct()).collect();
        assert!(pcts.iter().all(|&p| p > 0), "{pcts:?}");
    }

    /// At most `DETAILS_MAX` hash requests wait; the oldest goes first.
    #[test]
    fn hash_requests_capped() {
        let (_tmp, cfg, keys) = fake_procs(DETAILS_MAX as u32 + 1);
        let mut src = ProcessDetailSource::new(cfg);
        for &k in &keys {
            assert!(src.command(&WorkerCommand::ComputeHashes(k)));
        }
        assert_eq!(src.hash_requests.len(), DETAILS_MAX);
        assert_eq!(src.hash_requests[0], keys[1]);
    }

    #[test]
    fn pss_only_when_column_shown() {
        let (_tmp, cfg, keys) = fake_procs(3);
        let src = ProcessDetailSource::new(cfg);
        let s = EngineSettings::default();
        assert_eq!(src.period(&demand(&[], &keys), &s), None);
        assert_eq!(
            src.period(&demand(&[ColumnId::Pss], &keys), &s),
            Some(PSS_PERIOD)
        );
        assert_eq!(
            src.period(&demand(&[ColumnId::Uss], &keys), &s),
            Some(Duration::from_secs(1))
        );
        assert_eq!(src.period(&demand(&[ColumnId::Pss], &[]), &s), None);
    }

    #[test]
    fn hidden_columns_clear_keys_for_forced_sample() {
        let (_tmp, cfg, keys) = fake_procs(3);
        let (src, out) = run(
            ProcessDetailSource::new(cfg),
            &demand(&[ColumnId::Pss], &keys),
        );
        assert_eq!(out.len(), 3);
        let (src, out) = run(src, &demand(&[], &keys));
        assert!(out.is_empty());
        assert_eq!(src.cached_fds(), 0);
    }

    #[test]
    fn pss_values_from_fixture() {
        let (_tmp, cfg, keys) = fake_procs(3);
        let (_src, out) = run(
            ProcessDetailSource::new(cfg),
            &demand(&[ColumnId::Pss], &keys),
        );
        assert_eq!(out.len(), 3);
        for e in &out {
            assert_eq!(e.validity, Validity::Valid);
            assert_eq!(e.pss_bytes, 2_097_152);
            assert_eq!(e.uss_bytes, 1_572_864);
            assert_eq!(e.swap_bytes, 0);
        }
    }

    #[test]
    fn pss_restricted_other_user() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root");
            return;
        }
        let (tmp, cfg, keys) = fake_procs(2);
        let path = tmp.path().join("proc/10001/smaps_rollup");
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("chmod");
        let (_src, out) = run(
            ProcessDetailSource::new(cfg),
            &demand(&[ColumnId::Pss], &keys),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].validity, Validity::Valid);
        assert_eq!(out[1].key, keys[1]);
        assert_eq!(out[1].validity, Validity::Restricted);
    }

    #[test]
    fn fd_cache_bounded_128() {
        let (_tmp, cfg, keys) = fake_procs(200);
        let d = demand(&[ColumnId::Pss], &keys);
        let (src, out) = run(ProcessDetailSource::new(cfg), &d);
        assert_eq!(out.len(), 200);
        assert_eq!(src.cached_fds(), 128);
        let (src, _) = run(src, &d);
        assert_eq!(src.cached_fds(), 128);
        let (src, _) = run(src, &demand(&[ColumnId::Pss], &keys[150..]));
        assert_eq!(src.cached_fds(), 50);
    }

    #[test]
    fn pss_identity_mismatch_skipped() {
        let (_tmp, cfg, keys) = fake_procs(1);
        let (src, out) = run(
            ProcessDetailSource::new(cfg),
            &demand(&[ColumnId::Pss], &keys),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(src.cached_fds(), 1);
        let stale = key(keys[0].pid, START_TICKS - 1);
        let (src, out) = run(src, &demand(&[ColumnId::Pss], &[stale]));
        assert!(out.is_empty());
        assert_eq!(src.cached_fds(), 0);
    }

    // ---- Properties tabs (E10-004) ----

    /// Copies every entry of `src` to `dst`, keeping symlinks as links.
    fn copy_tree(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).expect("create fixture dir");
        for entry in fs::read_dir(src).expect("list fixture dir") {
            let entry = entry.expect("fixture dir entry");
            let to = dst.join(entry.file_name());
            let file_type = entry.file_type().expect("fixture file type");
            if file_type.is_symlink() {
                let target = fs::read_link(entry.path()).expect("read fixture symlink");
                std::os::unix::fs::symlink(target, &to).expect("create fixture symlink");
            } else if file_type.is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                fs::copy(entry.path(), &to).expect("copy fixture file");
            }
        }
    }

    /// A writable copy of the tl-collect `process-edge` fixture.
    fn edge_root() -> (tempfile::TempDir, Arc<EngineConfig>) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/process-edge");
        copy_tree(&src, tmp.path());
        let cfg = EngineConfig::new(EngineMode::Full, PathBuf::from("/nonexistent"))
            .with_sys_root(SysRoot::at(tmp.path()));
        (tmp, Arc::new(cfg))
    }

    /// process-edge 4201.
    fn a() -> ProcessKey {
        key(4201, START_TICKS)
    }

    fn tabs(list: &[(ProcessKey, DetailTab)]) -> Demand {
        let details: Vec<DetailDemand> = list
            .iter()
            .map(|&(k, tab)| DetailDemand {
                target: DetailTarget::Process(k),
                tab,
            })
            .collect();
        Demand {
            details: details.into(),
            ..Demand::default()
        }
    }

    fn every_ms(ms: u32) -> EngineSettings {
        EngineSettings {
            interval_ms: ms,
            ..EngineSettings::default()
        }
    }

    type TabOut = (DetailTab, Arc<TabPayload>, Validity);

    /// `step_all` without the General updates, which go out beside every demanded tab.
    fn step(
        src: ProcessDetailSource,
        d: &Demand,
        s: &EngineSettings,
        now_ms: u64,
    ) -> (ProcessDetailSource, Vec<TabOut>, SourceUpdate) {
        let (src, mut out, update) = step_all(src, d, s, now_ms);
        out.retain(|(tab, _, _)| *tab != DetailTab::General);
        (src, out, update)
    }

    /// `period`, `sample_at(now_ms)`, then every update the loop would send: the tab updates
    /// in order and the sample's own update.
    fn step_all(
        src: ProcessDetailSource,
        d: &Demand,
        s: &EngineSettings,
        now_ms: u64,
    ) -> (ProcessDetailSource, Vec<TabOut>, SourceUpdate) {
        src.period(d, s);
        let (mut src, update) = src.sample_at(now_ms * MS + 1);
        let mut out = Vec::new();
        let mut push = |u: &SourceUpdate| {
            if let SourceUpdate::Details(DetailsUpdate::Tab {
                tab,
                payload,
                validity,
                ..
            }) = u
            {
                out.push((*tab, Arc::clone(payload), *validity));
            }
        };
        while let Some(extra) = src.take_extra() {
            push(&extra);
        }
        push(&update);
        (src, out, update)
    }

    fn env_vars(p: &TabPayload) -> Vec<(String, String)> {
        match p {
            TabPayload::Environ(e) => e
                .vars
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            other => panic!("expected Environ, got {other:?}"),
        }
    }

    /// E10-004 AC 1: the demanded tab and General are read at the interval.
    #[test]
    fn tabs_read_at_interval_plus_general() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Threads)]);
        let s = every_ms(500);
        let mut src = ProcessDetailSource::new(cfg);
        assert_eq!(src.period(&d, &s), Some(Duration::from_millis(500)));
        for t in 0..4 {
            let (next, _, _) = step(src, &d, &s, t * 500);
            src = next;
        }
        for tab in DetailTab::ALL {
            let want = if matches!(tab, DetailTab::Threads | DetailTab::General) {
                4
            } else {
                0
            };
            assert_eq!(src.tab_reads(tab), want, "{tab:?}");
        }
    }

    /// With PSS shown the grid is 1 s, and a 2 s interval reads the tab every other sample.
    #[test]
    fn tabs_keep_interval_beside_pss() {
        let (_tmp, cfg) = edge_root();
        let mut d = tabs(&[(a(), DetailTab::Threads)]);
        d.process_columns.insert(ColumnId::Pss);
        d.detail_keys = Arc::from([a()]);
        let s = every_ms(2000);
        let mut src = ProcessDetailSource::new(cfg);
        assert_eq!(src.period(&d, &s), Some(PSS_PERIOD));
        for t in 0..4 {
            let (next, _, update) = step(src, &d, &s, t * 1000);
            assert_eq!(entries(update).len(), 1);
            src = next;
        }
        assert_eq!(src.tab_reads(DetailTab::Threads), 2);
    }

    /// A 1500 ms interval beside PSS splits into two 750 ms samples: tabs read every second
    /// sample, exactly at 1500 ms, and PSS every second sample too.
    #[test]
    fn tabs_keep_1500_ms_beside_pss() {
        let (_tmp, cfg) = edge_root();
        let mut d = tabs(&[(a(), DetailTab::Threads)]);
        d.process_columns.insert(ColumnId::Pss);
        d.detail_keys = Arc::from([a()]);
        let s = every_ms(1500);
        let mut src = ProcessDetailSource::new(cfg);
        assert_eq!(src.period(&d, &s), Some(Duration::from_millis(750)));
        let mut pss_reads = 0;
        for t in 0..4 {
            let (next, _, update) = step(src, &d, &s, t * 750);
            if matches!(update, SourceUpdate::Details(DetailsUpdate::Pss { .. })) {
                pss_reads += 1;
            }
            src = next;
        }
        assert_eq!(pss_reads, 2);
        assert_eq!(src.tab_reads(DetailTab::Threads), 2);
    }

    /// With PSS and Memory maps both on, each interval gives a period that divides it, tabs
    /// read exactly at the interval and `smaps` never reads under 1 s apart.
    #[test]
    fn shared_period_keeps_each_rate() {
        let (_tmp, cfg) = edge_root();
        let mut d = tabs(&[(a(), DetailTab::Maps)]);
        d.process_columns.insert(ColumnId::Pss);
        d.detail_keys = Arc::from([a()]);
        // Interval, period in ns, samples between tab reads, between `smaps` reads, between
        // PSS reads.
        for (interval, period_ns, tab_gap, smaps_gap, pss_gap) in [
            (500, 500_000_000, 1, 2, 2),
            (1000, 1_000_000_000, 1, 1, 1),
            (1100, 550_000_000, 2, 2, 2),
            (1500, 750_000_000, 2, 2, 2),
            (2500, 833_333_333, 3, 3, 2),
        ] {
            let s = every_ms(interval);
            let mut src = ProcessDetailSource::new(Arc::clone(&cfg));
            let period = src.period(&d, &s).expect("a period");
            assert_eq!(period, Duration::from_nanos(period_ns), "{interval}");
            let (mut tab_at, mut smaps_at, mut pss_at) = (Vec::new(), Vec::new(), Vec::new());
            for n in 0..12u64 {
                let (tab_before, smaps_before) =
                    (src.tab_reads(DetailTab::Maps), src.smaps_reads());
                src.period(&d, &s);
                let (next, update) = src.sample_at(n * period_ns + 1);
                src = next;
                while src.take_extra().is_some() {}
                if src.tab_reads(DetailTab::Maps) > tab_before {
                    tab_at.push(n);
                }
                if src.smaps_reads() > smaps_before {
                    smaps_at.push(n);
                }
                if matches!(update, SourceUpdate::Details(DetailsUpdate::Pss { .. })) {
                    pss_at.push(n);
                }
            }
            let gaps = |at: &[u64]| -> Vec<u64> { at.windows(2).map(|w| w[1] - w[0]).collect() };
            assert!(
                gaps(&tab_at).iter().all(|&g| g == tab_gap),
                "{interval}: {tab_at:?}"
            );
            assert!(
                gaps(&smaps_at).iter().all(|&g| g == smaps_gap),
                "{interval}: {smaps_at:?}"
            );
            assert!(
                gaps(&pss_at).iter().all(|&g| g == pss_gap),
                "{interval}: {pss_at:?}"
            );
            assert!(smaps_at.len() >= 2 && pss_at.len() >= 2, "{interval}");
            // Tabs read at the interval, to the nanosecond the division drops.
            let tab_ns = tab_gap * period_ns;
            let interval_ns = u64::from(interval) * 1_000_000;
            assert!(interval_ns - tab_ns < tab_gap, "{interval}: {tab_ns}");
            assert!(smaps_gap * period_ns >= 1_000_000_000, "{interval}");
            assert!(pss_gap * period_ns >= 1_000_000_000, "{interval}");
        }
    }

    /// E10-004 AC 2.
    #[test]
    fn smaps_capped_1hz() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Maps)]);
        let s = every_ms(250);
        let mut src = ProcessDetailSource::new(cfg);
        let mut last = None;
        for t in 0..8 {
            let (next, out, _) = step(src, &d, &s, t * 250);
            src = next;
            last = out.into_iter().next().or(last);
        }
        assert_eq!(src.tab_reads(DetailTab::Maps), 8);
        assert_eq!(src.smaps_reads(), 2);
        let (_, payload, validity) = last.expect("a Maps payload");
        assert_eq!(validity, Validity::Valid);
        let TabPayload::Maps(maps) = &*payload else {
            panic!("expected Maps");
        };
        let heap = maps
            .regions
            .iter()
            .find(|r| r.label.as_deref() == Some("[heap]"))
            .expect("heap region");
        assert_eq!(heap.rss_bytes.validity, Validity::Valid);
        assert!(
            maps.regions
                .iter()
                .any(|r| r.label.as_deref() == Some("anonymous"))
        );
        let lib = maps
            .nodes
            .iter()
            .find(|n| &*n.name == "libexample.so.1")
            .expect("library node");
        assert!(lib.shared_object);
        assert_eq!(lib.size_bytes, 0x20_0000);
    }

    /// A period change starts a new sample grid with a sample at once. The time sampled on the
    /// old grid still counts, so `smaps` does not read again under 1 s after its last read.
    #[test]
    fn smaps_cap_holds_across_period_change() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Maps)]);
        let mut src = ProcessDetailSource::new(cfg);
        // 250 ms samples at 0, 250, 500 and 750 ms: `smaps` reads at 0 only.
        let fast = every_ms(250);
        for t in 0..4 {
            let (next, _, _) = step(src, &d, &fast, t * 250);
            src = next;
        }
        assert_eq!(src.smaps_reads(), 1);
        // The interval goes to 1 s. The loop samples at once, still at 750 ms, then at 1750 ms.
        let slow = every_ms(1000);
        let (src, _, _) = step(src, &d, &slow, 750);
        assert_eq!(src.tab_reads(DetailTab::Maps), 5);
        assert_eq!(src.smaps_reads(), 1);
        let (src, _, _) = step(src, &d, &slow, 1750);
        assert_eq!(src.tab_reads(DetailTab::Maps), 6);
        assert_eq!(src.smaps_reads(), 2);
    }

    /// A late first sample does not let `smaps` read again under 1 s later, and neither does
    /// a tab or a window that closes and opens again.
    #[test]
    fn smaps_cap_holds_for_late_samples() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Maps)]);
        let s = every_ms(250);
        let mut src = ProcessDetailSource::new(cfg);
        // The 0 ms sample came at 100 ms.
        for t in [100, 250, 500, 750, 1000] {
            let (next, _, _) = step(src, &d, &s, t);
            src = next;
        }
        assert_eq!(src.tab_reads(DetailTab::Maps), 5);
        assert_eq!(src.smaps_reads(), 1);
        let (next, _, _) = step(src, &d, &s, 1100);
        src = next;
        assert_eq!(src.smaps_reads(), 2);
        // The window closes and opens again.
        assert_eq!(src.period(&Demand::default(), &s), None);
        let (src, _, _) = step(src, &d, &s, 1600);
        assert_eq!(src.tab_reads(DetailTab::Maps), 7);
        assert_eq!(src.smaps_reads(), 2);
        let (src, _, _) = step(src, &d, &s, 2100);
        assert_eq!(src.smaps_reads(), 3);
    }

    /// A period change samples at once. PSS read just before it does not read again under 1 s.
    #[test]
    fn pss_cap_holds_across_period_change() {
        let (_tmp, cfg) = edge_root();
        let mut pss_only = Demand::default();
        pss_only.process_columns.insert(ColumnId::Pss);
        pss_only.detail_keys = Arc::from([a()]);
        let mut with_tab = tabs(&[(a(), DetailTab::Threads)]);
        with_tab.process_columns.insert(ColumnId::Pss);
        with_tab.detail_keys = Arc::from([a()]);
        let s = every_ms(500);
        let src = ProcessDetailSource::new(cfg);
        assert_eq!(src.period(&pss_only, &s), Some(PSS_PERIOD));
        let (src, _, update) = step(src, &pss_only, &s, 0);
        assert_eq!(entries(update).len(), 1);
        // A tab opens: the period goes to 500 ms and the loop samples at once.
        assert_eq!(src.period(&with_tab, &s), Some(Duration::from_millis(500)));
        let mut pss_at = Vec::new();
        let mut src = src;
        for t in [10, 510, 1010, 1510, 2010] {
            let (next, _, update) = step(src, &with_tab, &s, t);
            src = next;
            if matches!(update, SourceUpdate::Details(DetailsUpdate::Pss { .. })) {
                pss_at.push(t);
            }
        }
        assert_eq!(pss_at, vec![1010, 2010]);
        // Back to PSS alone, at once after a read.
        let (_src, _, update) = step(src, &pss_only, &s, 2020);
        assert!(!matches!(
            update,
            SourceUpdate::Details(DetailsUpdate::Pss { .. })
        ));
    }

    /// Between `smaps` reads the RSS of the last read is kept, so the tab does not flicker.
    #[test]
    fn maps_keep_rss_between_smaps_reads() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Maps)]);
        let s = every_ms(250);
        let (src, first, _) = step(ProcessDetailSource::new(cfg), &d, &s, 0);
        assert_eq!(first.len(), 1);
        let (src, second, _) = step(src, &d, &s, 250);
        assert!(second.is_empty(), "same maps and RSS send nothing");
        assert_eq!(src.smaps_reads(), 1);
    }

    /// A source that already saw a demand without tabs, as a worker that ran before the window
    /// opened. Its next tabs are marked `open_pending`.
    fn running(cfg: Arc<EngineConfig>) -> ProcessDetailSource {
        let src = ProcessDetailSource::new(cfg);
        assert_eq!(src.period(&Demand::default(), &every_ms(500)), None);
        src
    }

    /// The `DetailOpened` the reducer sends for each tab that opens.
    fn opened(src: &mut ProcessDetailSource, tab: DetailTab) {
        assert!(src.command(&WorkerCommand::DetailOpened {
            target: DetailTarget::Process(a()),
            tab,
        }));
    }

    /// A user `DetailRefresh`.
    fn user_refresh(src: &mut ProcessDetailSource, tab: DetailTab) {
        assert!(src.command(&WorkerCommand::DetailRefresh {
            target: DetailTarget::Process(a()),
            tab,
        }));
    }

    /// E10-004 AC 3.
    #[test]
    fn environment_once_then_on_refresh() {
        let (tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let mut src = running(cfg);
        let (next, out, _) = step(src, &d, &s, 0);
        src = next;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2, Validity::Valid);
        assert!(env_vars(&out[0].1).contains(&("API_KEY".into(), "abc".into())));
        // The reducer's refresh for this open, after the read: the payload again, no read.
        opened(&mut src, DetailTab::Environment);
        let (next, out, _) = step(src, &d, &s, 250);
        src = next;
        assert_eq!(out.len(), 1);
        assert_eq!(src.tab_reads(DetailTab::Environment), 1);

        let environ = tmp.path().join("proc/4201/environ");
        fs::write(&environ, b"NEW=1\0").expect("rewrite environ");
        for t in 1..10 {
            let (next, out, _) = step(src, &d, &s, t * 500);
            src = next;
            assert!(out.is_empty());
        }
        assert_eq!(src.tab_reads(DetailTab::Environment), 1);

        user_refresh(&mut src, DetailTab::Environment);
        let (src, out, _) = step(src, &d, &s, 5000);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
        assert_eq!(env_vars(&out[0].1), vec![("NEW".into(), "1".into())]);
        let (src, _, _) = step(src, &d, &s, 5500);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
    }

    fn fixed_clock() -> (TimeNs, u64) {
        (1_000_000_000, 1_000_000_000)
    }

    /// E10-004 AC 4.
    #[test]
    fn unchanged_tab_reuses_arc() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Threads)]);
        let s = every_ms(500);
        let mut src = ProcessDetailSource::new(cfg);
        src.thread_clock = Some(fixed_clock);
        let (src, first, _) = step(src, &d, &s, 0);
        assert_eq!(first.len(), 1);
        let TabPayload::Threads(rows) = &*first[0].1 else {
            panic!("expected Threads");
        };
        let tids: Vec<u32> = rows.iter().map(|r| r.tid).collect();
        assert_eq!(tids, vec![4201, 4211]);
        let (src, second, update) = step(src, &d, &s, 500);
        assert!(second.is_empty());
        assert!(matches!(
            update,
            SourceUpdate::Health {
                state: SourceState::Running,
                ..
            }
        ));
        assert_eq!(src.tab_reads(DetailTab::Threads), 2);
    }

    /// E10-004 AC 5.
    #[test]
    fn restricted_without_helper() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root");
            return;
        }
        let (tmp, cfg) = edge_root();
        let environ = tmp.path().join("proc/4201/environ");
        fs::set_permissions(environ, fs::Permissions::from_mode(0o000)).expect("chmod");
        let d = tabs(&[(a(), DetailTab::Environment)]);
        let (_src, out, _) = step(ProcessDetailSource::new(cfg), &d, &every_ms(500), 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2, Validity::Restricted);
        assert!(env_vars(&out[0].1).is_empty());
    }

    /// E10-004 AC 7.
    #[test]
    fn details_cap_16() {
        let (_tmp, cfg) = edge_root();
        let list: Vec<(ProcessKey, DetailTab)> = (0..17)
            .map(|i| (key(20_000 + i, 1), DetailTab::Fds))
            .collect();
        let d = tabs(&list);
        let s = every_ms(500);
        let src = ProcessDetailSource::new(cfg);
        src.period(&d, &s);
        src.period(&d, &s);
        assert_eq!(src.cap_warnings.get(), 1);
        assert_eq!(src.details.borrow().len(), DETAILS_MAX);
        let (src, out, _) = step(src, &d, &s, 0);
        assert_eq!(src.targets_len(), 16);
        assert_eq!(src.tab_reads(DetailTab::Fds), 16);
        assert!(out.iter().all(|(_, _, v)| *v == Validity::Missing));
        // Back under the cap and over it again warns again.
        src.period(&tabs(&list[..3]), &s);
        src.period(&d, &s);
        assert_eq!(src.cap_warnings.get(), 2);
    }

    #[test]
    fn targets_leaving_demand_are_forgotten() {
        let (_tmp, cfg) = edge_root();
        let s = every_ms(500);
        let (src, _, _) = step(
            ProcessDetailSource::new(cfg),
            &tabs(&[(a(), DetailTab::Fds)]),
            &s,
            0,
        );
        assert_eq!(src.targets_len(), 1);
        let (src, _, _) = step(src, &Demand::default(), &s, 500);
        assert_eq!(src.targets_len(), 0);
    }

    /// With no demand left no sample runs, so `period` alone forgets the target. A window that
    /// opens again gets its unchanged tabs and a new Environment read.
    #[test]
    fn reopened_window_gets_its_tabs_again() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Threads), (a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let mut src = ProcessDetailSource::new(cfg);
        src.thread_clock = Some(fixed_clock);
        let (src, first, _) = step(src, &d, &s, 0);
        assert_eq!(first.len(), 2);
        assert_eq!(src.period(&Demand::default(), &s), None);
        assert_eq!(src.targets_len(), 0);
        let (src, again, _) = step(src, &d, &s, 500);
        let tabs_sent: Vec<DetailTab> = again.iter().map(|(tab, _, _)| *tab).collect();
        assert_eq!(tabs_sent, vec![DetailTab::Threads, DetailTab::Environment]);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
    }

    /// A tab that closed and opened again between two polls keeps its state here, but the
    /// reducer dropped its payload. The `DetailRefresh` the reducer then sends makes an
    /// unchanged tab go out again, and rereads Environment once.
    #[test]
    fn refresh_resends_unchanged_tab() {
        let (tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Fds), (a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let (mut src, first, _) = step(running(cfg), &d, &s, 0);
        assert_eq!(first.len(), 2);
        // The reducer's refreshes for the first open resend the same payloads.
        for tab in [DetailTab::Fds, DetailTab::Environment] {
            opened(&mut src, tab);
        }
        let (src, resent, _) = step(src, &d, &s, 500);
        assert_eq!(resent.len(), 2);
        assert!(Arc::ptr_eq(&first[0].1, &resent[0].1));
        assert!(Arc::ptr_eq(&first[1].1, &resent[1].1));
        fs::write(tmp.path().join("proc/4201/environ"), b"NEW=1\0").expect("rewrite environ");
        let (mut src, same, _) = step(src, &d, &s, 1000);
        assert!(
            same.is_empty(),
            "unchanged Fds and a loaded Environment send nothing"
        );
        // Closed and opened again between two polls: the reducer sends a refresh again.
        for tab in [DetailTab::Fds, DetailTab::Environment] {
            opened(&mut src, tab);
        }
        let (src, again, _) = step(src, &d, &s, 1500);
        let sent: Vec<DetailTab> = again.iter().map(|(tab, _, _)| *tab).collect();
        assert_eq!(sent, vec![DetailTab::Fds, DetailTab::Environment]);
        assert!(!Arc::ptr_eq(&first[0].1, &again[0].1));
        assert_eq!(env_vars(&again[1].1), vec![("NEW".into(), "1".into())]);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
        let (src, _, _) = step(src, &d, &s, 2000);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
    }

    /// The reducer's refresh can come before this worker sees the tab in the demand. The first
    /// read serves it, and a later user refresh still reads again.
    #[test]
    fn refresh_before_demand_reads_once() {
        let (tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let mut src = ProcessDetailSource::new(cfg);
        opened(&mut src, DetailTab::Environment);
        let (mut src, out, _) = step(src, &d, &s, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(src.tab_reads(DetailTab::Environment), 1);
        fs::write(tmp.path().join("proc/4201/environ"), b"NEW=1\0").expect("rewrite environ");
        user_refresh(&mut src, DetailTab::Environment);
        let (src, out, _) = step(src, &d, &s, 500);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
        assert_eq!(env_vars(&out[0].1), vec![("NEW".into(), "1".into())]);
    }

    /// A tab that the worker sees close and open again is read once for the new open, and the
    /// reducer's refresh for it reads nothing more.
    #[test]
    fn tab_switch_reads_environment_once_per_open() {
        let (_tmp, cfg) = edge_root();
        let env = tabs(&[(a(), DetailTab::Environment)]);
        let threads = tabs(&[(a(), DetailTab::Threads)]);
        let s = every_ms(500);
        let mut src = running(cfg);
        src.thread_clock = Some(fixed_clock);
        let (mut src, _, _) = step(src, &env, &s, 0);
        opened(&mut src, DetailTab::Environment);
        let (src, _, _) = step(src, &env, &s, 500);
        let (src, _, _) = step(src, &threads, &s, 1000);
        let (mut src, out, _) = step(src, &env, &s, 1500);
        assert_eq!(out.len(), 1);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
        opened(&mut src, DetailTab::Environment);
        let (src, out, _) = step(src, &env, &s, 2000);
        assert_eq!(out.len(), 1);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
    }

    /// A restarted worker finds the Environment tab open in its first demand. The reducer sends
    /// no open notice for a tab it already had open, so the next user refresh reads again.
    #[test]
    fn restart_with_open_tab_keeps_user_refresh() {
        let (tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let (mut src, out, _) = step(ProcessDetailSource::new(cfg), &d, &s, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(src.tab_reads(DetailTab::Environment), 1);
        fs::write(tmp.path().join("proc/4201/environ"), b"NEW=1\0").expect("rewrite environ");
        user_refresh(&mut src, DetailTab::Environment);
        let (src, out, _) = step(src, &d, &s, 500);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
        assert_eq!(env_vars(&out[0].1), vec![("NEW".into(), "1".into())]);
        let (src, out, _) = step(src, &d, &s, 1000);
        assert!(out.is_empty());
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
    }

    /// A tab open in the first demand a new source sees is read once. The reducer's open
    /// notice that comes after that read only resends the payload.
    #[test]
    fn first_demand_open_notice_reads_once() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let (mut src, first, _) = step(ProcessDetailSource::new(cfg), &d, &s, 0);
        assert_eq!(first.len(), 1);
        opened(&mut src, DetailTab::Environment);
        let (src, out, _) = step(src, &d, &s, 500);
        assert_eq!(out.len(), 1);
        assert!(Arc::ptr_eq(&first[0].1, &out[0].1));
        assert_eq!(src.tab_reads(DetailTab::Environment), 1);
    }

    /// A user refresh reads again even while the tab still waits for its open notice.
    #[test]
    fn user_refresh_ignores_open_pending() {
        let (tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let (mut src, _, _) = step(running(cfg), &d, &s, 0);
        fs::write(tmp.path().join("proc/4201/environ"), b"NEW=1\0").expect("rewrite environ");
        user_refresh(&mut src, DetailTab::Environment);
        let (src, out, _) = step(src, &d, &s, 500);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
        assert_eq!(env_vars(&out[0].1), vec![("NEW".into(), "1".into())]);
    }

    /// A user refresh between the first read and the reducer's open notice reads once more. The
    /// notice then only resends the payload: two `environ` reads in all, not three.
    #[test]
    fn user_refresh_before_open_notice_keeps_it_pending() {
        let (tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let (mut src, _, _) = step(running(cfg), &d, &s, 0);
        assert_eq!(src.tab_reads(DetailTab::Environment), 1);
        fs::write(tmp.path().join("proc/4201/environ"), b"NEW=1\0").expect("rewrite environ");
        user_refresh(&mut src, DetailTab::Environment);
        let (mut src, refreshed, _) = step(src, &d, &s, 500);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
        opened(&mut src, DetailTab::Environment);
        let (src, out, _) = step(src, &d, &s, 1000);
        assert_eq!(out.len(), 1);
        assert!(Arc::ptr_eq(&refreshed[0].1, &out[0].1));
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
        let (src, _, _) = step(src, &d, &s, 1500);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);

        // The refresh and the notice in one sample: one read for both.
        let (mut src, _, _) = step(running(src.cfg.clone()), &d, &s, 0);
        user_refresh(&mut src, DetailTab::Environment);
        opened(&mut src, DetailTab::Environment);
        let (src, out, _) = step(src, &d, &s, 500);
        assert_eq!(out.len(), 1);
        assert_eq!(src.tab_reads(DetailTab::Environment), 2);
    }

    /// A gone process resends the last payload as `Missing`, once.
    #[test]
    fn gone_target_keeps_last_payload_as_missing() {
        let (tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Fds)]);
        let s = every_ms(500);
        let (src, first, _) = step(ProcessDetailSource::new(cfg), &d, &s, 0);
        fs::remove_dir_all(tmp.path().join("proc/4201")).expect("remove process");
        let (src, gone, _) = step(src, &d, &s, 500);
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].2, Validity::Missing);
        assert!(Arc::ptr_eq(&first[0].1, &gone[0].1));
        let (_src, again, _) = step(src, &d, &s, 1000);
        assert!(again.is_empty());
    }

    #[test]
    fn fds_payload_kinds() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Fds)]);
        let (_src, out, _) = step(ProcessDetailSource::new(cfg), &d, &every_ms(500), 0);
        let TabPayload::Fds(fds) = &*out[0].1 else {
            panic!("expected Fds");
        };
        assert!(fds.fdinfo_available);
        let row = |fd: i32| fds.rows.iter().find(|r| r.fd == fd).expect("fd row");
        // The fixture link `socket:[1234]` has no socket inode behind it: "other", not unix.
        assert_eq!(row(0).kind, FdKind::AnonInode);
        assert_eq!(row(0).socket_inode, Some(1234));
        assert_eq!(row(0).pos, None);
        assert_eq!(row(1).kind, FdKind::Pipe);
        assert_eq!(row(1).socket_inode, None);
        assert_eq!(row(2).kind, FdKind::Eventfd);
        assert!(row(2).unnamed);
        assert_eq!(row(3).kind, FdKind::Memfd);
        assert_eq!(row(4).kind, FdKind::Drm);
        assert_eq!(row(12).kind, FdKind::File);
        assert!(!row(12).unnamed);
    }

    #[test]
    fn socket_protocol_names() {
        for (name, kind) in [
            (&b"TCP\0"[..], FdType::Tcp),
            (b"TCPv6\0", FdType::Tcp),
            (b"UDP\0", FdType::Udp),
            (b"UDPv6", FdType::Udp),
            (b"UNIX\0", FdType::Unix),
            (b"UNIX-STREAM\0", FdType::Unix),
            (b"NETLINK\0", FdType::Netlink),
            (b"RAW\0", FdType::Socket),
            (b"", FdType::Socket),
        ] {
            assert_eq!(protocol_type(name), kind, "{name:?}");
        }
    }

    /// TCP, UDP and unix sockets of this test process get their own type, not unix.
    #[test]
    fn fds_resolve_socket_protocols() {
        use std::os::fd::AsRawFd;

        let tcp = std::net::TcpListener::bind("127.0.0.1:0").expect("bind tcp");
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind udp");
        let (unix, _peer) = std::os::unix::net::UnixStream::pair().expect("unix pair");
        let stat = fs::read_to_string("/proc/self/stat").expect("own stat");
        // Field 22 is start_ticks; field 3 is the first after the ")".
        let start_ticks = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(19))
            .and_then(|t| t.parse().ok())
            .expect("start_ticks");
        let cfg = EngineConfig::new(EngineMode::Full, PathBuf::from("/nonexistent"))
            .with_sys_root(SysRoot::host());
        let me = key(std::process::id(), start_ticks);
        let d = tabs(&[(me, DetailTab::Fds)]);
        let (_src, out, _) = step(
            ProcessDetailSource::new(Arc::new(cfg)),
            &d,
            &every_ms(500),
            0,
        );
        assert_eq!(out[0].2, Validity::Valid);
        let TabPayload::Fds(fds) = &*out[0].1 else {
            panic!("expected Fds");
        };
        let kind = |fd: i32| fds.rows.iter().find(|r| r.fd == fd).map(|r| r.kind);
        assert_eq!(kind(tcp.as_raw_fd()), Some(FdKind::Tcp));
        assert_eq!(kind(udp.as_raw_fd()), Some(FdKind::Udp));
        assert_eq!(kind(unix.as_raw_fd()), Some(FdKind::Unix));
    }

    /// Another tab read first must not leave an empty user table behind for Security.
    #[test]
    fn security_names_after_other_tab() {
        let (tmp, cfg) = edge_root();
        fs::create_dir_all(tmp.path().join("etc")).expect("create etc");
        fs::write(
            tmp.path().join("etc/passwd"),
            b"alice:x:1000:1000::/home/alice:/bin/sh\n",
        )
        .expect("write passwd");
        let s = every_ms(500);
        let (src, _, _) = step(
            ProcessDetailSource::new(cfg),
            &tabs(&[(a(), DetailTab::Threads)]),
            &s,
            0,
        );
        let (_src, out, _) = step(src, &tabs(&[(a(), DetailTab::Security)]), &s, 500);
        let TabPayload::Security(sec) = &*out[0].1 else {
            panic!("expected Security");
        };
        assert_eq!(&*sec.uid_names.value[0], "alice");
    }

    #[test]
    fn security_payload_groups_and_ids() {
        let (_tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Security)]);
        let (_src, out, _) = step(ProcessDetailSource::new(cfg), &d, &every_ms(500), 0);
        assert_eq!(out[0].2, Validity::Valid);
        let TabPayload::Security(sec) = &*out[0].1 else {
            panic!("expected Security");
        };
        assert_eq!(sec.uids.value.real, 1000);
        // No etc/passwd in the fixture: the number.
        assert_eq!(&*sec.uid_names.value[0], "1000");
        assert_eq!(&*sec.gid_names.value[0], "user1");
        let groups: Vec<(&str, u32, bool, bool, bool)> = sec
            .groups
            .iter()
            .map(|g| (&*g.name, g.gid, g.primary, g.admin, g.root_equivalent))
            .collect();
        assert_eq!(
            groups,
            vec![
                ("user1", 1000, true, false, false),
                ("10", 10, false, false, false),
                ("wheel", 998, false, true, false),
            ]
        );
    }

    // ---- General tab (tasklens-dwy.13) ----

    /// `btime` in the General fixture's `proc/stat`.
    const BTIME: i64 = 1_700_000_000;

    /// `edge_root` plus the files the General tab reads that process-edge lacks, a passwd entry
    /// for uid 1000, 100 ticks per second and a fixed Location context.
    fn general_src() -> (tempfile::TempDir, ProcessDetailSource) {
        let (tmp, cfg) = edge_root();
        let base = tmp.path();
        fs::write(
            base.join("proc/stat"),
            format!("cpu  1 2 3 4 5 6 7 8 9 10\nintr 1 2 3\nbtime {BTIME}\n"),
        )
        .expect("write proc/stat");
        fs::create_dir_all(base.join("proc/sys/kernel")).expect("create proc/sys/kernel");
        fs::write(base.join("proc/sys/kernel/randomize_va_space"), b"2\n").expect("write aslr");
        fs::write(base.join("proc/4201/personality"), b"00040000\n").expect("write personality");
        fs::write(base.join("proc/4201/autogroup"), b"/autogroup-12 nice 0\n")
            .expect("write autogroup");
        fs::write(
            base.join("etc/passwd"),
            b"user1:x:1000:1000::/home/user1:/bin/sh\n",
        )
        .expect("write passwd");
        let mut src = ProcessDetailSource::new(cfg);
        src.ticks_per_sec = 100;
        src.general.location = Some(LocationCtx {
            home: "/home/user1".into(),
            runtime_dir: Some("/run/user/1000".into()),
            downloads: Some("/home/user1/Downloads".into()),
        });
        src.general.self_uid = 1000;
        src.general.apps = Some(desktop_apps(
            &tmp,
            &[("firefox", "/usr/lib/firefox/firefox %u")],
        ));
        (tmp, src)
    }

    /// A desktop index of `entries` (id, `Exec`) without a watch. The directory under `tmp` is
    /// named after the ids, so another set of ids never shares it.
    fn desktop_apps(tmp: &tempfile::TempDir, entries: &[(&str, &str)]) -> AppIndex {
        let ids: Vec<&str> = entries.iter().map(|(id, _)| *id).collect();
        let dir = tmp
            .path()
            .join(format!("share-{}/applications", ids.join("+")));
        fs::create_dir_all(&dir).expect("create applications dir");
        for (id, exec) in entries {
            fs::write(
                dir.join(format!("{id}.desktop")),
                format!("[Desktop Entry]\nType=Application\nName={id}\nExec={exec}\n"),
            )
            .expect("write desktop file");
        }
        AppIndex {
            index: DesktopIndex::build_in(&[dir], &[]),
            watch: None,
        }
    }

    fn write_cgroup(tmp: &tempfile::TempDir, unit: &str) {
        fs::write(
            tmp.path().join("proc/4201/cgroup"),
            format!("0::/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}\n"),
        )
        .expect("rewrite cgroup");
    }

    /// The General App ID of 4201 after one read.
    fn app_id_of(src: ProcessDetailSource) -> Option<Arc<str>> {
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        general(&out).0.app_id.clone()
    }

    /// The General payload in `out` and its validity.
    fn general(out: &[TabOut]) -> (Arc<GeneralDetail>, Validity) {
        out.iter()
            .find_map(|(tab, payload, validity)| match (tab, &**payload) {
                (DetailTab::General, TabPayload::General(g)) => Some((Arc::clone(g), *validity)),
                _ => None,
            })
            .expect("a General payload")
    }

    fn relink_exe(tmp: &tempfile::TempDir, target: &Path) {
        let link = tmp.path().join("proc/4201/exe");
        fs::remove_file(&link).expect("remove exe link");
        std::os::unix::fs::symlink(target, &link).expect("create exe link");
    }

    /// General is read beside another tab, first, with the PROC-078 fields from the fixture.
    #[test]
    fn general_fields_from_fixture() {
        let (_tmp, src) = general_src();
        let d = tabs(&[(a(), DetailTab::Threads)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        let sent: Vec<DetailTab> = out.iter().map(|(tab, _, _)| *tab).collect();
        assert_eq!(sent, vec![DetailTab::General, DetailTab::Threads]);
        let (g, validity) = general(&out);
        assert_eq!(validity, Validity::Valid);

        // Started, user and priority.
        assert_eq!(g.started_s, valid(BTIME + 3512));
        assert_eq!(g.uids.value.real, 1000);
        assert_eq!(&*g.uid_names.value.0, "user1");
        assert_eq!(g.policy, valid(SchedPolicy::Other));
        assert_eq!(g.rt_prio, 0);
        assert_eq!(g.kernel_prio, valid(120));
        assert_eq!(&*g.autogroup.value.0, "autogroup-12");
        assert_eq!(g.autogroup.value.1, 0);
        assert_eq!(g.autogroup.validity, Validity::Valid);

        // Affinity 0-7,16-23; thread 4211 has 8-11.
        assert_eq!(g.affinity.validity, Validity::Valid);
        assert_eq!(g.affinity.value.count(), 16);
        assert!(g.affinity.value.contains(0) && g.affinity.value.contains(23));
        assert!(!g.affinity.value.contains(8));
        assert!(g.affinity_threads_differ);

        // Hardening from status, the ASLR setting and the personality.
        assert_eq!(g.shadow_stack, valid(true));
        assert_eq!(g.aslr_system, valid(2));
        assert_eq!(g.aslr_disabled_for_process, valid(true));
        assert_eq!(g.no_new_privs, valid(true));
        assert_eq!(g.seccomp, valid(Seccomp::Filter { filters: 3 }));

        // Paths, cgroup, unit and app id.
        assert_eq!(&*g.exe.value, "/usr/lib/firefox/firefox");
        assert!(!g.exe_deleted);
        assert_eq!(g.location, valid(ExeLocation::System));
        assert_eq!(&*g.cwd.value, "/home/user1");
        assert_eq!(g.cwd.validity, Validity::Valid);
        assert_eq!(&*g.root.value, "/");
        assert_eq!(g.cgroup.validity, Validity::Valid);
        assert_eq!(
            g.cgroup.value.as_deref(),
            Some(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox@3a1b.service"
            )
        );
        assert_eq!(
            g.unit,
            valid(Some(UnitRef {
                manager: Manager::User,
                name: Arc::from("app-firefox@3a1b.service"),
            }))
        );
        assert_eq!(g.app_id.as_deref(), Some("firefox"));
        assert_eq!(&*g.cmdline.value, "firefox -P default");
        assert_eq!(g.cmdline.validity, Validity::Valid);

        // Memory, CPU time, I/O and disk counters.
        assert_eq!(g.handles.validity, Validity::Valid);
        assert_eq!(g.rss_shared_bytes, valid((198_520 + 45_182) * 1024));
        assert_eq!(g.commit_bytes, valid((100 + 132) * 1024));
        assert_eq!(g.cpu_time_ms, valid((8812 + 1204) * 10));
        assert_eq!(g.io_rchar, valid(48_213_377));
        assert_eq!(g.io_wchar, valid(9_120_044));
        assert_eq!(g.io_syscr, valid(61_204));
        assert_eq!(g.io_syscw, valid(18_840));
        assert_eq!(g.disk_read_bytes, valid(31_571_968));
        assert_eq!(g.disk_write_bytes, valid(6_627_328));
        assert_eq!(g.cancelled_write_bytes, valid(1_048_576));
        // No ComputeHashes was sent.
        assert_eq!(g.hashes, HashState::Pending);
    }

    /// The ELF hardening and architecture of a real executable: this test binary.
    #[test]
    fn general_elf_hardening() {
        let (tmp, src) = general_src();
        relink_exe(&tmp, &std::env::current_exe().expect("test binary path"));
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        let (g, _) = general(&out);
        assert_eq!(g.pie.validity, Validity::Valid);
        assert_eq!(g.relro.validity, Validity::Valid);
        assert_eq!(g.nx_stack, valid(true));
        let arch = if cfg!(target_arch = "x86_64") {
            ElfArch::X86_64
        } else if cfg!(target_arch = "aarch64") {
            ElfArch::AArch64
        } else {
            ElfArch::Unknown
        };
        assert_eq!(g.architecture, valid(arch));
    }

    /// A deleted executable keeps the kernel's suffix and is in the Deleted location. Its ELF
    /// cannot be opened, so the hardening rows are "Not reported".
    #[test]
    fn general_deleted_exe() {
        let (tmp, src) = general_src();
        relink_exe(&tmp, Path::new("/home/user1/bin/app (deleted)"));
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        let (g, _) = general(&out);
        assert_eq!(&*g.exe.value, "/home/user1/bin/app (deleted)");
        assert!(g.exe_deleted);
        assert_eq!(g.location, valid(ExeLocation::Deleted));
        assert_eq!(g.pie.validity, Validity::Unsupported);
    }

    /// Realtime: stat priority -51 is kernel prio 49.
    #[test]
    fn general_realtime_priority() {
        let (tmp, src) = general_src();
        let stat = std::str::from_utf8(STAT)
            .expect("stat is text")
            .replace(" 20 0 27 0 351220 ", " -51 0 27 0 351220 ")
            .replace(" 17 3 0 0 ", " 17 3 50 1 ");
        fs::write(tmp.path().join("proc/4201/stat"), stat).expect("rewrite stat");
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        let (g, _) = general(&out);
        assert_eq!(g.policy, valid(SchedPolicy::Fifo));
        assert_eq!(g.rt_prio, 50);
        assert_eq!(g.kernel_prio, valid(49));
    }

    /// EACCES on a field's own file makes that field `Restricted`; the tab stays `Valid`.
    #[test]
    fn general_restricted_fields() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root");
            return;
        }
        let (tmp, src) = general_src();
        for file in ["io", "personality", "cmdline", "cgroup"] {
            let path = tmp.path().join("proc/4201").join(file);
            fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("chmod");
        }
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        let (g, validity) = general(&out);
        assert_eq!(validity, Validity::Valid);
        assert_eq!(g.io_rchar.validity, Validity::Restricted);
        assert_eq!(g.disk_read_bytes.validity, Validity::Restricted);
        assert_eq!(g.aslr_disabled_for_process.validity, Validity::Restricted);
        assert_eq!(g.cmdline.validity, Validity::Restricted);
        assert_eq!(g.cgroup, total(None, Validity::Restricted));
        assert_eq!(g.unit, total(None, Validity::Restricted));
        assert_eq!(g.uids.validity, Validity::Valid);
        assert_eq!(g.cpu_time_ms.validity, Validity::Valid);
    }

    /// EACCES on `status`, `autogroup` and `randomize_va_space` makes only their fields
    /// `Restricted`. Without the real uid there is no App ID.
    #[test]
    fn general_restricted_status_autogroup_aslr() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root");
            return;
        }
        let (tmp, src) = general_src();
        for file in [
            "proc/4201/status",
            "proc/4201/autogroup",
            "proc/sys/kernel/randomize_va_space",
        ] {
            let path = tmp.path().join(file);
            fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("chmod");
        }
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        let (g, validity) = general(&out);
        assert_eq!(validity, Validity::Valid);
        for v in [
            g.uids.validity,
            g.uid_names.validity,
            g.affinity.validity,
            g.shadow_stack.validity,
            g.no_new_privs.validity,
            g.seccomp.validity,
            g.rss_shared_bytes.validity,
            g.commit_bytes.validity,
            g.autogroup.validity,
            g.aslr_system.validity,
        ] {
            assert_eq!(v, Validity::Restricted);
        }
        assert!(!g.affinity_threads_differ);
        assert_eq!(g.app_id, None);
        // Fields from other files keep their values.
        assert_eq!(g.kernel_prio, valid(120));
        assert_eq!(g.cpu_time_ms, valid((8812 + 1204) * 10));
        assert_eq!(g.io_rchar, valid(48_213_377));
        assert_eq!(&*g.exe.value, "/usr/lib/firefox/firefox");
    }

    /// A missing `autogroup`, `randomize_va_space` or `cgroup` is "Not reported", not
    /// `Restricted`.
    #[test]
    fn general_absent_autogroup_and_aslr() {
        let (tmp, src) = general_src();
        fs::remove_file(tmp.path().join("proc/4201/autogroup")).expect("remove autogroup");
        fs::remove_file(tmp.path().join("proc/4201/cgroup")).expect("remove cgroup");
        fs::remove_file(tmp.path().join("proc/sys/kernel/randomize_va_space"))
            .expect("remove aslr");
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        let (g, _) = general(&out);
        assert_eq!(g.autogroup.validity, Validity::Unsupported);
        assert_eq!(g.aslr_system.validity, Validity::Unsupported);
        assert_eq!(g.cgroup, total(None, Validity::Unsupported));
        assert_eq!(g.unit, total(None, Validity::Unsupported));
    }

    /// The App ID is the desktop match of the process sweep: the unit app id, the launcher
    /// form's id without its launcher, then the executable. A unit app id with no desktop entry
    /// is no App ID, and neither is a process of another user.
    #[test]
    fn general_app_id_from_desktop_match() {
        // The unit app id `firefox` is a desktop id.
        let (_tmp, src) = general_src();
        assert_eq!(app_id_of(src).as_deref(), Some("firefox"));

        // A launcher scope: `gnome-firefox` is not a desktop id, `firefox` is.
        let (tmp, mut src) = general_src();
        write_cgroup(&tmp, "app-gnome-firefox-777.scope");
        src.general.apps = Some(desktop_apps(&tmp, &[("firefox", "/opt/other/bin")]));
        assert_eq!(app_id_of(src).as_deref(), Some("firefox"));

        // No desktop id for the unit: the executable matches another entry's `Exec`.
        let (tmp, mut src) = general_src();
        src.general.apps = Some(desktop_apps(
            &tmp,
            &[("org.mozilla.firefox", "/usr/lib/firefox/firefox %u")],
        ));
        assert_eq!(app_id_of(src).as_deref(), Some("org.mozilla.firefox"));

        // The unit parses, but nothing in the index matches.
        let (tmp, mut src) = general_src();
        src.general.apps = Some(desktop_apps(&tmp, &[]));
        assert_eq!(app_id_of(src), None);

        // Another user's process is a System row, so it has no App ID.
        let (_tmp, mut src) = general_src();
        src.general.self_uid = 1001;
        assert_eq!(app_id_of(src), None);
    }

    /// An unchanged General keeps its `Arc`. An open notice resends it, a changed file sends a
    /// new payload, and a user refresh reads it again.
    #[test]
    fn general_change_detection_and_notices() {
        let (tmp, src) = general_src();
        let d = tabs(&[(a(), DetailTab::Threads)]);
        let s = every_ms(500);
        let (mut src, out, _) = step_all(src, &d, &s, 0);
        let (first, _) = general(&out);

        // The reducer's notice for the Threads tab resends General.
        opened(&mut src, DetailTab::Threads);
        let (src, out, _) = step_all(src, &d, &s, 500);
        let (resent, _) = general(&out);
        assert!(Arc::ptr_eq(&first, &resent));

        let (src, out, _) = step_all(src, &d, &s, 1000);
        assert!(out.iter().all(|(tab, _, _)| *tab != DetailTab::General));

        let io = fs::read_to_string(tmp.path().join("proc/4201/io")).expect("read io");
        fs::write(
            tmp.path().join("proc/4201/io"),
            io.replace("rchar: 48213377", "rchar: 50000000"),
        )
        .expect("rewrite io");
        let (mut src, out, _) = step_all(src, &d, &s, 1500);
        let (changed, _) = general(&out);
        assert_eq!(changed.io_rchar, valid(50_000_000));

        // A user refresh of General drops its state, so the same values go out again.
        user_refresh(&mut src, DetailTab::General);
        let (_src, out, _) = step_all(src, &d, &s, 1600);
        let (refreshed, _) = general(&out);
        assert!(!Arc::ptr_eq(&changed, &refreshed));
        assert_eq!(refreshed.io_rchar, valid(50_000_000));
    }

    /// An exited process keeps its last General values as `Missing`, so the window shows the
    /// exited state.
    #[test]
    fn general_gone_is_missing() {
        let (tmp, src) = general_src();
        let d = tabs(&[(a(), DetailTab::Fds)]);
        let s = every_ms(500);
        let (src, out, _) = step_all(src, &d, &s, 0);
        let (first, _) = general(&out);
        fs::remove_dir_all(tmp.path().join("proc/4201")).expect("remove process");
        let (_src, out, _) = step_all(src, &d, &s, 500);
        let (gone, validity) = general(&out);
        assert_eq!(validity, Validity::Missing);
        assert!(Arc::ptr_eq(&first, &gone));
    }

    /// Owns `owned` as package `firefox 131.0-1` by Jan Doe; every other path is not packaged.
    struct FakePkg {
        owned: std::path::PathBuf,
        outcome: tl_collect::pkg::VerifyOutcome,
    }

    impl tl_collect::pkg::PkgBackend for FakePkg {
        fn owners(&mut self, paths: &[&Path]) -> Result<tl_collect::pkg::PathOwners, CollectError> {
            Ok(paths
                .iter()
                .map(|p| {
                    (
                        p.to_path_buf(),
                        (*p == self.owned).then(|| Arc::from("firefox")),
                    )
                })
                .collect())
        }

        fn info(
            &mut self,
            name: &str,
        ) -> Result<Option<tl_model::process::PackageInfo>, CollectError> {
            Ok(Some(tl_model::process::PackageInfo {
                name: Arc::from(name),
                version: Arc::from("131.0-1"),
                description: Some(Arc::from("Web browser")),
                packager: Some(Arc::from("Jan Doe")),
                build_date_s: None,
                url: None,
            }))
        }

        fn verify(&mut self, name: &str) -> Result<tl_collect::pkg::VerifyOutcome, CollectError> {
            assert_eq!(name, "firefox");
            Ok(self.outcome.clone())
        }

        fn db_stamp(&mut self) -> Option<i64> {
            Some(1)
        }
    }

    /// `general_src` with 4201's exe linked to a real file, and an index whose backend owns
    /// `owned` (relative to the root) or nothing.
    fn package_src(owned: Option<&str>) -> (tempfile::TempDir, ProcessDetailSource, PackageIndex) {
        package_src_with(owned, tl_collect::pkg::VerifyOutcome::Unmodified)
    }

    /// `package_src` with the result `PackageIndex::verify` gives.
    fn package_src_with(
        owned: Option<&str>,
        outcome: tl_collect::pkg::VerifyOutcome,
    ) -> (tempfile::TempDir, ProcessDetailSource, PackageIndex) {
        let (tmp, src) = general_src();
        let exe = tmp.path().join("usr-bin-app");
        fs::write(&exe, b"#!/bin/sh\n").expect("write exe");
        relink_exe(&tmp, &exe);
        let owned = owned.map_or_else(Default::default, |name| tmp.path().join(name));
        let index = PackageIndex::new(Box::new(FakePkg { owned, outcome }));
        let src = src.with_package_index(Some(index.clone()));
        (tmp, src, index)
    }

    /// A packaged executable: `WarmingUp` until the batch, then name, version and packager, and
    /// the change goes out at the next General read.
    #[test]
    fn general_package_from_index() {
        let (_tmp, src, index) = package_src(Some("usr-bin-app"));
        let d = tabs(&[(a(), DetailTab::General)]);
        let s = every_ms(500);
        let (src, out, _) = step_all(src, &d, &s, 0);
        let (g, _) = general(&out);
        assert_eq!(g.package, total(None, Validity::WarmingUp));
        assert!(
            index.due(Instant::now() + Duration::from_secs(3)),
            "exe queued"
        );

        assert_eq!(index.run_batch().expect("batch"), 1);
        let (_src, out, _) = step_all(src, &d, &s, 500);
        let (g, validity) = general(&out);
        assert_eq!(validity, Validity::Valid);
        assert_eq!(
            g.package,
            valid(Some(PackageInfo {
                name: Arc::from("firefox"),
                version: Arc::from("131.0-1"),
                packager: Some(Arc::from("Jan Doe")),
            }))
        );
    }

    /// No package owns the executable: `Valid(None)`, "Not packaged".
    #[test]
    fn general_package_not_packaged() {
        let (_tmp, src, index) = package_src(None);
        let d = tabs(&[(a(), DetailTab::General)]);
        let s = every_ms(500);
        let (src, _, _) = step_all(src, &d, &s, 0);
        assert_eq!(index.run_batch().expect("batch"), 1);
        let (_src, out, _) = step_all(src, &d, &s, 500);
        assert_eq!(general(&out).0.package, valid(None));
    }

    /// Without a package backend the field is `Unsupported` and nothing is queued.
    #[test]
    fn general_package_without_backend() {
        let (tmp, src) = general_src();
        relink_exe(&tmp, &std::env::current_exe().expect("test binary path"));
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        assert_eq!(general(&out).0.package, total(None, Validity::Unsupported));
    }

    /// A deleted executable is not looked up: its path may belong to a newer version now.
    #[test]
    fn general_package_deleted_exe_not_looked_up() {
        let (tmp, src, index) = package_src(Some("usr-bin-app"));
        relink_exe(&tmp, Path::new("/home/user1/bin/app (deleted)"));
        let d = tabs(&[(a(), DetailTab::General)]);
        let (_src, out, _) = step_all(src, &d, &every_ms(500), 0);
        assert_eq!(general(&out).0.package, total(None, Validity::Unsupported));
        assert!(!index.due(Instant::now() + Duration::from_secs(3)));
    }

    /// `VerifyPackage` on a packaged process: the General payload carries the check.
    fn verified(outcome: VerifyOutcome) -> PackageCheck {
        let (_tmp, src, index) = package_src_with(Some("usr-bin-app"), outcome);
        let d = tabs(&[(a(), DetailTab::General)]);
        let s = every_ms(500);
        let (src, out, _) = step_all(src, &d, &s, 0);
        assert_eq!(general(&out).0.package_check, PackageCheck::NotVerified);
        assert_eq!(index.run_batch().expect("batch"), 1);
        // The next read settles the package; the request then runs.
        let (mut src, _, _) = step_all(src, &d, &s, 500);
        assert!(src.command(&WorkerCommand::VerifyPackage(a())));
        let (src, out, _) = step_all(src, &d, &s, 1000);
        assert!(src.verify_requests.is_empty());
        general(&out).0.package_check
    }

    #[test]
    fn verify_package_matches() {
        assert_eq!(verified(VerifyOutcome::Unmodified), PackageCheck::Matches);
    }

    #[test]
    fn verify_package_does_not_match() {
        assert_eq!(
            verified(VerifyOutcome::Modified { altered_files: 2 }),
            PackageCheck::DoesNotMatch
        );
    }

    #[test]
    fn verify_package_failed() {
        assert_eq!(
            verified(VerifyOutcome::Failed(Arc::from("pacman not found"))),
            PackageCheck::Failed
        );
    }

    /// Not packaged: nothing is verified, the check stays and no payload goes out.
    #[test]
    fn verify_package_not_packaged() {
        let (_tmp, src, index) = package_src_with(None, VerifyOutcome::Unmodified);
        let d = tabs(&[(a(), DetailTab::General)]);
        let s = every_ms(500);
        let (src, _, _) = step_all(src, &d, &s, 0);
        assert_eq!(index.run_batch().expect("batch"), 1);
        // The next read settles the package; the request then runs.
        let (mut src, out, _) = step_all(src, &d, &s, 500);
        assert_eq!(general(&out).0.package, valid(None));
        assert!(src.command(&WorkerCommand::VerifyPackage(a())));
        let (src, out, _) = step_all(src, &d, &s, 1000);
        assert!(src.verify_requests.is_empty());
        assert!(out.is_empty(), "General changed: {out:?}");
    }

    #[test]
    fn download_dir_from_user_dirs() {
        let file =
            b"# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\n";
        assert_eq!(
            download_dir(file, "/home/u").as_deref(),
            Some("/home/u/Downloads")
        );
        assert_eq!(
            download_dir(b"XDG_DOWNLOAD_DIR=\"/data/dl/\"\n", "/home/u").as_deref(),
            Some("/data/dl")
        );
        assert_eq!(
            download_dir(b"XDG_DOWNLOAD_DIR=\"$HOME/\"\n", "/home/u"),
            None
        );
        assert_eq!(download_dir(b"XDG_DOWNLOAD_DIR=\"dl\"\n", "/home/u"), None);
        assert_eq!(download_dir(b"", "/home/u"), None);
    }

    // ---- Hashes and affinity masks (E10-005) ----

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// General of 4201 beside the PSS column at a 2 s interval: tabs are read on every second
    /// sample, so a sample in between reads General only for a reason.
    fn general_beside_pss() -> (Demand, EngineSettings) {
        let mut d = tabs(&[(a(), DetailTab::General)]);
        d.process_columns.insert(ColumnId::Pss);
        d.detail_keys = Arc::from([a()]);
        (d, every_ms(2000))
    }

    /// E10-005 AC 1: the General payload after `ComputeHashes` has both digests, and a second
    /// request hits the cache and sends nothing.
    #[test]
    fn hashes_once_and_cached() {
        let (tmp, src) = general_src();
        let exe = tmp.path().join("exe-abc");
        fs::write(&exe, b"abc").expect("write exe");
        relink_exe(&tmp, &exe);
        let (d, s) = general_beside_pss();
        let (mut src, out, _) = step_all(src, &d, &s, 0);
        assert_eq!(general(&out).0.hashes, HashState::Pending);

        let reads = hash::hash_reads();
        assert!(src.command(&WorkerCommand::ComputeHashes(a())));
        let (mut src, out, _) = step_all(src, &d, &s, 1000);
        let HashState::Done { sha256, md5 } = general(&out).0.hashes.clone() else {
            panic!("expected hashes, got {out:?}");
        };
        assert_eq!(
            hex(&sha256),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hex(&md5), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hash::hash_reads(), reads + 1);
        assert!(src.hash_requests.is_empty());

        assert!(src.command(&WorkerCommand::ComputeHashes(a())));
        let (src, out, _) = step_all(src, &d, &s, 2000);
        assert!(out.is_empty(), "unchanged General was sent: {out:?}");
        assert_eq!(hash::hash_reads(), reads + 1);
        assert_eq!(src.hash_cache.len(), 1);
    }

    /// E10-005 AC 2: an executable that cannot be opened gives "Executable path unavailable".
    /// argv0 "firefox" is not absolute, so it cannot stand in.
    #[test]
    fn hashes_unreadable_exe() {
        let (tmp, src) = general_src();
        relink_exe(&tmp, &tmp.path().join("no-such-exe"));
        let (d, s) = general_beside_pss();
        let (mut src, _, _) = step_all(src, &d, &s, 0);
        assert!(src.command(&WorkerCommand::ComputeHashes(a())));
        let (_src, out, _) = step_all(src, &d, &s, 1000);
        assert_eq!(
            general(&out).0.hashes,
            HashState::Failed(HashError::ExePathUnavailable)
        );
    }

    /// A read error gives "Failed to compute one or more hashes" with its errno.
    #[test]
    fn hashes_read_error() {
        let (tmp, src) = general_src();
        let dir = tmp.path().join("exe-dir");
        fs::create_dir_all(&dir).expect("create exe dir");
        relink_exe(&tmp, &dir);
        let (d, s) = general_beside_pss();
        let (mut src, _, _) = step_all(src, &d, &s, 0);
        assert!(src.command(&WorkerCommand::ComputeHashes(a())));
        let (_src, out, _) = step_all(src, &d, &s, 1000);
        assert_eq!(
            general(&out).0.hashes,
            HashState::Failed(HashError::ReadFailed {
                errno: rustix::io::Errno::ISDIR.raw_os_error(),
            })
        );
    }

    /// A request sent before its window's demand waits for it, as the VirusTotal flow sends
    /// them.
    #[test]
    fn hashes_wait_for_demand() {
        let (tmp, mut src) = general_src();
        let exe = tmp.path().join("exe-abc");
        fs::write(&exe, b"abc").expect("write exe");
        relink_exe(&tmp, &exe);
        assert!(src.command(&WorkerCommand::ComputeHashes(a())));
        let (src, out, _) = step_all(src, &Demand::default(), &every_ms(500), 0);
        assert!(out.is_empty());
        assert_eq!(src.hash_requests, vec![a()]);
        let d = tabs(&[(a(), DetailTab::General)]);
        let (src, out, _) = step_all(src, &d, &every_ms(500), 500);
        assert!(matches!(general(&out).0.hashes, HashState::Done { .. }));
        assert!(src.hash_requests.is_empty());
    }

    /// `period`, `sample_at(now_ms)`, then the entries of every affinity update the loop would
    /// send.
    fn affinity_step(
        src: ProcessDetailSource,
        d: &Demand,
        s: &EngineSettings,
        now_ms: u64,
    ) -> (ProcessDetailSource, Vec<Vec<(ProcessKey, AffinityInfo)>>) {
        src.period(d, s);
        let (mut src, update) = src.sample_at(now_ms * MS + 1);
        let mut out = Vec::new();
        let mut push = |u: SourceUpdate| {
            if let SourceUpdate::Details(DetailsUpdate::Affinity { entries, .. }) = u {
                out.push(entries);
            }
        };
        while let Some(extra) = src.take_extra() {
            push(extra);
        }
        push(update);
        (src, out)
    }

    fn cpus(list: &str) -> CpuSet {
        cpu_set(list.as_bytes(), &mut SmallVec::new()).expect("cpu list")
    }

    fn affinity_demand(keys: &[ProcessKey]) -> Demand {
        Demand {
            affinity_targets: Arc::from(keys),
            ..Demand::default()
        }
    }

    /// process-edge with 4202, a copy of 4201 in `system.slice/b.service`. 4201's cgroup
    /// allows 0-7; b.service has no cpuset controller, so its parent's 16-23 applies.
    fn affinity_root() -> (tempfile::TempDir, Arc<EngineConfig>, ProcessKey) {
        let (tmp, cfg) = edge_root();
        let base = tmp.path();
        copy_tree(&base.join("proc/4201"), &base.join("proc/4202"));
        fs::write(
            base.join("proc/4202/cgroup"),
            b"0::/system.slice/b.service\n",
        )
        .expect("write cgroup");
        let cg = base.join("sys/fs/cgroup");
        let a_cg = cg.join(
            "user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox@3a1b.service",
        );
        fs::create_dir_all(&a_cg).expect("create cgroup dir");
        fs::write(a_cg.join("cpuset.cpus.effective"), b"0-7\n").expect("write cpuset");
        fs::create_dir_all(cg.join("system.slice/b.service")).expect("create cgroup dir");
        fs::write(cg.join("system.slice/cpuset.cpus.effective"), b"16-23\n").expect("write cpuset");
        (tmp, cfg, key(4202, START_TICKS))
    }

    /// E10-005 AC 3: one update a second with the mask and the cpuset limit of each target.
    #[test]
    fn affinity_targets_1hz() {
        let (_tmp, cfg, b) = affinity_root();
        let d = affinity_demand(&[a(), b]);
        let s = EngineSettings::default();
        let mut src = ProcessDetailSource::new(cfg);
        assert_eq!(src.period(&d, &s), Some(AFFINITY_PERIOD));
        let mut updates = Vec::new();
        for t in [0, 1000, 2000] {
            let (next, out) = affinity_step(src, &d, &s, t);
            src = next;
            updates.extend(out);
        }
        assert_eq!(updates.len(), 3);
        for entries in &updates {
            assert_eq!(
                entries,
                &vec![
                    (
                        a(),
                        AffinityInfo {
                            mask: cpus("0-7,16-23"),
                            cpuset_limit: cpus("0-7"),
                        }
                    ),
                    (
                        b,
                        AffinityInfo {
                            mask: cpus("0-7,16-23"),
                            cpuset_limit: cpus("16-23"),
                        }
                    ),
                ]
            );
        }
        // Beside tabs at a 2 s interval, the period stays at 1 s.
        let mut both = tabs(&[(a(), DetailTab::Threads)]);
        both.affinity_targets = Arc::from([a()]);
        assert_eq!(
            src.period(&both, &every_ms(2000)),
            Some(Duration::from_secs(1))
        );
    }

    /// E10-005 AC 3 with a Properties tab open (leader decision 2026-10-16): the period is
    /// gcd(interval, 1 s), at least 250 ms. Each interval gives 3 affinity updates in 3 s and
    /// tab reads at the expected spacing. 1600 ms is not a multiple of its 250 ms period, so
    /// tabs read every 6 samples.
    #[test]
    fn affinity_1hz_beside_tabs() {
        let (_tmp, cfg, b) = affinity_root();
        let mut d = tabs(&[(a(), DetailTab::Threads)]);
        d.affinity_targets = Arc::from([a(), b]);
        for (interval, period, spacing) in [
            (500, 500, 500),
            (1000, 1000, 1000),
            (1500, 500, 1500),
            (1600, 250, 1500),
            (2500, 500, 2500),
        ] {
            let s = every_ms(interval);
            let mut src = ProcessDetailSource::new(Arc::clone(&cfg));
            assert_eq!(
                src.period(&d, &s),
                Some(Duration::from_millis(period)),
                "{interval}"
            );
            let mut updates = Vec::new();
            let mut tab_at = Vec::new();
            for t in (0..3000).step_by(period as usize) {
                let before = src.tab_reads(DetailTab::Threads);
                let (next, out) = affinity_step(src, &d, &s, t);
                src = next;
                if src.tab_reads(DetailTab::Threads) > before {
                    tab_at.push(t);
                }
                updates.extend(out);
            }
            assert_eq!(updates.len(), 3, "{interval}");
            assert!(updates.iter().all(|e| e.len() == 2), "{interval}");
            assert_eq!(updates[0][1].1.cpuset_limit, cpus("16-23"));
            let expected: Vec<u64> = (0..3000).step_by(spacing).collect();
            assert_eq!(tab_at, expected, "{interval}");
        }
        for (interval, period) in [(750, 250), (250, 250), (1100, 250), (10_000, 1000)] {
            let src = ProcessDetailSource::new(Arc::clone(&cfg));
            assert_eq!(
                src.period(&d, &every_ms(interval)),
                Some(Duration::from_millis(period)),
                "{interval}"
            );
        }
        // PSS beside tabs without a dialog keeps its two 750 ms parts.
        let src = ProcessDetailSource::new(Arc::clone(&cfg));
        let mut pss = tabs(&[(a(), DetailTab::Threads)]);
        pss.process_columns.insert(ColumnId::Pss);
        pss.detail_keys = Arc::from([a()]);
        assert_eq!(
            src.period(&pss, &every_ms(1500)),
            Some(Duration::from_millis(750))
        );
    }

    /// A gone process is left out; a cgroup without any cpuset file gives no limit.
    #[test]
    fn affinity_gone_and_no_cpuset() {
        let (tmp, cfg, b) = affinity_root();
        fs::remove_file(
            tmp.path()
                .join("sys/fs/cgroup/system.slice/cpuset.cpus.effective"),
        )
        .expect("remove cpuset");
        let gone = key(4201, START_TICKS - 1);
        let d = affinity_demand(&[gone, b]);
        let s = EngineSettings::default();
        let (_src, out) = affinity_step(ProcessDetailSource::new(cfg), &d, &s, 0);
        assert_eq!(
            out,
            vec![vec![(
                b,
                AffinityInfo {
                    mask: cpus("0-7,16-23"),
                    cpuset_limit: CpuSet::empty(),
                }
            )]]
        );
    }

    /// A cgroup path outside tasklens's namespace is never joined under `fs/cgroup`.
    #[test]
    fn cpuset_outside_namespace_is_no_limit() {
        let (_tmp, cfg, _) = affinity_root();
        let mut buf = Vec::new();
        let mut list = SmallVec::new();
        assert_eq!(
            cpuset_effective(&cfg.sys_root, "../../system.slice", &mut buf, &mut list),
            None
        );
        assert_eq!(
            cpuset_effective(&cfg.sys_root, "system.slice/b.service", &mut buf, &mut list),
            Some(cpus("16-23"))
        );
    }

    /// E10-005 AC 4: no dialog and no details give no period, and a dialog that closes stops
    /// the reads.
    #[test]
    fn affinity_off_when_empty() {
        let (_tmp, cfg, b) = affinity_root();
        let s = EngineSettings::default();
        let src = ProcessDetailSource::new(cfg);
        assert_eq!(src.period(&Demand::default(), &s), None);
        let (src, out) = affinity_step(src, &affinity_demand(&[b]), &s, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(src.period(&Demand::default(), &s), None);
        // A forced sample after the dialog closed reads nothing.
        let (src, out) = affinity_step(src, &Demand::default(), &s, 1000);
        assert!(out.is_empty());
        // A dialog that opens again reads at once.
        let (_src, out) = affinity_step(src, &affinity_demand(&[b]), &s, 1100);
        assert_eq!(out.len(), 1);
    }

    fn reply(body: HelperReplyBody) -> HelperReply {
        HelperReply {
            seq: 1,
            last: false,
            body,
        }
    }

    fn file(file: ProcFiles, data: &[u8]) -> HelperReply {
        reply(HelperReplyBody::File {
            file,
            data: data.to_vec(),
        })
    }

    fn os(errno: i32) -> HelperReply {
        reply(HelperReplyBody::Err(HelperError::Os(errno)))
    }

    /// qfx.18 review: a General file the helper could not read keeps its errno's validity, and
    /// EFBIG sets the "too large" error, instead of the field silently staying Restricted.
    #[test]
    fn general_patch_keeps_file_errors() {
        let requested = ProcFiles::EXE | ProcFiles::CWD | ProcFiles::IO | ProcFiles::ROOT;
        // Bit order: IO, CWD, EXE, ROOT. ROOT is missing from the reply.
        let replies = [file(ProcFiles::IO, b"not io\n"), os(2), os(27)];
        let mut too_large = false;
        let files = helper_files(requested, &replies, &mut too_large).expect("per-file errors");
        assert!(!too_large);
        let patch = general_patch(requested, &files, None, &mut too_large);
        assert!(too_large, "EFBIG on exe");
        assert!(matches!(patch.exe, Some(Err(Validity::Failed))));
        assert!(matches!(patch.cwd, Some(Err(Validity::Missing))));
        assert!(matches!(patch.io, Some(Err(Validity::Failed))));
        assert!(matches!(patch.root, Some(Err(Validity::Failed))));

        let replies = [file(ProcFiles::CWD, b"/srv"), os(13)];
        let mut too_large = false;
        let requested = ProcFiles::EXE | ProcFiles::CWD;
        let files = helper_files(requested, &replies, &mut too_large).expect("per-file errors");
        let patch = general_patch(requested, &files, None, &mut too_large);
        assert!(!too_large);
        assert!(matches!(patch.exe, Some(Err(Validity::Restricted))));
        assert!(matches!(&patch.cwd, Some(Ok(cwd)) if &**cwd == "/srv"));
        assert!(patch.io.is_none() && patch.root.is_none(), "not requested");
    }

    /// qfx.18 review round 2: an `smaps` the helper could not read never gives a Valid Memory
    /// maps tab. EACCES and EPERM are Restricted, ENOENT and ESRCH Missing (the target exited),
    /// EFBIG Failed with "too large", any other errno Failed.
    #[test]
    fn maps_from_helper_smaps_errors() {
        const MAPS: &[u8] = b"00400000-00452000 r-xp 00000000 08:02 173521 /usr/bin/x\n";
        let requested = ProcFiles::MAPS | ProcFiles::SMAPS;
        let maps = |smaps: HelperReply, too_large: &mut bool| {
            let replies = [file(ProcFiles::MAPS, MAPS), smaps];
            let files = helper_files(requested, &replies, too_large).expect("per-file errors");
            maps_from_helper(&files, None, too_large)
        };
        for (errno, want) in [
            (1, Validity::Restricted),
            (13, Validity::Restricted),
            (2, Validity::Missing),
            (3, Validity::Missing),
            (5, Validity::Failed),
        ] {
            let mut too_large = false;
            assert!(
                matches!(maps(os(errno), &mut too_large), Err(v) if v == want),
                "errno {errno}"
            );
            assert!(!too_large, "errno {errno}");
        }
        let mut too_large = false;
        assert!(matches!(
            maps(os(27), &mut too_large),
            Err(Validity::Failed)
        ));
        assert!(too_large, "EFBIG");

        let mut too_large = false;
        let smaps = file(
            ProcFiles::SMAPS,
            b"00400000-00452000 r-xp 00000000 08:02 173521 /usr/bin/x\nRss: 8 kB\nPss: 8 kB\nSwap: 0 kB\n",
        );
        let read = maps(smaps, &mut too_large);
        assert!(
            matches!(&read, Ok(TabRead::Maps { regions, .. }) if regions[0].rss == Some(8 * 1024))
        );
        let replies = [file(ProcFiles::MAPS, MAPS)];
        let files = helper_files(requested, &replies, &mut too_large).expect("one file");
        assert!(matches!(
            maps_from_helper(&files, None, &mut too_large),
            Err(Validity::Failed)
        ));
    }

    /// qfx.18 review round 2: the helper's `STATUS` fills the status fields of a General read
    /// whose `status` was Restricted, and a `status` error sets its validity on all of them.
    #[test]
    fn general_patch_status_fields() {
        const STATUS: &[u8] =
            include_bytes!("../../../tl-collect/tests/fixtures/process-edge/proc/4201/status");
        let mut too_large = false;
        let files = [(ProcFiles::STATUS, Ok(STATUS.to_vec()))];
        let patch = general_patch(ProcFiles::STATUS, &files, None, &mut too_large);
        let Some(Ok(fields)) = &patch.status else {
            panic!("status parsed");
        };
        assert_eq!(fields.uids, valid([1000; 4]));
        assert_eq!(fields.no_new_privs, valid(true));
        assert_eq!(fields.seccomp, valid(Seccomp::Filter { filters: 3 }));
        assert_eq!(fields.commit_bytes, valid(232 * 1024));

        let files = [(ProcFiles::STATUS, Err(13))];
        let patch = general_patch(ProcFiles::STATUS, &files, None, &mut too_large);
        assert!(matches!(patch.status, Some(Err(Validity::Restricted))));
        let files = [(ProcFiles::STATUS, Ok(b"no uid line\n".to_vec()))];
        let patch = general_patch(ProcFiles::STATUS, &files, None, &mut too_large);
        assert!(matches!(patch.status, Some(Err(Validity::Failed))));
        assert!(!too_large);
    }

    /// qfx.18 review: a patch sets the helper's validity on a Restricted field and leaves a
    /// field the direct read could read alone.
    #[test]
    fn patch_field_sets_validity_on_restricted() {
        let restricted = || Total {
            value: Arc::<str>::from(""),
            validity: Validity::Restricted,
        };
        let mut field = restricted();
        patch_field(&mut field, &Some(Err(Validity::Missing)));
        assert_eq!(field.validity, Validity::Missing);
        let mut field = restricted();
        patch_field(&mut field, &Some(Ok(Arc::from("/"))));
        assert_eq!((&*field.value, field.validity), ("/", Validity::Valid));
        let mut field = valid(Arc::<str>::from("/home"));
        patch_field(&mut field, &Some(Err(Validity::Failed)));
        assert_eq!((&*field.value, field.validity), ("/home", Validity::Valid));
    }

    /// qfx.18 review: `IdentityChanged` and `NoSuchProcess` drop the PSS entry as for a gone
    /// process; ENOENT is Missing, EFBIG Failed with "too large", EACCES stays Restricted.
    #[test]
    fn pss_from_helper_errors() {
        let files = ProcFiles::SMAPS_ROLLUP;
        let pss = |replies: Vec<HelperReply>, too_large: &mut bool| {
            pss_from_helper(files, Ok(replies), too_large)
        };
        let mut too_large = false;
        for e in [HelperError::IdentityChanged, HelperError::NoSuchProcess] {
            let out = pss(vec![reply(HelperReplyBody::Err(e))], &mut too_large);
            assert!(matches!(out, Some(HelperPss::Gone)));
        }
        assert!(matches!(
            pss(vec![os(2)], &mut too_large),
            Some(HelperPss::Invalid(Validity::Missing))
        ));
        assert!(matches!(
            pss(vec![os(3)], &mut too_large),
            Some(HelperPss::Invalid(Validity::Missing))
        ));
        assert!(pss(vec![os(13)], &mut too_large).is_none());
        assert!(!too_large);
        assert!(matches!(
            pss(vec![os(27)], &mut too_large),
            Some(HelperPss::Invalid(Validity::Failed))
        ));
        assert!(too_large);
        let mut too_large = false;
        let out = pss(vec![file(files, ROLLUP)], &mut too_large);
        assert!(matches!(out, Some(HelperPss::Ok(r)) if r.pss > 0));
        let not_running = pss_from_helper(files, Err(HelperReadError::NotRunning), &mut too_large);
        assert!(not_running.is_none(), "stays Restricted");
    }

    /// qfx.18 review: a queued read is sent only while its slot is demanded.
    #[test]
    fn wanted_slots_follow_the_demand() {
        let k = key(4201, START_TICKS);
        let other = key(4202, START_TICKS);
        let wanted = Wanted {
            details: Arc::from([DetailDemand {
                target: DetailTarget::Process(k),
                tab: DetailTab::Maps,
            }]),
            pss: Arc::from([other]),
        };
        assert!(wanted.contains(HelperSlot::Tab(k, DetailTab::Maps)));
        assert!(wanted.contains(HelperSlot::Tab(k, DetailTab::General)));
        assert!(!wanted.contains(HelperSlot::Tab(k, DetailTab::Environment)));
        assert!(!wanted.contains(HelperSlot::Tab(other, DetailTab::General)));
        assert!(wanted.contains(HelperSlot::Pss(other)));
        assert!(!wanted.contains(HelperSlot::Pss(k)));
    }
}
