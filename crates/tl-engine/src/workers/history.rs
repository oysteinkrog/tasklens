//! The `HistoryRecorder` worker (ADR-012, ADR-004) and its reducer side.
//!
//! Disk and compression work must not run on the reducer. `HistorySink` is the reducer's
//! `RecordSink`: each callback builds one `HistoryMsg` and pushes it into the worker's channel
//! with `try_send`, so it never blocks. With no sender, a full queue or a closed channel the
//! message is dropped and counted; the sink passes the count on with `HistoryMsg::Dropped`, and
//! the worker reports it in its health message.
//!
//! Each worker generation creates its own channel and stores the sender in the `HistorySlot`, so
//! no `mpsc::Receiver` moves into a restartable worker (ADR-004 factory contract).
//!
//! The worker switches between three modes each turn (ADR-012, D-010, PROC-086): recording to
//! the store when `history.enabled` is true, the 10-minute `RollingWindow` when recording is off
//! and the Processes tab is open (`Demand.process_table`), and nothing otherwise. Hiding the
//! window stops the sweep but keeps `process_table`, so the rolling window survives a minimize
//! with a gap in its data.
//!
//! Recording (E30-014, PROC-087, R-025): entering it loads the store in one blocking job, joined
//! with a 2.5 s timeout. Each closed block is appended and retention runs after it. Leaving it,
//! or cancellation, writes the open block and closes the store; with `history.persist = false`
//! the segments of this boot are deleted instead.
//!
//! The state lives in a mutex that each blocking job locks, so a job whose result is lost to
//! cancellation still leaves the store and the open block for the shutdown step.
//!
//! Never hold a `watch` `Ref` across another channel call. This module only uses
//! `borrow_and_clone()` (bead `c11v6v`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::time;
use asupersync::{Cx, Outcome};
use rustix::time::{ClockId, Timespec, clock_gettime};
use tl_collect::SysRoot;
use tl_collect::parse::meminfo::{MemInfo, parse_meminfo};
use tl_history::format::{BLOCK_SECS, SEGMENT_SECS};
use tl_history::recorder::Recorder;
use tl_history::retention;
use tl_history::rolling::RollingWindow;
use tl_history::store::{LoadProblem, LoadReport, Store, StoreConfig};
use tl_model::command::HistoryWhat;
use tl_model::demand::Demand;
use tl_model::devices::DeviceCatalog;
use tl_model::health::{SourceId, SourceState};
use tl_model::history::{
    HistoryLoadProblem, HistoryMetric, HistoryRange, HistorySource, HistoryStoreState, HistoryTop,
    HistoryView, RecordSink,
};
use tl_model::ids::{ProcessKey, SeriesId};
use tl_model::process::ProcessRecord;
use tl_model::series::Sample;
use tl_model::settings::{EngineSettings, HistorySettings};
use tl_model::time::{BootId, TimeNs, now_boot_ns};

use crate::history_query::{self, QueryCtx, QuerySource};
use crate::source::WorkerIo;
use crate::update::{EngineMsg, SourceUpdate, WorkerError};

/// Capacity of the channel each worker generation creates.
pub const HISTORY_CHANNEL_CAPACITY: usize = 512;

/// One event for the `HistoryRecorder` worker.
pub enum HistoryMsg {
    SessionStart {
        epoch: u32,
        t_ns: TimeNs,
    },
    System {
        t_ns: TimeNs,
        series: Box<[(SeriesId, Sample)]>,
    },
    Sweep {
        t_ns: TimeNs,
        records: Arc<[ProcessRecord]>,
    },
    /// `DeviceCatalog` holds its entries in an `Arc`, so the clone is cheap.
    Catalog(DeviceCatalog),
    Query {
        id: u64,
        range: HistoryRange,
        what: HistoryWhat,
    },
    Delete {
        keys: Arc<[ProcessKey]>,
    },
    /// The sink's running count of dropped messages, sent after the next message that got
    /// through.
    Dropped {
        total: u64,
    },
    /// Test seam for `tests/lab_history.rs`: runs on the blocking pool with the worker's mode, in message order.
    #[doc(hidden)]
    Inspect(Box<dyn FnOnce(&Mode) + Send>),
}

/// Each worker generation creates a fresh `mpsc::channel(512)` and stores its `Sender` here; the
/// sink reads it.
pub type HistorySlot = Arc<ArcSwapOption<mpsc::Sender<HistoryMsg>>>;

/// The reducer's `RecordSink` while history is on. A thin forwarder to the worker's channel.
pub struct HistorySink {
    slot: HistorySlot,
    dropped: u64,
    /// The last `dropped` value the worker received.
    reported: u64,
}

impl HistorySink {
    pub fn new(slot: HistorySlot) -> Self {
        HistorySink {
            slot,
            dropped: 0,
            reported: 0,
        }
    }

    /// For `Command::HistoryQuery` and `Command::HistoryDelete` routed by the reducer (E30-A2).
    pub fn command(&mut self, msg: HistoryMsg) {
        self.send(msg);
    }

