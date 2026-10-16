//! The `ProcessDetail` worker (ADR-008, ADR-016): PSS, USS and swap for `detail_keys`, and the
//! Properties tab payloads of `Demand.details`.
//!
//! `/proc/<pid>/smaps_rollup` is read at 1 Hz, only while a PSS or USS column is shown and
//! `detail_keys` (visible rows plus one viewport of overscan, at most 512) is not empty. The
//! `tl-collect` `PssReader` keeps up to 128 fds open between samples (ADR-016), checks
//! `start_ticks` when it opens a file and closes the fds of keys that left the demand.
//!
//! Tabs (E10-004): the demanded tab of each demanded process target (at most `DETAILS_MAX`) is
//! read at `processes.interval_ms`: Threads, File descriptors, Memory maps and Security. Memory
//! maps reads `smaps` for the RSS at most once per `SMAPS_MIN_PERIOD` and keeps the last RSS in
//! between. Environment is read when its tab is first demanded and again only on
//! `DetailRefresh`. A tab is sent only when the reader output changed, so an unchanged tab keeps
//! its `Arc` in the reducer. EACCES gives an empty `Restricted` payload. A gone process resends
//! the last payload as `Missing`, another read error resends it as `Failed`. Targets that leave
//! the demand are forgotten. Tab updates go out through `take_extra`. When PSS is not due the
//! sample itself is the last tab update or `Health { Running }`, never an empty `Pss`.
//!
//! Per-key errors become `Validity`, never `WorkerError`. Environment values, command lines and
//! paths are never logged (ADR-015).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::supervision::ManagedGeneration;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_collect::SysRoot;
use tl_collect::parse::topology::parse_cpu_list;
use tl_collect::proc::detail::{
    self as rd, DetailRead, EnvInfo, EnvRead, FdInfo, FdType, LsmLabel, MapNodeInfo, MapRegionInfo,
    SecurityInfo, ThreadInfo, ThreadReader, WchanValue,
};
use tl_collect::proc::meta::{PasswdUserNames, ReadBufs, UserNames};
use tl_collect::proc::smaps::{PssReader, RollupRead};
use tl_collect::sysroot::read_file;
use tl_model::actions::SchedPolicy;
use tl_model::demand::{DETAILS_MAX, Demand, DetailDemand};
use tl_model::details::{
    CapRow, DetailTab, DetailTarget, Elevated, EnvDetail, FdDetail, FdKind, FdRow,
    GroupMembershipRow, IdTriple, MapNode, MapRegion, MapsDetail, NsKind, NsRow, NsState, Seccomp,
    SecurityDetail, TabPayload, ThreadRow,
};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::process::ProcState;
use tl_model::settings::EngineSettings;
use tl_model::table::Total;
use tl_model::time::{TimeNs, now_boot_ns};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{DetailsUpdate, PssEntry, SourceUpdate};
use crate::workers::WorkerSpec;
use crate::workers::processes::{MAX_INTERVAL_MS, MIN_INTERVAL_MS};

/// PSS and USS are read once a second (ADR-008).
pub const PSS_PERIOD: Duration = Duration::from_secs(1);

/// Memory maps reads `smaps` (per-region RSS) at most this often (ADR-008).
pub const SMAPS_MIN_PERIOD: Duration = Duration::from_secs(1);

/// The `smaps` due check allows a sample this much early, so scheduling jitter on a 250 or
/// 500 ms grid does not skip a whole period.
const SMAPS_SLACK_NS: u64 = 5_000_000;

/// Read limit for `<root>/etc/group`.
const GROUP_FILE_MAX: usize = 1024 * 1024;

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

/// True while a PSS or USS column is shown.
pub fn wants_pss(d: &Demand) -> bool {
    d.process_columns.contains(ColumnId::Pss) || d.process_columns.contains(ColumnId::Uss)
}

