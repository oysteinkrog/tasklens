//! The `MetricHistory` worker (ADR-017, ADR-004): feeds the device metric history store of
//! `tl_history::metrics` and answers `Demand.device_history_range` with the `DeviceHistory`
//! snapshot component.
//!
//! The store is open while `Demand.device_history` and `history.device_metrics` are both true.
//! Turning `history.device_metrics` off closes it and deletes `$XDG_STATE_HOME/tasklens/metrics`.
//!
//! Data path: the reducer's `MetricSink` sends each system sample batch and every new device
//! catalog into the worker's channel with `try_send`, so it never blocks. A full queue drops the
//! batch, which the store reads as a gap. The worker stores the channel's sender in the
//! `MetricHistorySlot` only while the store is open, so nothing is queued while it is off. Each
//! open gets a new sender generation, and the sink sends the latest catalog to every new
//! generation before its first samples.
//!
//! Series are stored by `(persistent_key, MetricId)` (ADR-017 "Series identity") and mapped back
//! to the current catalog's `DeviceIdx` on read, so a renumbered device continues its series.
//!
//! Read path: while `Demand.device_history_range` is set, the worker reads the range's tier once
//! per tier point (1 s, 10 s or 60 s) and whenever the request changes, and publishes at most
//! 1,024 min/max-decimated points per series. A series whose points did not change keeps its
//! `Arc`, and a read that changed nothing is not sent. When the range is cleared, one empty
//! `DeviceHistory` frees the points.
//!
//! Disk work runs on the blocking pool, under a mutex that each job locks. The event detector and
//! the timeline (ADR-017 "Event detector", UI-PRF-057) are not part of this module yet.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::time;
use asupersync::{Cx, Outcome};
use rustc_hash::FxHashMap;
use rustix::time::{ClockId, Timespec, clock_gettime};
use tl_history::metrics::{self, MetricConfig, MetricHistory, MetricSlot, MetricSpan, Tier};
use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
use tl_model::devices::DeviceCatalog;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, SeriesId};
use tl_model::series::Sample;
use tl_model::snapshot::{DeviceHistory, HistPoint};
use tl_model::time::{BootId, TimeNs, now_boot_ns};

use crate::source::WorkerIo;
use crate::update::{EngineMsg, SourceUpdate, WorkerError};

/// Capacity of the channel each open creates.
pub const METRIC_CHANNEL_CAPACITY: usize = 512;
/// How long one `recv` waits before the loop re-reads demand and settings.
const RECV_WAIT: Duration = Duration::from_millis(500);
/// At most this many messages go into one blocking job.
const BATCH_MAX: usize = 64;
/// The store is ticked this often: it writes due minutes, ages the ring and drops idle spans.
const TICK_NS: u64 = 1_000_000_000;
/// A store that failed to open is tried again after this long.
const RETRY_NS: u64 = 60_000_000_000;
/// `MetricHistory::open` stops loading the 1 s ring after this long (ADR-012 start budget).
const LOAD_BUDGET: Duration = Duration::from_secs(2);

/// One event for the `MetricHistory` worker.
pub enum MetricMsg {
    /// One system sample batch. Each sample carries its own `t_ns`.
    System(Box<[(SeriesId, Sample)]>),
    /// `DeviceCatalog` holds its entries in an `Arc`, so the clone is cheap.
    Catalog(DeviceCatalog),
}

/// The sender of one open store. `generation` tells the sink that a new store needs the catalog.
pub struct MetricTx {
    pub generation: u64,
    pub tx: mpsc::Sender<MetricMsg>,
}

/// Holds the open store's sender; empty while the store is closed.
pub type MetricHistorySlot = Arc<ArcSwapOption<MetricTx>>;

/// Sender generations, unique per process.
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The reducer side: a thin forwarder to the worker's channel.
pub struct MetricSink {
    slot: MetricHistorySlot,
    /// The newest catalog the reducer published.
    catalog: Option<DeviceCatalog>,
    /// The sender generation that received `catalog`.
    catalog_sent_to: Option<u64>,
}

impl MetricSink {
    pub fn new(slot: MetricHistorySlot) -> Self {
        MetricSink {
            slot,
            catalog: None,
            catalog_sent_to: None,
        }
    }