    /// Messages dropped so far: no sender, queue full or channel closed.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    fn send(&mut self, msg: HistoryMsg) {
        let guard = self.slot.load();
        let Some(tx) = guard.as_deref() else {
            self.dropped += 1;
            return;
        };
        if tx.try_send(msg).is_err() {
            self.dropped += 1;
            return;
        }
        if self.dropped > self.reported
            && tx
                .try_send(HistoryMsg::Dropped {
                    total: self.dropped,
                })
                .is_ok()
        {
            self.reported = self.dropped;
        }
    }
}

impl RecordSink for HistorySink {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]) {
        self.send(HistoryMsg::System {
            t_ns,
            series: series.into(),
        });
    }

    fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>) {
        self.send(HistoryMsg::Sweep {
            t_ns,
            records: Arc::clone(records),
        });
    }

    fn on_device_catalog(&mut self, catalog: &DeviceCatalog) {
        self.send(HistoryMsg::Catalog(catalog.clone()));
    }
}

/// How long one `recv` waits before the loop re-reads demand and settings.
const RECV_WAIT: Duration = Duration::from_millis(500);
/// At most this many messages go into one blocking job: one `recv` plus 63 `try_recv`.
const BATCH_MAX: usize = 64;
/// MemTotal and the clock offset are read this often, and drops are reported at most this often.
/// A store that failed to open or write is opened again after this long (R-249).
const MINUTE_NS: u64 = 60_000_000_000;
/// The load job is joined with this timeout (PROC-087).
const LOAD_TIMEOUT: Duration = Duration::from_millis(2500);
/// `Store::open` stops reading segments after this long.
const LOAD_BUDGET: Duration = Duration::from_secs(2);
/// The shutdown drain is 1000 ms (ADR-004 shutdown step 2). Past this, the purge is skipped.
const PURGE_DEADLINE: Duration = Duration::from_millis(800);
const BLOCK_NS: u64 = BLOCK_SECS * 1_000_000_000;
const SEGMENT_NS: u64 = SEGMENT_SECS * 1_000_000_000;

pub struct HistoryWorkerConfig {
    pub boot: BootId,
    /// `EngineConfig.history_dir` (R-150 item 3). `None` keeps recording in RAM only.
    pub history_dir: Option<PathBuf>,
    /// Test seam for `tests/lab_history.rs`: the load job's join waits this long first, so a
    /// test can make the load miss its 2.5 s timeout in virtual time. `None` in the engine.
    #[doc(hidden)]
    pub load_delay: Option<Duration>,
}

/// What the worker holds (R-262).
// One `Mode` per worker; the rolling window is about 550 bytes.
#[allow(clippy::large_enum_variant)]
pub enum Mode {
    Off,
    Rolling(RollingWindow),
    Recording(RecordingState),
}

/// `retention_h` in ns.
fn retention_ns(h: &HistorySettings) -> u64 {
    u64::from(h.retention_h) * 3_600_000_000_000
}

/// `max_disk_mb` in bytes.
fn max_disk_bytes(h: &HistorySettings) -> u64 {
    u64::from(h.max_disk_mb) * 1_048_576
}

/// Predicts when the store starts a new segment, so the recorder resets its meta scope before
/// that segment's first block opens. `BlockBuilder::reset_meta_scope` only acts when the next
/// block opens, and a gap can move that block past a segment boundary, so the prediction follows
/// every block open and close with the store's own rule (`Store::will_rotate`).
#[derive(Default)]
struct SegmentScope {
    /// Start of the block open in the recorder.
    open_start: Option<TimeNs>,
    /// Start of the store's open segment. `None` when the next append opens a new segment.
    seg_start: Option<TimeNs>,
}

impl SegmentScope {
    /// Before a sweep or system sample at `t`. Returns true when that event opens a block that
    /// starts a new segment.
    fn before_event(&mut self, t: TimeNs) -> bool {
        if let Some(start) = self.open_start {
            if t < start.saturating_add(BLOCK_NS) {
                return false;
            }
            self.close();
        }
        self.open_start = Some(t);
        self.seg_start
            .is_none_or(|seg| t >= seg.saturating_add(SEGMENT_NS))
    }

    /// The open block closes and goes to the store.
    fn close(&mut self) {
        let Some(start) = self.open_start.take() else {
            return;
        };
        if self
            .seg_start
            .is_none_or(|seg| start >= seg.saturating_add(SEGMENT_NS))
        {
            self.seg_start = Some(start);
        }
    }
}

/// The recorder and its store while `history.enabled` is true.
pub struct RecordingState {
    recorder: Recorder,
    /// `None` without a `history_dir`, while loading, and after a failed open or write.
    store: Option<Store>,
    store_state: HistoryStoreState,
    /// `history.persist`, read at shutdown.
    persist: bool,
    /// Retention and disk cap last given to the store, in ns and bytes.
    limits: (u64, u64),
    /// Runtime time in ns when a failed store is opened again.
    retry_at: Option<u64>,
    /// Newest sample time seen. Retention measures age against it.
    last_t_ns: TimeNs,
    scope: SegmentScope,
}

impl RecordingState {
    fn new(boot: BootId, h: &HistorySettings) -> Self {
        RecordingState {
            recorder: Recorder::new(boot),
            store: None,
            store_state: HistoryStoreState::Loading,
            persist: h.persist,
            limits: (retention_ns(h), max_disk_bytes(h)),
            retry_at: None,
            last_t_ns: 0,
            scope: SegmentScope::default(),
        }
    }