/// The tabs read from demand. General follows in tasklens-dwy.13. Strings is never read from
/// demand, the reducer builds AppGeneral, and AppProcesses has no payload.
fn reads_from_demand(tab: DetailTab) -> bool {
    matches!(
        tab,
        DetailTab::Threads
            | DetailTab::Fds
            | DetailTab::Maps
            | DetailTab::Security
            | DetailTab::Environment
    )
}

/// True when `details` holds a process target with a tab this worker reads.
fn wants_tabs(details: &[DetailDemand]) -> bool {
    details
        .iter()
        .any(|d| matches!(d.target, DetailTarget::Process(_)) && reads_from_demand(d.tab))
}

/// `processes.interval_ms`, clamped as the Processes worker clamps it.
fn tab_interval(s: &EngineSettings) -> Duration {
    Duration::from_millis(u64::from(
        s.interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS),
    ))
}

/// Samples per read for a read that is due every `every` on a sample grid of `period`. Rounds
/// up, so a read never runs more often than `every`.
fn stride(every: Duration, period: Duration) -> u32 {
    let period = period.as_nanos().max(1);
    u32::try_from(every.as_nanos().div_ceil(period))
        .unwrap_or(u32::MAX)
        .max(1)
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

/// The state of one tab of one target.
#[derive(Default)]
struct TabState {
    /// The reader output behind the last payload sent. `None` after a read without data.
    last: Option<TabRead>,
    /// The last payload sent and its validity.
    published: Option<(Arc<TabPayload>, Validity)>,
    /// Memory maps: boot time of the last `smaps` read.
    last_read_ns: Option<TimeNs>,
    /// Environment: read once; later reads only on `DetailRefresh`.
    env_loaded: bool,
}

/// Everything kept for one demanded process. Dropped when the target leaves the demand.
#[derive(Default)]
struct TargetState {
    /// Indexed by `DetailTab::index()`.
    tabs: [TabState; 9],
    /// The Threads tab CPU baselines.
    threads: Option<ThreadReader>,
    /// Memory maps: RSS by region start from the last `smaps` read.
    rss: FxHashMap<u64, u64>,
}

pub struct ProcessDetailSource {
    cfg: Arc<EngineConfig>,
    pss: PssReader,
    /// Set by `period`: the keys of the latest demand.
    keys: RefCell<Arc<[ProcessKey]>>,
    /// Reused every sample.
    buf: Vec<u8>,
    reads: Vec<(ProcessKey, RollupRead)>,
    /// Commands the worker owns, drained at the start of every sample. E10-005, E10-006 and
    /// E10-A3 add their handlers; until then they are logged by kind and dropped.
    pending: Vec<WorkerCommand>,
    /// Set by `period`: the first `DETAILS_MAX` entries of `Demand.details`.
    details: RefCell<Arc<[DetailDemand]>>,
    /// Set by `period`: samples per PSS read and per tab read.
    pss_every: Cell<u32>,
    tabs_every: Cell<u32>,
    pss_stride: Stride,
    tabs_stride: Stride,
    /// True while the demand holds more than `DETAILS_MAX` targets, so the warning is logged
    /// once per overflow.
    over_cap: Cell<bool>,
    cap_warnings: Cell<u32>,
    /// `DetailRefresh` commands for the next sample.
    refresh: Vec<(DetailTarget, DetailTab)>,
    targets: FxHashMap<ProcessKey, TargetState>,
    /// Tab updates for `take_extra`.
    extra: VecDeque<SourceUpdate>,
    bufs: ReadBufs,
    /// `<root>/etc/passwd`, loaded at the first Security read.
    users: Option<PasswdUserNames>,
    group_file: Vec<u8>,
    ticks_per_sec: u64,
    online_cpus: u32,
    /// Test hook for the Threads CPU clock.
    thread_clock: Option<fn() -> (TimeNs, u64)>,
    /// Reader calls per tab and `smaps` reads, for tests.
    tab_reads: [u32; 9],
    smaps_reads: u32,
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
        async move { source::run(cx, io, ProcessDetailSource::new(cfg)).await }
    })
}