    /// Sends the batch when a store is open. A store that has not received the catalog gets it
    /// first; without a catalog the samples cannot be keyed and are dropped.
    pub fn on_system_sample(&mut self, _t_ns: TimeNs, series: &[(SeriesId, Sample)]) {
        let guard = self.slot.load();
        let Some(tx) = guard.as_deref() else {
            return;
        };
        if self.sync_catalog(tx) {
            let _ = tx.tx.try_send(MetricMsg::System(series.into()));
        }
    }

    pub fn on_device_catalog(&mut self, catalog: &DeviceCatalog) {
        self.catalog = Some(catalog.clone());
        self.catalog_sent_to = None;
        let guard = self.slot.load();
        if let Some(tx) = guard.as_deref() {
            self.sync_catalog(tx);
        }
    }

    /// True when `tx` has the newest catalog, sending it now if needed.
    fn sync_catalog(&mut self, tx: &MetricTx) -> bool {
        if self.catalog_sent_to == Some(tx.generation) {
            return true;
        }
        let Some(catalog) = &self.catalog else {
            return false;
        };
        if tx.tx.try_send(MetricMsg::Catalog(catalog.clone())).is_err() {
            return false;
        }
        self.catalog_sent_to = Some(tx.generation);
        true
    }
}

/// The tier a range is stored at: 1 s for 1 min, 5 min and 1 h, 10 s for 24 h, 1 min for 7 d
/// (UI-PRF-055).
pub fn range_tier(range: DeviceRange) -> Tier {
    match range {
        DeviceRange::M1 | DeviceRange::M5 | DeviceRange::H1 => Tier::S1,
        DeviceRange::H24 => Tier::S10,
        DeviceRange::D7 => Tier::M1,
    }
}

/// The finest tier that still holds a point `age_ns` old: 1 s within the last hour, 10 s within
/// the last 24 hours, else 1 min (UI-PRF-057 "Paused view").
pub fn finest_tier_at(age_ns: i64) -> Tier {
    Tier::ALL
        .into_iter()
        .find(|t| age_ns <= t.retention_ns() as i64)
        .unwrap_or(Tier::M1)
}

/// The tier and the `[from, to]` CLOCK_REALTIME span that answer `req` at `now_realtime_ns`. A
/// live range uses its own tier. A paused range ends at its time (never after now) and uses the
/// coarser of its own tier and the finest tier that still covers that end.
pub fn plan(req: DeviceHistoryRequest, now_realtime_ns: i64) -> (Tier, i64, i64) {
    let natural = range_tier(req.range);
    let (tier, to) = match req.end_realtime_ns {
        None => (natural, now_realtime_ns),
        Some(end) => {
            let to = end.min(now_realtime_ns);
            (natural.max(finest_tier_at(now_realtime_ns - to)), to)
        }
    };
    let from = to.saturating_sub(req.range.span_ns() as i64);
    (tier, from, to)
}

/// One published series: its id in the current catalog and its points.
type HistSeries = (SeriesId, Arc<[HistPoint]>);

/// What the last `DeviceHistory` held, to reuse its `Arc`s and skip unchanged reads.
struct Published {
    request: DeviceHistoryRequest,
    tier: Tier,
    step_ns: u64,
    since: Option<i64>,
    series: Arc<[HistSeries]>,
    /// Boottime of the read.
    read_at: TimeNs,
}