    pub fn store_state(&self) -> HistoryStoreState {
        self.store_state
    }

    /// True while a store is open for writing.
    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }

    /// Takes the result of an open. Blocks recorded without a store never reach it: the queue
    /// and the open block are dropped, and the next block starts a new meta scope.
    fn install(&mut self, opened: Opened, now: u64, limits: (u64, u64)) {
        while self.recorder.take_closed().is_some() {}
        let _ = self.recorder.flush();
        self.recorder.reset_meta_scope();
        self.scope = SegmentScope::default();
        self.store = opened.store;
        self.store_state = opened.state;
        self.limits = limits;
        self.retry_at = opened.retry.then(|| now.saturating_add(MINUTE_NS));
    }

    /// True when the store failed and its retry time has come.
    fn retry_due(&self, now: u64) -> bool {
        self.store.is_none() && self.retry_at.is_some_and(|t| now >= t)
    }

    /// `persist` is kept for shutdown; new limits go to the store and apply at the next enforce.
    fn apply_settings(&mut self, h: &HistorySettings) {
        self.persist = h.persist;
        let limits = (retention_ns(h), max_disk_bytes(h));
        if limits != self.limits {
            self.limits = limits;
            if let Some(store) = &mut self.store {
                store.set_limits(limits.0, limits.1);
            }
        }
    }

    fn record(&mut self, msg: HistoryMsg, now: u64) {
        match msg {
            HistoryMsg::System { t_ns, series } => {
                self.before_event(t_ns);
                self.recorder.on_system_sample(t_ns, &series);
            }
            HistoryMsg::Sweep { t_ns, records } => {
                self.before_event(t_ns);
                self.recorder.on_process_sweep(t_ns, &records);
            }
            HistoryMsg::Catalog(catalog) => self.recorder.on_device_catalog(&catalog),
            HistoryMsg::SessionStart { t_ns, .. } => {
                self.scope.close();
                self.recorder.mark_gap(t_ns);
            }
            // `State::apply` handles queries and deletes (E30-015).
            HistoryMsg::Delete { .. }
            | HistoryMsg::Query { .. }
            | HistoryMsg::Dropped { .. }
            | HistoryMsg::Inspect(_) => {}
        }
        self.write(now);
    }

    fn before_event(&mut self, t_ns: TimeNs) {
        self.last_t_ns = self.last_t_ns.max(t_ns);
        if self.scope.before_event(t_ns) {
            self.recorder.reset_meta_scope();
        }
    }

    /// Appends every closed block, then enforces retention. Without a store the blocks are
    /// dropped. A failed append drops the store (R-249); it is opened again after 60 s.
    fn write(&mut self, now: u64) {
        while let Some(cb) = self.recorder.take_closed() {
            let Some(store) = &mut self.store else {
                self.scope.seg_start = None;
                continue;
            };
            if let Err(e) = store.append(cb.block, cb.realtime_offset_ns) {
                // The error text holds a user path: debug level only.
                tracing::debug!("history append failed: {e}");
                tracing::warn!("history store write failed; recording in RAM, retry in 60 s");
                self.store = None;
                self.store_state = HistoryStoreState::Problem(HistoryLoadProblem::OpenFailed);
                self.retry_at = Some(now.saturating_add(MINUTE_NS));
                self.scope.seg_start = None;
                continue;
            }
            if let Err(e) = retention::enforce(store, self.last_t_ns) {
                tracing::debug!("history retention failed: {e}");
            }
        }
    }

    /// Leaving Recording or cancellation: writes the queued and the open block and closes the
    /// store, or with `persist = false` deletes this boot's segments. With `cancel`, the
    /// append and close run masked, and the purge is skipped when the cancel was seen more than
    /// 800 ms ago.
    fn leave(self, cancel: Option<(&Cx, Instant)>) {
        let RecordingState {
            mut recorder,
            store,
            persist,
            ..
        } = self;
        let Some(mut store) = store else {
            return;
        };
        let late = cancel.is_some_and(|(_, seen)| seen.elapsed() > PURGE_DEADLINE);
        if !persist && !late {
            if let Err(e) = retention::purge_current_boot(store) {
                tracing::debug!("history purge failed: {e}");
            }
            return;
        }
        if !persist {
            tracing::warn!("history purge skipped: shutdown took more than 800 ms");
        }
        let mut blocks: Vec<_> = std::iter::from_fn(|| recorder.take_closed()).collect();
        blocks.extend(recorder.flush());
        let finish = || {
            for cb in blocks {
                if let Err(e) = store.append(cb.block, cb.realtime_offset_ns) {
                    tracing::debug!("history append failed: {e}");
                    break;
                }
            }
            if let Err(e) = store.close() {
                tracing::debug!("history close failed: {e}");
            }
        };
        match cancel {
            Some((cx, _)) => cx.masked(finish),
            None => finish(),
        }
    }
}

/// The result of opening the store.
struct Opened {
    store: Option<Store>,
    state: HistoryStoreState,
    /// Open again after 60 s.
    retry: bool,
}