impl ProcessDetailSource {
    pub fn new(cfg: Arc<EngineConfig>) -> Self {
        Self {
            cfg,
            pss: PssReader::new(),
            keys: RefCell::new(Arc::from([])),
            buf: Vec::new(),
            reads: Vec::new(),
            pending: Vec::new(),
            details: RefCell::new(Arc::from([])),
            pss_every: Cell::new(1),
            tabs_every: Cell::new(1),
            pss_stride: Stride::default(),
            tabs_stride: Stride::default(),
            over_cap: Cell::new(false),
            cap_warnings: Cell::new(0),
            refresh: Vec::new(),
            targets: FxHashMap::default(),
            extra: VecDeque::new(),
            bufs: ReadBufs::default(),
            users: None,
            group_file: Vec::new(),
            ticks_per_sec: rustix::param::clock_ticks_per_second(),
            online_cpus: 1,
            thread_clock: None,
            tab_reads: [0; 9],
            smaps_reads: 0,
        }
    }

    /// Commands waiting for the next sample.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
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
        self.targets.len()
    }

    /// `sample()` is `self.sample_at(now_boot_ns())`; tests call this with an injected clock.
    /// `now` times the `smaps` cap and stamps the PSS update.
    pub fn sample_at(mut self, now: TimeNs) -> (Self, SourceUpdate) {
        for cmd in self.pending.drain(..) {
            // Kind only: the arguments can hold search patterns.
            tracing::debug!(
                kind = cmd.kind(),
                "process detail command has no handler yet"
            );
        }
        self.pss_stride.set(self.pss_every.get());
        self.tabs_stride.set(self.tabs_every.get());
        self.read_tabs(now);

        let keys = self.keys.borrow().clone();
        // With no keys the reader still runs while fds are cached, so hidden columns close them.
        let pss_due = if keys.is_empty() {
            self.pss.cached() > 0
        } else {
            self.pss_stride.ready()
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
        let entries = self
            .reads
            .iter()
            .filter_map(|&(key, read)| {
                let (pss, uss, swap, validity) = match read {
                    RollupRead::Ok(r) => (r.pss, r.uss, r.swap, Validity::Valid),
                    RollupRead::Restricted => (0, 0, 0, Validity::Restricted),
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

    /// Reads the demanded tabs that are due and queues the changed ones in `extra`.
    fn read_tabs(&mut self, now: TimeNs) {
        let details = self.details.borrow().clone();
        self.targets.retain(|key, _| {
            details
                .iter()
                .any(|d| d.target == DetailTarget::Process(*key))
        });
        let refresh = std::mem::take(&mut self.refresh);
        let periodic = wants_tabs(&details) && self.tabs_stride.ready();
        for d in details.iter() {
            let DetailTarget::Process(key) = d.target else {
                continue;
            };
            if !reads_from_demand(d.tab) {
                continue;
            }
            let refreshed = refresh.contains(&(d.target, d.tab));
            let wanted = if d.tab == DetailTab::Environment {
                refreshed
                    || !self
                        .targets
                        .get(&key)
                        .is_some_and(|t| t.tabs[d.tab.index()].env_loaded)
            } else {
                periodic || refreshed
            };
            if wanted && let Some(update) = self.read_tab(key, d.tab, now) {
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
        if tab == DetailTab::Security {
            let base = root_base(&self.cfg.sys_root);
            if self.users.is_none() {
                self.users = Some(PasswdUserNames::load(&base.join("etc/passwd")));
            }
            if read_file(
                &base.join("etc/group"),
                &mut self.group_file,
                GROUP_FILE_MAX,
            )
            .is_err()
            {
                self.group_file.clear();
            }
        }
        let root = &self.cfg.sys_root;
        let bufs = &mut self.bufs;
        let state = self.targets.entry(key).or_default();
        let read = match tab {
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
                detail_result(rd::read_fds(root, key, bufs, &mut rows), tab)
                    .map(|()| TabRead::Fds(rows))
            }
            DetailTab::Maps => {
                let with_rss = state.tabs[tab.index()]
                    .last_read_ns
                    .is_none_or(|t| due_ns(t, now, SMAPS_MIN_PERIOD));
                let mut regions = Vec::new();
                let mut nodes = Vec::new();
                let read = rd::read_maps(root, key, with_rss, bufs, &mut regions, &mut nodes);
                detail_result(read, tab).map(|()| {
                    if with_rss {
                        self.smaps_reads += 1;
                        state.tabs[tab.index()].last_read_ns = Some(now);
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
                state.tabs[tab.index()].env_loaded = true;
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

        let slot = &mut state.tabs[tab.index()];
        let (payload, validity) = match read {
            Ok(read) => {
                let validity = read.validity();
                let same_validity = slot.published.as_ref().is_some_and(|(_, v)| *v == validity);
                if same_validity && slot.last.as_ref() == Some(&read) {
                    return None;
                }
                let users = self.users.get_or_insert_with(PasswdUserNames::default);
                let payload = Arc::new(build_payload(&read, users, &self.group_file, root));
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
}

impl Source for ProcessDetailSource {
    fn id(&self) -> SourceId {
        SourceId::ProcessDetail
    }

    /// PSS alone runs at 1 Hz, tabs alone at `processes.interval_ms`, both at the shorter of the
    /// two, with each read on its own stride of samples.
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
        let pss = shown && !d.detail_keys.is_empty();
        let tabs = wants_tabs(&details);
        *self.details.borrow_mut() = details;
        let interval = tab_interval(s);
        let period = match (pss, tabs) {
            (true, true) => Some(PSS_PERIOD.min(interval)),
            (true, false) => Some(PSS_PERIOD),
            (false, true) => Some(interval),
            (false, false) => None,
        };
        let grid = period.unwrap_or(PSS_PERIOD);
        self.pss_every.set(stride(PSS_PERIOD, grid));
        self.tabs_every.set(stride(interval, grid));
        period
    }

    /// The Threads tab CPU baselines start over.
    fn reset_baselines(&mut self) {
        for target in self.targets.values_mut() {
            target.threads = None;
        }
    }

    fn command(&mut self, cmd: &WorkerCommand) -> bool {
        match cmd {
            WorkerCommand::DetailRefresh { target, tab } => {
                self.refresh.push((*target, *tab));
                true
            }
            WorkerCommand::ComputeHashes(_)
            | WorkerCommand::VerifyPackage(_)
            | WorkerCommand::StringScan { .. }
            | WorkerCommand::CancelStringScan(_)
            | WorkerCommand::HandleSearch { .. }
            | WorkerCommand::CancelHandleSearch(_) => {
                self.pending.push(cmd.clone());
                true
            }
            _ => false,
        }
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(now_boot_ns())
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.extra.pop_front()
    }
}

/// True when at least `every` has passed from `last` to `now`, less `SMAPS_SLACK_NS`.
fn due_ns(last: TimeNs, now: TimeNs, every: Duration) -> bool {
    let every = u64::try_from(every.as_nanos()).unwrap_or(u64::MAX);
    now.saturating_sub(last).saturating_add(SMAPS_SLACK_NS) >= every
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

/// The tl-model payload of one reader output.
fn build_payload(
    read: &TabRead,
    users: &mut dyn UserNames,
    group_file: &[u8],
    root: &SysRoot,
) -> TabPayload {
    match read {
        TabRead::Threads(rows) => TabPayload::Threads(rows.iter().map(thread_row).collect()),
        TabRead::Fds(rows) => TabPayload::Fds(Arc::new(fd_detail(rows))),
        TabRead::Maps { regions, nodes } => TabPayload::Maps(Arc::new(maps_detail(regions, nodes))),
        TabRead::Security(info) => {
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

/// tl-model has no unresolved socket kind. An unresolved socket shows as `unix` until the
/// connections map resolves its `socket_inode`.
fn fd_kind(kind: FdType) -> FdKind {
    match kind {
        FdType::File => FdKind::File,
        FdType::Directory => FdKind::Directory,
        FdType::Tcp => FdKind::Tcp,
        FdType::Udp => FdKind::Udp,
        FdType::Unix | FdType::Socket => FdKind::Unix,
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
        FdType::AnonInode => FdKind::AnonInode,
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
    let seccomp = match info.seccomp {
        Some(0) => valid(Seccomp::Disabled),
        Some(1) => valid(Seccomp::Strict),
        Some(2) => valid(Seccomp::Filter {
            filters: info.seccomp_filters.unwrap_or(0),
        }),
        _ => total(Seccomp::Disabled, Validity::Unsupported),
    };
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

/// A Security payload with no data: every field carries `validity`.
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
    use tl_model::details::ScanId;
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

    /// `period`, then `sample`, as the source loop does.
    fn run(src: ProcessDetailSource, d: &Demand) -> (ProcessDetailSource, Vec<PssEntry>) {
        src.period(d, &EngineSettings::default());
        let (src, update) = src.sample();
        (src, entries(update))
    }

    #[test]
    fn command_routing() {
        let (_tmp, cfg, keys) = fake_procs(1);
        let mut src = ProcessDetailSource::new(cfg);
        assert!(src.command(&WorkerCommand::ComputeHashes(keys[0])));
        assert_eq!(src.pending_len(), 1);
        assert!(src.command(&WorkerCommand::CancelStringScan(ScanId(1))));
        assert!(src.command(&WorkerCommand::VerifyPackage(keys[0])));
        assert_eq!(src.pending_len(), 3);
        assert!(!src.command(&WorkerCommand::RefreshNow));
        assert!(!src.command(&WorkerCommand::Reprobe));
        assert!(!src.command(&WorkerCommand::SetupStatus));
        assert_eq!(src.pending_len(), 3);
        // DetailRefresh is handled, not queued.
        assert!(src.command(&WorkerCommand::DetailRefresh {
            target: DetailTarget::Process(keys[0]),
            tab: DetailTab::Environment,
        }));
        assert_eq!(src.pending_len(), 3);
        let (src, _) = src.sample();
        assert_eq!(src.pending_len(), 0);
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

    /// `period`, `sample_at(now_ms)`, then every update the loop would send: the tab updates
    /// in order and the sample's own update.
    fn step(
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

    /// E10-004 AC 1. General moved to tasklens-dwy.13, so only Threads is read here.
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
            let want = if tab == DetailTab::Threads { 4 } else { 0 };
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

    /// E10-004 AC 3.
    #[test]
    fn environment_once_then_on_refresh() {
        let (tmp, cfg) = edge_root();
        let d = tabs(&[(a(), DetailTab::Environment)]);
        let s = every_ms(500);
        let mut src = ProcessDetailSource::new(cfg);
        let (next, out, _) = step(src, &d, &s, 0);
        src = next;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2, Validity::Valid);
        assert!(env_vars(&out[0].1).contains(&("API_KEY".into(), "abc".into())));

        let environ = tmp.path().join("proc/4201/environ");
        fs::write(&environ, b"NEW=1\0").expect("rewrite environ");
        for t in 1..10 {
            let (next, out, _) = step(src, &d, &s, t * 500);
            src = next;
            assert!(out.is_empty());
        }
        assert_eq!(src.tab_reads(DetailTab::Environment), 1);

        assert!(src.command(&WorkerCommand::DetailRefresh {
            target: DetailTarget::Process(a()),
            tab: DetailTab::Environment,
        }));
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
        assert_eq!(row(0).kind, FdKind::Unix);
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
}