/// The worker's state. Each blocking job locks it.
pub struct State {
    store: Option<MetricHistory>,
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME given to the open store.
    realtime_offset_ns: i64,
    catalog: DeviceCatalog,
    /// Persistent key to its index in `catalog`. A present entry wins over an absent one.
    by_key: FxHashMap<Arc<str>, DeviceIdx>,
    /// The store handle of each live series; `None` when its device is not in the catalog.
    slots: FxHashMap<SeriesId, Option<MetricSlot>>,
    published: Option<Published>,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        State {
            store: None,
            realtime_offset_ns: 0,
            catalog: DeviceCatalog::default(),
            by_key: FxHashMap::default(),
            slots: FxHashMap::default(),
            published: None,
        }
    }

    /// Takes an opened store. `realtime_offset_ns` must be the one in its `MetricConfig`.
    pub fn install(&mut self, store: MetricHistory, realtime_offset_ns: i64) {
        self.store = Some(store);
        self.realtime_offset_ns = realtime_offset_ns;
        self.slots.clear();
    }

    pub fn is_open(&self) -> bool {
        self.store.is_some()
    }

    /// The open store, for tests.
    pub fn store_mut(&mut self) -> Option<&mut MetricHistory> {
        self.store.as_mut()
    }

    /// Ends every point and writes every minute not yet written, then drops the store.
    pub fn close(&mut self, now_t_ns: TimeNs) -> Result<(), tl_history::HistoryError> {
        self.slots.clear();
        match self.store.take() {
            Some(mut store) => store.close(now_t_ns),
            None => Ok(()),
        }
    }

    /// A new catalog: keys are mapped again and series registered again on their next sample.
    pub fn set_catalog(&mut self, catalog: DeviceCatalog) {
        self.by_key.clear();
        for e in catalog.entries.iter() {
            let replace = match self.by_key.get(&e.persistent_key) {
                None => true,
                Some(&idx) => e.present && catalog.get(idx).is_some_and(|old| !old.present),
            };
            if replace {
                self.by_key.insert(Arc::clone(&e.persistent_key), e.idx);
            }
        }
        self.catalog = catalog;
        self.slots.clear();
    }

    /// Folds one sample batch into the store. The 1 Hz tier views are skipped: the store builds
    /// its own per-second points from the base series.
    pub fn push(&mut self, series: &[(SeriesId, Sample)]) {
        let State {
            store,
            catalog,
            slots,
            ..
        } = self;
        let Some(store) = store.as_mut() else {
            return;
        };
        for &(id, sample) in series {
            if id.metric.is_tier_1hz() {
                continue;
            }
            let slot = *slots.entry(id).or_insert_with(|| {
                let entry = catalog.get(id.device)?;
                store.register(&entry.persistent_key, &entry.display_name, id.metric)
            });
            if let Some(slot) = slot {
                store.push(slot, sample);
            }
        }
    }

    pub fn apply(&mut self, msg: MetricMsg) {
        match msg {
            MetricMsg::System(series) => self.push(&series),
            MetricMsg::Catalog(catalog) => self.set_catalog(catalog),
        }
    }

    /// Writes due minutes and applies retention (`MetricHistory::tick`).
    pub fn tick(&mut self, now_t_ns: TimeNs) -> Result<(), tl_history::HistoryError> {
        match self.store.as_mut() {
            Some(store) => store.tick(now_t_ns),
            None => Ok(()),
        }
    }

    /// The `DeviceHistory` to send for `request` at `now_t_ns`, or `None` when nothing needs
    /// sending: the request and the data are unchanged, or the last read is less than one tier
    /// point old. A cleared request sends one empty `DeviceHistory`.
    pub fn answer(
        &mut self,
        request: Option<DeviceHistoryRequest>,
        now_t_ns: TimeNs,
    ) -> Result<Option<DeviceHistory>, tl_history::HistoryError> {
        let Some(request) = request else {
            return Ok(self.published.take().map(|_| self.empty()));
        };
        let Some(store) = self.store.as_mut() else {
            return Ok(None);
        };
        let now_rt = (now_t_ns as i64).saturating_add(self.realtime_offset_ns);
        let (tier, from, to) = plan(request, now_rt);
        if let Some(p) = &self.published
            && p.request == request
            && p.tier == tier
            && now_t_ns < p.read_at.saturating_add(tier.point_ns())
        {
            return Ok(None);
        }
        let span = store.read(tier, from, to, now_t_ns)?;
        let since = store.since_realtime_ns(tier);
        let (series, reused) = self.map_series(span);
        let step_ns = series_step(tier, from, to);
        let unchanged = self.published.as_ref().is_some_and(|p| {
            p.request == request
                && p.tier == tier
                && p.step_ns == step_ns
                && p.since == since
                && reused
                && p.series.len() == series.len()
        });
        let series: Arc<[_]> = series.into();
        if unchanged && let Some(p) = &mut self.published {
            p.read_at = now_t_ns;
            return Ok(None);
        }
        self.published = Some(Published {
            request,
            tier,
            step_ns,
            since,
            series: Arc::clone(&series),
            read_at: now_t_ns,
        });
        Ok(Some(DeviceHistory {
            request: Some(request),
            realtime_offset_ns: self.realtime_offset_ns,
            since_realtime_ns: since,
            step_ns,
            series,
            ..DeviceHistory::default()
        }))
    }

    /// The span's series under their current `SeriesId`, sorted, with the `Arc` of the last
    /// publish where the points are equal. The bool is true when every series was reused.
    fn map_series(&self, span: MetricSpan) -> (Vec<HistSeries>, bool) {
        let mut out: Vec<HistSeries> = span
            .series
            .into_iter()
            .filter_map(|(key, points)| {
                let device = *self.by_key.get(&key.device_key)?;
                let id = SeriesId {
                    device,
                    metric: key.metric,
                };
                let points: Arc<[HistPoint]> = points
                    .iter()
                    .map(|p| HistPoint {
                        t_realtime_ns: p.t_realtime_ns,
                        mean: p.mean,
                        min: p.min,
                        max: p.max,
                    })
                    .collect();
                Some((id, points))
            })
            .collect();
        out.sort_by_key(|(id, _)| *id);
        let mut all_reused = true;
        let prev = self.published.as_ref().map(|p| &p.series);
        for (id, points) in &mut out {
            let old = prev.and_then(|s| {
                s.binary_search_by_key(id, |(sid, _)| *sid)
                    .ok()
                    .map(|i| &s[i].1)
            });
            match old {
                Some(old) if same_points(old, points) => *points = Arc::clone(old),
                _ => all_reused = false,
            }
        }
        (out, all_reused)
    }

    fn empty(&self) -> DeviceHistory {
        DeviceHistory {
            realtime_offset_ns: self.realtime_offset_ns,
            ..DeviceHistory::default()
        }
    }
}