/// Maps a load report one to one. `OpenFailed` drops the store and asks for a retry.
fn opened(store: Store, report: &LoadReport, timed_out: bool) -> Opened {
    warn_bad_deleted_keys(&report.bad_deleted_keys);
    let problem = match report.problem {
        Some(LoadProblem::OpenFailed) => {
            return Opened {
                store: None,
                state: HistoryStoreState::Problem(HistoryLoadProblem::OpenFailed),
                retry: true,
            };
        }
        _ if timed_out => Some(HistoryLoadProblem::TimedOut),
        None => None,
        Some(LoadProblem::EmptyOrTruncated) => Some(HistoryLoadProblem::EmptyOrTruncated),
        Some(LoadProblem::VersionMismatch) => Some(HistoryLoadProblem::VersionMismatch),
        Some(LoadProblem::NoInRange) => Some(HistoryLoadProblem::NoInRange),
        Some(LoadProblem::Stale) => Some(HistoryLoadProblem::Stale),
        Some(LoadProblem::TimedOut) => Some(HistoryLoadProblem::TimedOut),
    };
    Opened {
        store: Some(store),
        state: problem.map_or(HistoryStoreState::Ready, HistoryStoreState::Problem),
        retry: false,
    }
}

/// One warning per deleted-keys file renamed to `.bad` (R-271). It names the boot folder only,
/// never the full path.
fn warn_bad_deleted_keys(paths: &[PathBuf]) {
    for path in paths {
        let boot = path
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        tracing::warn!(
            boot = %boot,
            "history deleted-keys file did not decode; renamed to .bad, that boot's older history is no longer read"
        );
    }
}

/// Prunes other boots and opens the store in one blocking job, joined with a 2.5 s timeout. On
/// timeout the late result is dropped and a fresh store, without loading, is opened instead.
/// `Err` holds the loop's result: `Outcome::Ok` on cancel.
async fn open_store(
    cx: &Cx,
    cfg: &HistoryWorkerConfig,
    h: &HistorySettings,
) -> Result<Opened, Outcome<(), WorkerError>> {
    let Some(root) = cfg.history_dir.clone() else {
        return Ok(Opened {
            store: None,
            state: HistoryStoreState::Problem(HistoryLoadProblem::OpenFailed),
            retry: false,
        });
    };
    let store_cfg = StoreConfig {
        root,
        boot: cfg.boot,
        retention_ns: retention_ns(h),
        max_disk_bytes: max_disk_bytes(h),
        load_existing: h.persist,
    };
    let job_cfg = store_cfg.clone();
    let mut job = spawn(cx, move || {
        let real_now = realtime_offset_ns().saturating_add(now_boot_ns() as i64);
        if let Err(e) = retention::prune_other_boots(
            &job_cfg.root,
            job_cfg.boot,
            real_now,
            job_cfg.retention_ns,
        ) {
            tracing::debug!("history prune failed: {e}");
        }
        Store::open(job_cfg, now_boot_ns(), Instant::now() + LOAD_BUDGET)
    })?;
    let delay = cfg.load_delay;
    let joined = time::timeout(cx.now(), LOAD_TIMEOUT, async {
        if let Some(d) = delay {
            time::sleep(cx.now(), d).await;
        }
        job.join(cx).await
    })
    .await;
    if let Ok(result) = joined {
        let (store, report) = join_result(cx, result)?;
        return Ok(opened(store, &report, false));
    }
    tracing::warn!("history load timed out; starting fresh");
    drop(job);
    let fresh = StoreConfig {
        load_existing: false,
        ..store_cfg
    };
    let mut job = spawn(cx, move || {
        Store::open(fresh, now_boot_ns(), Instant::now() + LOAD_BUDGET)
    })?;
    let (store, report) = join_result(cx, job.join(cx).await)?;
    Ok(opened(store, &report, true))
}

fn spawn<T: Send + 'static>(
    cx: &Cx,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<asupersync::runtime::TaskHandle<T>, Outcome<(), WorkerError>> {
    cx.spawn_blocking(move |_| f())
        .map_err(|e| Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())))
}

/// A joined job's value, or the loop's result: `Ok` when `cx` is cancelled.
fn join_result<T>(cx: &Cx, r: Result<T, JoinError>) -> Result<T, Outcome<(), WorkerError>> {
    match r {
        Ok(v) => Ok(v),
        Err(JoinError::Cancelled(reason)) => {
            if cx.checkpoint().is_err() {
                return Err(Outcome::Ok(()));
            }
            Err(Outcome::Cancelled(reason))
        }
        Err(JoinError::Panicked(payload)) => Err(Outcome::Panicked(payload)),
        Err(JoinError::PolledAfterCompletion) => Err(Outcome::Err(WorkerError::Other(
            "job polled after completion".into(),
        ))),
    }
}

/// The mode that demand and settings ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wanted {
    Off,
    Rolling,
    Recording,
}

fn wanted(demand: &Demand, settings: &EngineSettings) -> Wanted {
    if settings.history.enabled {
        Wanted::Recording
    } else if demand.process_table {
        Wanted::Rolling
    } else {
        Wanted::Off
    }
}

/// A TopAt query: `(id, t, metric, n)`.
type TopQuery = (u64, TimeNs, HistoryMetric, u8);

/// The worker's state. Each blocking job locks it.
struct State {
    mode: Mode,
    boot: BootId,
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME, read once per minute.
    realtime_offset_ns: i64,
    /// `header.version` of the last answer.
    version: u64,
    /// The highest drop total the sink sent.
    dropped: u64,
    /// The last Frame query, answered again after a delete (UI-PRC-037).
    last_frame: Option<(u64, HistoryRange, HistoryWhat)>,
    /// The last TopAt query and its answer, merged into every view.
    last_top: Option<(TopQuery, Arc<HistoryTop>)>,
    /// The last view sent. A TopAt answer goes out as this view with the new top list.
    last_view: Option<HistoryView>,
}

impl State {
    /// Off to Rolling creates the window. Rolling to Off drops it, which frees its RAM at once.
    /// Returns true when a window was created. Recording is entered and left by `run_history`.
    fn switch(&mut self, want: Wanted, boot: BootId) -> bool {
        let rolling = want == Wanted::Rolling;
        match (&self.mode, rolling) {
            (Mode::Off, true) => {
                self.mode = Mode::Rolling(RollingWindow::new(boot));
                true
            }
            (Mode::Rolling(_), false) => {
                self.mode = Mode::Off;
                false
            }
            _ => false,
        }
    }

    /// Reads MemTotal and the clock offset. Runs on the blocking pool.
    fn refresh(&mut self) {
        self.realtime_offset_ns = realtime_offset_ns();
        match &mut self.mode {
            Mode::Off => {}
            Mode::Rolling(window) => {
                if let Some(kib) = read_mem_total_kib() {
                    window.set_mem_total_kib(kib);
                }
            }
            Mode::Recording(rec) => {
                rec.recorder.set_realtime_offset_ns(self.realtime_offset_ns);
                if let Some(kib) = read_mem_total_kib() {
                    rec.recorder.set_mem_total_kib(kib);
                }
            }
        }
    }

    /// Applies a batch and pushes the answers to `out`. Data that arrives while the mode is Off
    /// is dropped. Runs on the blocking pool. `now` is the runtime time in ns.
    fn apply(
        &mut self,
        msgs: &mut Vec<HistoryMsg>,
        out: &mut Vec<SourceUpdate>,
        h: &HistorySettings,
        now: u64,
    ) {
        if let Mode::Recording(rec) = &mut self.mode {
            rec.apply_settings(h);
        }
        for msg in msgs.drain(..) {
            match msg {
                HistoryMsg::Dropped { total } => self.dropped = self.dropped.max(total),
                HistoryMsg::Inspect(f) => f(&self.mode),
                HistoryMsg::Query { id, range, what } => {
                    let view = self.answer(id, range, what);
                    out.push(SourceUpdate::History(Arc::new(view)));
                }
                HistoryMsg::Delete { keys } => {
                    if let Some(view) = self.delete(&keys) {
                        out.push(SourceUpdate::History(Arc::new(view)));
                    }
                }
                msg => match &mut self.mode {
                    Mode::Off => {}
                    Mode::Rolling(window) => record(window, msg),
                    Mode::Recording(rec) => rec.record(msg, now),
                },
            }
        }
    }

    /// Answers a query (E30-015). A Frame query builds a new view; a TopAt query answers the
    /// top list and sends it merged into the last view.
    fn answer(&mut self, id: u64, range: HistoryRange, what: HistoryWhat) -> HistoryView {
        let view = match what {
            HistoryWhat::TopAt { t, metric, n } => {
                let top = history_query::answer_top(self.source(), &self.ctx(), id, t, metric, n);
                self.last_top = Some(((id, t, metric, n), Arc::new(top)));
                match &self.last_view {
                    Some(view) => view.clone(),
                    None => self.frame(id, range, &what),
                }
            }
            what => {
                let view = self.frame(id, range, &what);
                self.last_frame = Some((id, range, what));
                view
            }
        };
        self.publish(view)
    }

    /// "Delete from history" (R-103), then the last Frame and TopAt queries again, so the keys
    /// leave the next view (UI-PRC-037). `None` when no view was sent yet. While the store is
    /// not open, the keys are not saved.
    fn delete(&mut self, keys: &[ProcessKey]) -> Option<HistoryView> {
        match &mut self.mode {
            Mode::Off => {}
            Mode::Rolling(window) => window.delete_keys(keys),
            Mode::Recording(rec) => {
                if let Some(store) = &mut rec.store
                    && let Err(e) = store.delete_keys(keys)
                {
                    // The error text holds a user path: debug level only.
                    tracing::debug!("history delete failed: {e}");
                }
            }
        }
        if let Some(((id, t, metric, n), _)) = self.last_top {
            let top = history_query::answer_top(self.source(), &self.ctx(), id, t, metric, n);
            self.last_top = Some(((id, t, metric, n), Arc::new(top)));
        }
        let view = match &self.last_frame {
            Some((id, range, what)) => self.frame(*id, *range, what),
            None => self.last_view.clone()?,
        };
        Some(self.publish(view))
    }

    /// Bumps the version, merges the last top list and keeps the view as the last one sent.
    fn publish(&mut self, mut view: HistoryView) -> HistoryView {
        self.version += 1;
        view.header.version = self.version;
        view.top = self.last_top.as_ref().map(|(_, top)| Arc::clone(top));
        self.last_view = Some(view.clone());
        view
    }