/// The bucket width `MetricHistory::read` uses for `[from, to]`.
fn series_step(tier: Tier, from: i64, to: i64) -> u64 {
    metrics::step_ns(tier, from, to.max(from))
}

/// Equal times and bit-equal values, so two gaps (NaN) compare equal.
fn same_points(a: &[HistPoint], b: &[HistPoint]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.t_realtime_ns == y.t_realtime_ns
                && x.mean.to_bits() == y.mean.to_bits()
                && x.min.to_bits() == y.min.to_bits()
                && x.max.to_bits() == y.max.to_bits()
        })
}

pub struct MetricWorkerConfig {
    pub boot: BootId,
    /// `$XDG_STATE_HOME/tasklens/metrics`.
    pub root: PathBuf,
}

/// CLOCK_REALTIME minus CLOCK_BOOTTIME, in ns.
fn realtime_offset_ns() -> i64 {
    let ns = |ts: Timespec| i128::from(ts.tv_sec) * 1_000_000_000 + i128::from(ts.tv_nsec);
    let real = ns(clock_gettime(ClockId::Realtime));
    let boot = ns(clock_gettime(ClockId::Boottime));
    i64::try_from(real - boot).unwrap_or(0)
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clears the slot when the generation ends on any path, a panic included, while it still holds
/// this generation's sender.
struct SlotGuard {
    slot: MetricHistorySlot,
    mine: Arc<MetricTx>,
}

impl SlotGuard {
    fn open(&self) {
        self.slot.store(Some(Arc::clone(&self.mine)));
    }

    fn close(&self) {
        self.slot
            .compare_and_swap(&Some(Arc::clone(&self.mine)), None);
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.close();
    }
}

/// Cancellation: writes the open minutes and closes the store, masked.
fn shut_down(cx: &Cx, state: &Mutex<State>) -> Outcome<(), WorkerError> {
    let mut st = lock(state);
    if st.is_open()
        && let Err(e) = cx.masked(|| st.close(now_boot_ns()))
    {
        // The error text holds a user path: debug level only.
        tracing::debug!("device history close failed: {e}");
    }
    Outcome::Ok(())
}

/// One generation of the `MetricHistory` worker. Returns `Outcome::Ok(())` when `cx` is
/// cancelled and `Outcome::Err` when a job cannot be spawned or the reducer's queue is closed, so
/// the supervisor restarts it.
pub async fn run_metric_history(
    cx: Cx,
    io: WorkerIo,
    slot: MetricHistorySlot,
    cfg: MetricWorkerConfig,
) -> Outcome<(), WorkerError> {
    let (tx, mut rx) = mpsc::channel::<MetricMsg>(METRIC_CHANNEL_CAPACITY);
    let guard = SlotGuard {
        slot,
        mine: Arc::new(MetricTx {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            tx,
        }),
    };
    let state = Arc::new(Mutex::new(State::new()));
    let mut msgs: Vec<MetricMsg> = Vec::with_capacity(BATCH_MAX);
    // The controller sent `Running` before this generation started.
    let mut health: (SourceState, Option<Arc<str>>) = (SourceState::Running, None);
    let mut retry_at: Option<u64> = None;
    let mut purged = false;
    let mut next_tick = 0u64;

    macro_rules! stop {
        ($outcome:expr) => {{
            let outcome = $outcome;
            if matches!(outcome, Outcome::Ok(())) {
                return shut_down(&cx, &state);
            }
            return outcome;
        }};
    }
    macro_rules! report {
        ($state:expr, $error:expr) => {{
            let next: (SourceState, Option<Arc<str>>) = ($state, $error);
            if next != health {
                health = next.clone();
                let update = SourceUpdate::Health {
                    id: SourceId::MetricHistory,
                    state: next.0,
                    last_error: next.1,
                    t_ns: now_boot_ns(),
                };
                if let Some(outcome) = send(&cx, &io, update).await {
                    stop!(outcome);
                }
            }
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
        let demand = io.demand.borrow_and_clone();
        let enabled = settings.history.device_metrics;
        let want = enabled && demand.device_history;
        let max_disk_bytes = u64::from(settings.history.device_metrics_max_mb) << 20;
        let now = cx.now().as_nanos();
        let open = lock(&state).is_open();

        if open && !want {
            guard.close();
            msgs.clear();
            while rx.try_recv().is_ok() {}
            let job_state = Arc::clone(&state);
            let job = spawn(&cx, move || {
                let mut st = lock(&job_state);
                if let Err(e) = st.close(now_boot_ns()) {
                    tracing::debug!("device history close failed: {e}");
                }
                st.answer(None, now_boot_ns()).ok().flatten()
            });
            let empty = match job {
                Ok(mut job) => match join_result(&cx, job.join(&cx).await) {
                    Ok(empty) => empty,
                    Err(outcome) => stop!(outcome),
                },
                Err(outcome) => stop!(outcome),
            };
            if let Some(empty) = empty
                && let Some(outcome) =
                    send(&cx, &io, SourceUpdate::DeviceHistory(Arc::new(empty))).await
            {
                stop!(outcome);
            }
        }
        if !enabled && !purged {
            let root = cfg.root.clone();
            let job = spawn(&cx, move || metrics::purge(&root));
            let result = match job {
                Ok(mut job) => join_result(&cx, job.join(&cx).await),
                Err(outcome) => stop!(outcome),
            };
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::debug!("device history purge failed: {e}"),
                Err(outcome) => stop!(outcome),
            }
            purged = true;
        }
        purged &= !enabled;

        if !want {
            retry_at = None;
            report!(SourceState::Off, None);
            continue;
        }
        if !lock(&state).is_open() {
            if retry_at.is_some_and(|t| now < t) {
                continue;
            }
            let config = MetricConfig {
                root: cfg.root.clone(),
                boot: cfg.boot,
                realtime_offset_ns: realtime_offset_ns(),
                max_disk_bytes,
                sync: true,
            };
            let offset = config.realtime_offset_ns;
            let job = spawn(&cx, move || {
                MetricHistory::open(config, now_boot_ns(), Instant::now() + LOAD_BUDGET)
            });
            let opened = match job {
                Ok(mut job) => match join_result(&cx, job.join(&cx).await) {
                    Ok(opened) => opened,
                    Err(outcome) => stop!(outcome),
                },
                Err(outcome) => stop!(outcome),
            };
            match opened {
                Ok((store, report)) => {
                    if report.timed_out {
                        tracing::warn!("device history load timed out; the last hour starts empty");
                    }
                    lock(&state).install(store, offset);
                    guard.open();
                    retry_at = None;
                    next_tick = 0;
                    report!(SourceState::Running, None);
                }
                Err(e) => {
                    // The error text holds a user path: debug level only.
                    tracing::debug!("device history open failed: {e}");
                    retry_at = Some(cx.now().as_nanos().saturating_add(RETRY_NS));
                    report!(
                        SourceState::Failed,
                        Some("Device history could not open its folder".into())
                    );
                    continue;
                }
            }
        }

        let now = cx.now().as_nanos();
        if msgs.is_empty() && now < next_tick {
            continue;
        }
        let tick = now >= next_tick;
        if tick {
            next_tick = now.saturating_add(TICK_NS);
        }
        let request = demand.device_history_range;
        let job_state = Arc::clone(&state);
        let mut batch = std::mem::take(&mut msgs);
        let job = spawn(&cx, move || {
            let mut st = lock(&job_state);
            if let Some(store) = st.store_mut() {
                store.set_max_disk_bytes(max_disk_bytes);
            }
            for msg in batch.drain(..) {
                st.apply(msg);
            }
            let now_t_ns = now_boot_ns();
            let mut error = None;
            if tick && let Err(e) = st.tick(now_t_ns) {
                tracing::debug!("device history write failed: {e}");
                error = Some(());
            }
            let answer = match st.answer(request, now_t_ns) {
                Ok(answer) => answer,
                Err(e) => {
                    tracing::debug!("device history read failed: {e}");
                    None
                }
            };
            (batch, answer, error)
        });
        let (batch, answer, error) = match job {
            Ok(mut job) => match join_result(&cx, job.join(&cx).await) {
                Ok(done) => done,
                Err(outcome) => stop!(outcome),
            },
            Err(outcome) => stop!(outcome),
        };
        msgs = batch;
        if let Some(history) = answer
            && let Some(outcome) =
                send(&cx, &io, SourceUpdate::DeviceHistory(Arc::new(history))).await
        {
            stop!(outcome);
        }
        if tick {
            // R-249: a failed write keeps the state `Running`; the text says what failed.
            let text = error.map(|()| Arc::from("Device history could not write to disk"));
            report!(SourceState::Running, text);
        }
    }
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
    use std::path::Path;

    use tl_history::metrics::DEFAULT_MAX_DISK_BYTES;
    use tl_model::devices::{DeviceEntry, DeviceIdentity};
    use tl_model::ids::MetricId;
    use tl_model::series::SampleFlags;

    use super::*;

    const SEC: u64 = 1_000_000_000;
    const HOUR: i64 = 3_600 * SEC as i64;
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME of the fake boot.
    const OFFSET: i64 = 1_700_000_000 * SEC as i64;

    fn tempdir() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&base).expect("create target/tmp");
        tempfile::Builder::new()
            .prefix("metric-history-")
            .tempdir_in(base)
            .expect("tempdir")
    }

    fn open(root: &Path, boot: u64, offset: i64, now_t_ns: TimeNs) -> State {
        let cfg = MetricConfig {
            root: root.to_path_buf(),
            boot: BootId(boot),
            realtime_offset_ns: offset,
            max_disk_bytes: DEFAULT_MAX_DISK_BYTES,
            sync: false,
        };
        let (store, _) =
            MetricHistory::open(cfg, now_t_ns, Instant::now() + LOAD_BUDGET).expect("open store");
        let mut st = State::new();
        st.install(store, offset);
        st
    }

    fn entry(idx: u16, identity: DeviceIdentity, key: &str, present: bool) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from(key),
            present,
            parent: None,
            persistent_key: Arc::from(key),
            net_kind: None,
            hot_c: None,
        }
    }

    fn disk(name: &str) -> DeviceIdentity {
        DeviceIdentity::Disk {
            major: 8,
            minor: 0,
            name: Arc::from(name),
            wwn_or_serial: None,
        }
    }

    fn catalog(version: u64, entries: Vec<DeviceEntry>) -> DeviceCatalog {
        DeviceCatalog {
            version,
            entries: entries.into(),
        }
    }

    fn sid(device: u16, metric: MetricId) -> SeriesId {
        SeriesId {
            device: DeviceIdx(device),
            metric,
        }
    }

    fn sample(t_ns: TimeNs, value: f32) -> Sample {
        Sample {
            t_ns,
            value,
            flags: SampleFlags::VALID,
        }
    }

    /// The valid means of one series, oldest first.
    fn means(h: &DeviceHistory, id: SeriesId) -> Vec<f32> {
        h.get(id)
            .map(|p| p.iter().filter(|p| !p.is_gap()).map(|p| p.mean).collect())
            .unwrap_or_default()
    }

    fn live(range: DeviceRange) -> Option<DeviceHistoryRequest> {
        Some(DeviceHistoryRequest {
            range,
            end_realtime_ns: None,
        })
    }

    #[test]
    fn plan_picks_tier_and_span() {
        let now = 100 * 86_400 * SEC as i64;
        let req = |range, end| DeviceHistoryRequest {
            range,
            end_realtime_ns: end,
        };
        assert_eq!(
            plan(req(DeviceRange::H1, None), now),
            (Tier::S1, now - HOUR, now)
        );
        assert_eq!(
            plan(req(DeviceRange::H24, None), now),
            (Tier::S10, now - 24 * HOUR, now)
        );
        assert_eq!(
            plan(req(DeviceRange::D7, None), now),
            (Tier::M1, now - 7 * 24 * HOUR, now)
        );
        // Paused: the coarser of the range's tier and the finest tier that covers the end.
        let half_hour = now - HOUR / 2;
        assert_eq!(plan(req(DeviceRange::M5, Some(half_hour)), now).0, Tier::S1);
        assert_eq!(
            plan(req(DeviceRange::M5, Some(now - 2 * HOUR)), now).0,
            Tier::S10
        );
        assert_eq!(
            plan(req(DeviceRange::H1, Some(now - 3 * 24 * HOUR)), now),
            (Tier::M1, now - 73 * HOUR, now - 3 * 24 * HOUR)
        );
        assert_eq!(plan(req(DeviceRange::D7, Some(half_hour)), now).0, Tier::M1);
        // A paused end after now is read as now.
        assert_eq!(
            plan(req(DeviceRange::M1, Some(now + HOUR)), now),
            (Tier::S1, now - 60 * SEC as i64, now)
        );
    }

    /// ADR-017 verification: a series written under one `DeviceIdx` continues under its device
    /// key after the catalog renumbers it, and again after a restart on a new boot.
    #[test]
    fn renumbered_device_continues_its_series() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        st.set_catalog(catalog(
            1,
            vec![
                entry(0, DeviceIdentity::CpuPackage, "cpu", true),
                entry(1, disk("sda"), "disk:WWN-A", true),
            ],
        ));
        for i in 0..5 {
            let t = t0 + i * SEC;
            st.push(&[
                (sid(0, MetricId::CPU_TOTAL), sample(t, 10.0 + i as f32)),
                (sid(1, MetricId::DISK_READ), sample(t, 100.0 + i as f32)),
                // The 1 Hz tier view is not stored.
                (sid(1, MetricId::DISK_WRITE.with_tier_1hz()), sample(t, 7.0)),
            ]);
        }

        // Hotplug: the disk comes back as idx 3; idx 1 is now another disk.
        st.set_catalog(catalog(
            2,
            vec![
                entry(0, DeviceIdentity::CpuPackage, "cpu", true),
                entry(1, disk("sdb"), "disk:WWN-B", true),
                entry(2, disk("sda"), "disk:WWN-A", false),
                entry(3, disk("sda"), "disk:WWN-A", true),
            ],
        ));
        for i in 5..8 {
            let t = t0 + i * SEC;
            st.push(&[(sid(3, MetricId::DISK_READ), sample(t, 100.0 + i as f32))]);
        }
        let now = t0 + 10 * SEC;
        st.tick(now).expect("tick");
        let h = st
            .answer(live(DeviceRange::M1), now)
            .expect("read")
            .expect("sent");
        // The newest second of each series is still open, so not readable yet.
        assert_eq!(
            means(&h, sid(3, MetricId::DISK_READ)),
            [100.0, 101.0, 102.0, 103.0, 104.0, 105.0, 106.0]
        );
        assert!(h.get(sid(1, MetricId::DISK_READ)).is_none());
        assert!(h.get(sid(2, MetricId::DISK_READ)).is_none());
        assert!(h.get(sid(3, MetricId::DISK_WRITE)).is_none());
        assert_eq!(
            means(&h, sid(0, MetricId::CPU_TOTAL)),
            [10.0, 11.0, 12.0, 13.0]
        );
        assert_eq!(h.realtime_offset_ns, OFFSET);
        assert!(h.series.windows(2).all(|w| w[0].0 < w[1].0));
        // Closing writes the open seconds too.
        st.close(now).expect("close");

        // A reboot: boottime starts again and the disk is idx 7 now. The last hour comes back
        // from disk under the new number, with a gap for the time the machine was off.
        let offset2 = OFFSET + 1_020 * SEC as i64;
        let t1 = 5 * SEC;
        let mut st = open(dir.path(), 2, offset2, t1);
        st.set_catalog(catalog(1, vec![entry(7, disk("sda"), "disk:WWN-A", true)]));
        st.push(&[(sid(7, MetricId::DISK_READ), sample(t1, 200.0))]);
        st.push(&[(sid(7, MetricId::DISK_READ), sample(t1 + SEC, 201.0))]);
        let now = t1 + 3 * SEC;
        st.tick(now).expect("tick");
        let h = st
            .answer(live(DeviceRange::M1), now)
            .expect("read")
            .expect("sent");
        let points = h.get(sid(7, MetricId::DISK_READ)).expect("series");
        let valid: Vec<f32> = points
            .iter()
            .filter(|p| !p.is_gap())
            .map(|p| p.mean)
            .collect();
        assert_eq!(
            valid,
            [
                100.0, 101.0, 102.0, 103.0, 104.0, 105.0, 106.0, 107.0, 200.0
            ]
        );
        let last_old = points.iter().rposition(|p| p.mean == 107.0).expect("old");
        assert!(points[last_old + 1].is_gap());
        assert_eq!(h.realtime_offset_ns, offset2);
    }

    #[test]
    fn answer_reuses_unchanged_series_and_skips_unchanged_reads() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        st.set_catalog(catalog(
            1,
            vec![
                entry(0, DeviceIdentity::CpuPackage, "cpu", true),
                entry(1, DeviceIdentity::Memory, "memory", true),
            ],
        ));
        for i in 0..10 {
            let t = t0 + i * SEC;
            st.push(&[
                (sid(0, MetricId::CPU_TOTAL), sample(t, i as f32)),
                (sid(1, MetricId::MEM_USED), sample(t, 50.0)),
            ]);
        }
        // Memory's second 10 is still open, so not yet in the ring.
        st.push(&[(sid(1, MetricId::MEM_USED), sample(t0 + 10 * SEC, 60.0))]);
        let paused = Some(DeviceHistoryRequest {
            range: DeviceRange::M1,
            end_realtime_ns: Some((t0 + 12 * SEC) as i64 + OFFSET),
        });
        let now = t0 + 13 * SEC;
        let first = st.answer(paused, now).expect("read").expect("sent");
        assert_eq!(first.request, paused);
        assert_eq!(first.step_ns, SEC);
        assert_eq!(means(&first, sid(1, MetricId::MEM_USED)).len(), 10);
        // Within one tier point nothing is read again.
        assert!(st.answer(paused, now + SEC / 4).expect("read").is_none());

        // Memory's second 10 closes; the CPU series keeps its `Arc`.
        st.push(&[(sid(1, MetricId::MEM_USED), sample(t0 + 11 * SEC, 70.0))]);
        let second = st.answer(paused, now + SEC).expect("read").expect("sent");
        let cpu = sid(0, MetricId::CPU_TOTAL);
        assert!(Arc::ptr_eq(
            first.get(cpu).expect("cpu"),
            second.get(cpu).expect("cpu")
        ));
        assert_eq!(
            means(&second, sid(1, MetricId::MEM_USED)).last(),
            Some(&60.0)
        );

        // Nothing changed: no new component.
        assert!(st.answer(paused, now + 2 * SEC).expect("read").is_none());

        // Clearing the range sends one empty component, then nothing.
        let empty = st.answer(None, now + 3 * SEC).expect("read").expect("sent");
        assert!(empty.series.is_empty() && empty.request.is_none());
        assert_eq!(empty.realtime_offset_ns, OFFSET);
        assert!(st.answer(None, now + 4 * SEC).expect("read").is_none());
    }

    #[test]
    fn sink_sends_the_catalog_to_each_new_store_first() {
        let slot: MetricHistorySlot = Arc::new(ArcSwapOption::empty());
        let mut sink = MetricSink::new(Arc::clone(&slot));
        let batch = [(sid(0, MetricId::CPU_TOTAL), sample(SEC, 1.0))];
        // No store open: nothing is queued anywhere.
        sink.on_device_catalog(&catalog(1, Vec::new()));
        sink.on_system_sample(SEC, &batch);

        let (tx, mut rx) = mpsc::channel::<MetricMsg>(8);
        slot.store(Some(Arc::new(MetricTx { generation: 1, tx })));
        sink.on_system_sample(SEC, &batch);
        sink.on_system_sample(2 * SEC, &batch);
        let kinds: Vec<&str> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| match m {
                MetricMsg::Catalog(_) => "catalog",
                MetricMsg::System(_) => "system",
            })
            .collect();
        assert_eq!(kinds, ["catalog", "system", "system"]);

        // A new generation gets the catalog again.
        let (tx, mut rx) = mpsc::channel::<MetricMsg>(8);
        slot.store(Some(Arc::new(MetricTx { generation: 2, tx })));
        sink.on_system_sample(3 * SEC, &batch);
        assert!(matches!(rx.try_recv(), Ok(MetricMsg::Catalog(_))));
        assert!(matches!(rx.try_recv(), Ok(MetricMsg::System(_))));
    }
}