    fn frame(&self, id: u64, range: HistoryRange, what: &HistoryWhat) -> HistoryView {
        let mut view = history_query::answer_frame(self.source(), &self.ctx(), id, &range, what);
        if matches!(self.mode, Mode::Recording(_)) {
            // While the store loads or after a failed open, there is no data yet.
            view.source = HistorySource::Store;
        }
        view
    }

    fn source(&self) -> QuerySource<'_> {
        match &self.mode {
            Mode::Off => QuerySource::None,
            Mode::Rolling(window) => QuerySource::Rolling(window),
            Mode::Recording(rec) => match &rec.store {
                Some(store) => QuerySource::Store {
                    store,
                    open: rec.recorder.open_snapshot(),
                },
                None => QuerySource::None,
            },
        }
    }

    fn ctx(&self) -> QueryCtx {
        let store_state = match &self.mode {
            Mode::Recording(rec) => rec.store_state,
            Mode::Off | Mode::Rolling(_) => HistoryStoreState::Off,
        };
        QueryCtx {
            boot: self.boot,
            now_t_ns: now_boot_ns(),
            store_state,
            realtime_offset_ns: self.realtime_offset_ns,
            version: self.version,
        }
    }
}

fn record(window: &mut RollingWindow, msg: HistoryMsg) {
    match msg {
        HistoryMsg::System { t_ns, series } => window.on_system_sample(t_ns, &series),
        HistoryMsg::Sweep { t_ns, records } => window.on_process_sweep(t_ns, &records),
        HistoryMsg::Catalog(catalog) => window.on_device_catalog(&catalog),
        HistoryMsg::SessionStart { t_ns, .. } => window.mark_gap(t_ns),
        // `State::apply` handles queries and deletes (E30-015).
        HistoryMsg::Delete { .. } | HistoryMsg::Query { .. } | HistoryMsg::Dropped { .. } => {}
        HistoryMsg::Inspect(_) => {}
    }
}

/// MemTotal in KiB from `/proc/meminfo`, `None` when the file cannot be read or parsed.
fn read_mem_total_kib() -> Option<u64> {
    let buf = std::fs::read(SysRoot::host().proc_path("meminfo")).ok()?;
    let mut info = MemInfo::default();
    parse_meminfo(&buf, &mut info).ok()?;
    Some(info.mem_total / 1024)
}

/// CLOCK_REALTIME minus CLOCK_BOOTTIME, in ns.
fn realtime_offset_ns() -> i64 {
    let ns = |ts: Timespec| i128::from(ts.tv_sec) * 1_000_000_000 + i128::from(ts.tv_nsec);
    let real = ns(clock_gettime(ClockId::Realtime));
    let boot = ns(clock_gettime(ClockId::Boottime));
    i64::try_from(real - boot).unwrap_or(0)
}

/// Clears the slot when the generation ends on any path, a panic included. It only clears the
/// slot while it still holds this generation's sender.
struct SlotGuard {
    slot: HistorySlot,
    mine: Option<Arc<mpsc::Sender<HistoryMsg>>>,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.slot.compare_and_swap(&self.mine, None);
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Cancellation: writes and closes the store (bounded, see `RecordingState::leave`).
fn shut_down(cx: &Cx, state: &Mutex<State>) -> Outcome<(), WorkerError> {
    let seen = Instant::now();
    let mode = std::mem::replace(&mut lock(state).mode, Mode::Off);
    if let Mode::Recording(rec) = mode {
        rec.leave(Some((cx, seen)));
    }
    Outcome::Ok(())
}

/// One generation of the `HistoryRecorder` worker. Returns `Outcome::Ok(())` when `cx` is
/// cancelled and `Outcome::Err` when a job cannot be spawned or the reducer's queue is closed,
/// so the supervisor restarts it.
pub async fn run_history(
    cx: Cx,
    io: WorkerIo,
    slot: HistorySlot,
    cfg: HistoryWorkerConfig,
) -> Outcome<(), WorkerError> {
    let (tx, mut rx) = mpsc::channel::<HistoryMsg>(HISTORY_CHANNEL_CAPACITY);
    let mine = Some(Arc::new(tx));
    slot.store(mine.clone());
    let _guard = SlotGuard { slot, mine };

    let state = Arc::new(Mutex::new(State {
        mode: Mode::Off,
        boot: cfg.boot,
        realtime_offset_ns: 0,
        version: 0,
        dropped: 0,
        last_frame: None,
        last_top: None,
        last_view: None,
    }));
    let mut msgs: Vec<HistoryMsg> = Vec::with_capacity(BATCH_MAX);
    // Runtime time in ns of the next MemTotal and clock offset read.
    let mut next_refresh = 0u64;
    let mut reported = 0u64;
    let mut last_report: Option<u64> = None;

    // Every return on cancel goes through `shut_down`.
    macro_rules! stop {
        ($outcome:expr) => {{
            let outcome = $outcome;
            if matches!(outcome, Outcome::Ok(())) {
                return shut_down(&cx, &state);
            }
            return outcome;
        }};
    }

    loop {
        if let Ok(Ok(msg)) = time::timeout(cx.now(), RECV_WAIT, rx.recv(&cx)).await {
            msgs.push(msg);
            while msgs.len() < BATCH_MAX {
                match rx.try_recv() {
                    Ok(msg) => msgs.push(msg),
                    Err(_) => break,
                }
            }
        }
        if cx.checkpoint().is_err() {
            return shut_down(&cx, &state);
        }

        let settings = io.settings.borrow_and_clone();
        let want = wanted(&io.demand.borrow_and_clone(), &settings);
        let h = settings.history.clone();
        let now = cx.now().as_nanos();
        let recording = matches!(lock(&state).mode, Mode::Recording(_));
        if !recording && want == Wanted::Recording {
            // Rolling to Recording drops the window. Messages wait in the channel during the load.
            lock(&state).mode = Mode::Recording(RecordingState::new(cfg.boot, &h));
            let opened = match open_store(&cx, &cfg, &h).await {
                Ok(o) => o,
                Err(outcome) => stop!(outcome),
            };
            if let Mode::Recording(rec) = &mut lock(&state).mode {
                rec.install(
                    opened,
                    cx.now().as_nanos(),
                    (retention_ns(&h), max_disk_bytes(&h)),
                );
            }
            next_refresh = now;
            if cx.checkpoint().is_err() {
                return shut_down(&cx, &state);
            }
        } else if recording && want != Wanted::Recording {
            let Mode::Recording(rec) = std::mem::replace(&mut lock(&state).mode, Mode::Off) else {
                unreachable!("the mode was Recording above");
            };
            let mut job = match spawn(&cx, move || rec.leave(None)) {
                Ok(job) => job,
                Err(outcome) => stop!(outcome),
            };
            if let Err(outcome) = join_result(&cx, job.join(&cx).await) {
                stop!(outcome);
            }
        } else if recording
            && matches!(&lock(&state).mode, Mode::Recording(rec) if rec.retry_due(now))
        {
            let opened = match open_store(&cx, &cfg, &h).await {
                Ok(o) => o,
                Err(outcome) => stop!(outcome),
            };
            if let Mode::Recording(rec) = &mut lock(&state).mode {
                // A retry that fails again keeps the first problem until a store opens.
                let keep = opened.store.is_none().then_some(rec.store_state);
                rec.install(
                    opened,
                    cx.now().as_nanos(),
                    (retention_ns(&h), max_disk_bytes(&h)),
                );
                if let Some(problem) = keep {
                    rec.store_state = problem;
                }
            }
        }
        if lock(&state).switch(want, cfg.boot) {
            next_refresh = now;
        }
        let refresh = !matches!(lock(&state).mode, Mode::Off) && now >= next_refresh;
        if refresh {
            next_refresh = now.saturating_add(MINUTE_NS);
        }

        if !msgs.is_empty() || refresh {
            let job_state = Arc::clone(&state);
            let mut batch = std::mem::take(&mut msgs);
            let job = spawn(&cx, move || {
                let mut out = Vec::new();
                let mut st = lock(&job_state);
                if refresh {
                    st.refresh();
                }
                st.apply(&mut batch, &mut out, &h, now);
                (batch, out)
            });
            let mut job = match job {
                Ok(job) => job,
                Err(outcome) => stop!(outcome),
            };
            let out;
            (msgs, out) = match join_result(&cx, job.join(&cx).await) {
                Ok(done) => done,
                Err(outcome) => stop!(outcome),
            };
            for update in out {
                if let Some(outcome) = send(&cx, &io, update).await {
                    stop!(outcome);
                }
            }
        }

        // R-249: the state stays `Running`; the text says how many events were dropped.
        let now = cx.now().as_nanos();
        let dropped = lock(&state).dropped;
        let due = last_report.is_none_or(|t| now.saturating_sub(t) >= MINUTE_NS);
        if dropped > reported && due {
            reported = dropped;
            last_report = Some(now);
            let health = SourceUpdate::Health {
                id: SourceId::HistoryRecorder,
                state: SourceState::Running,
                last_error: Some(format!("History dropped {reported} events").into()),
                t_ns: now_boot_ns(),
            };
            if let Some(outcome) = send(&cx, &io, health).await {
                stop!(outcome);
            }
        }
    }
}

/// Sends one update. Returns the loop's result when the loop must stop: `Ok` on cancel,
/// `Err(Closed)` when the reducer is gone.
async fn send(cx: &Cx, io: &WorkerIo, update: SourceUpdate) -> Option<Outcome<(), WorkerError>> {
    match io.updates.send(cx, EngineMsg::Source(update)).await {
        Ok(()) => None,
        Err(mpsc::SendError::Cancelled(_)) => Some(Outcome::Ok(())),
        Err(_) => Some(Outcome::Err(WorkerError::Closed)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_model::history::HistoryMetric;
    use tl_model::ids::GroupKey;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    fn record(pid: u32, meta: &Arc<ProcessMeta>) -> ProcessRecord {
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: meta.clone(),
            group: GroupKey(0),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn sweep(n: u32) -> Arc<[ProcessRecord]> {
        let meta = Arc::new(ProcessMeta {
            comm: "x".into(),
            name: "x".into(),
            exe: None,
            cmdline: None,
            user: "user".into(),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            search_lower: "x".into(),
        });
        (1..=n).map(|pid| record(pid, &meta)).collect()
    }

    fn slot_with_channel() -> (HistorySlot, mpsc::Receiver<HistoryMsg>) {
        let (tx, rx) = mpsc::channel(HISTORY_CHANNEL_CAPACITY);
        let slot: HistorySlot = Arc::new(ArcSwapOption::from_pointee(tx));
        (slot, rx)
    }

    #[test]
    fn sink_without_sender_drops() {
        let mut sink = HistorySink::new(Arc::new(ArcSwapOption::empty()));
        sink.on_process_sweep(1, &sweep(3));
        assert_eq!(sink.dropped(), 1);
    }

    #[test]
    fn sink_never_blocks_when_full() {
        let (slot, rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let records = sweep(1);
        for t in 0..600 {
            sink.on_process_sweep(t, &records);
        }
        assert_eq!(rx.len(), 512);
        assert_eq!(sink.dropped(), 88);
    }

    #[test]
    fn sink_reports_drops_after_next_send() {
        let (slot, mut rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let records = sweep(1);
        for t in 0..520 {
            sink.on_process_sweep(t, &records);
        }
        while rx.try_recv().is_ok() {}
        sink.on_process_sweep(600, &records);
        assert!(matches!(
            rx.try_recv(),
            Ok(HistoryMsg::Sweep { t_ns: 600, .. })
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(HistoryMsg::Dropped { total: 8 })
        ));
        // The count is sent once.
        sink.on_process_sweep(601, &records);
        assert!(matches!(
            rx.try_recv(),
            Ok(HistoryMsg::Sweep { t_ns: 601, .. })
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sweep_arc_not_copied() {
        let (slot, mut rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let records = sweep(2_000);
        sink.on_process_sweep(7, &records);
        match rx.try_recv() {
            Ok(HistoryMsg::Sweep { t_ns, records: got }) => {
                assert_eq!(t_ns, 7);
                assert!(Arc::ptr_eq(&got, &records));
            }
            _ => panic!("expected one Sweep message"),
        }
        assert_eq!(sink.dropped(), 0);
    }

    #[test]
    fn command_forwards_query() {
        let (slot, mut rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let what = HistoryWhat::TopAt {
            t: 5,
            metric: HistoryMetric::Cpu,
            n: 3,
        };
        sink.command(HistoryMsg::Query {
            id: 42,
            range: HistoryRange::At(5),
            what: what.clone(),
        });
        match rx.try_recv() {
            Ok(HistoryMsg::Query {
                id,
                range,
                what: got,
            }) => {
                assert_eq!(id, 42);
                assert_eq!(range, HistoryRange::At(5));
                assert_eq!(got, what);
            }
            _ => panic!("expected one Query message"),
        }
        assert_eq!(sink.dropped(), 0);
    }

    /// Keeps the fields of every warn event as text.
    #[derive(Clone, Default)]
    struct Warnings(Arc<std::sync::Mutex<Vec<String>>>);

    struct Fields(String);

    impl tracing::field::Visit for Fields {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push_str(&format!("{}={value:?} ", field.name()));
        }
    }

    impl tracing::Subscriber for Warnings {
        fn register_callsite(
            &self,
            _: &'static tracing::Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::sometimes()
        }
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN {
                let mut fields = Fields(String::new());
                event.record(&mut fields);
                self.0.lock().expect("warnings lock").push(fields.0);
            }
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    #[test]
    fn bad_deleted_keys_warn_once_each_without_path() {
        let warnings = Warnings::default();
        let report = LoadReport {
            problem: None,
            segments: 0,
            span: None,
            bad_deleted_keys: vec![PathBuf::from(
                "/home/someone/.local/state/tasklens/history/00000000000000ab/deleted-keys.bin.bad",
            )],
        };
        tracing::subscriber::with_default(warnings.clone(), || {
            warn_bad_deleted_keys(&report.bad_deleted_keys);
        });
        let got = warnings.0.lock().expect("warnings lock").clone();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("00000000000000ab"), "{got:?}");
        assert!(!got[0].contains("/home"), "no full path: {got:?}");
    }

    const S: u64 = 1_000_000_000;

    #[test]
    fn scope_resets_at_segment_start() {
        let mut scope = SegmentScope::default();
        // The first block starts the first segment.
        assert!(scope.before_event(1_000 * S));
        // Blocks every 60 s up to 1_540 s stay in it.
        for k in 1..10 {
            assert!(!scope.before_event((1_000 + 60 * k) * S));
            assert!(!scope.before_event((1_000 + 60 * k + 30) * S));
        }
        // 1_600 s is 600 s past the segment start.
        assert!(scope.before_event(1_600 * S));
    }

    #[test]
    fn scope_resets_after_gap_across_boundary() {
        let mut scope = SegmentScope::default();
        assert!(scope.before_event(1_000 * S));
        for k in 1..=8 {
            assert!(!scope.before_event((1_000 + 60 * k) * S));
        }
        // Block open at 1_480 s; the block after it would start at 1_540 s, inside the segment.
        // A gap closes it, and the next block starts at 1_650 s, in a new segment.
        scope.close();
        assert!(scope.before_event(1_650 * S));
        assert!(!scope.before_event(1_710 * S));
    }
}
