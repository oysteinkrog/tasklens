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
//! Battery charge and power (crw.107) come with each UPower publish, at least once per 60 s
//! resync, not once a second. The sink repeats the last valid sample of each such series once per
//! second, at the time of the next system batch, for up to `HOLD_NS` after it. So every second of
//! the 1 s tier has a value and the 1 h range draws a line, not one dot per publish.
//!
//! Series are stored by `(persistent_key, MetricId)` (ADR-017 "Series identity") and mapped back
//! to the current catalog's `DeviceIdx` on read, so a renumbered device continues its series.
//!
//! Memory groups (D-036, ADR-017 "Memory group series"): the reducer's `MEM_GROUP` series on
//! `memgroup:<key>` devices are stored like any device series. A group registers with its first
//! sample, which it gets when it first reaches the top 20, and so enters the block catalog under
//! its display name. Outside the top 20 it gets no samples, so its points are gaps.
//!
//! Read path: the worker answers each wanted range (ADR-018): the local window's
//! `Demand.device_history_range`, then the distinct ranges of the other client sessions. The sink
//! sends every session's range as one numbered `RangeList` (level-triggered, as the history
//! worker's session list): each list is the whole truth, so a list the full queue refused only
//! delays the change until the next one gets through. A session that shows no range is the key
//! `None`. Two sessions on one range share one read and one answer.
//! For each range the worker reads its tier once per tier point (1 s, 10 s or 60 s) and whenever
//! the range becomes wanted, and publishes at most 1,024 min/max-decimated points per series. A
//! series whose points did not change keeps its `Arc`, and a read that changed nothing is not
//! sent. All answers go to the reducer together as `DeviceHistories`, which also holds the key
//! `None` when a session shows no range: one empty `DeviceHistory` that frees the points. A new
//! worker generation has sent nothing, so its first read answers every key, `None` included: the
//! reducer may still hold the answers of the generation before it.
//! The answer for the key `None` needs no store read. While the store is not open, so before
//! each open and while it fails to open, and after a failed read, the worker sends that answer
//! alone, so a session that shows no range never keeps old points. The ranged sessions keep what
//! they show until the next good read. A store that closes keeps the newest range list.
//!
//! Timeline (UI-PRF-057): while `Demand.device_timeline` is true, the same component carries a
//! `DeviceTimeline`: the last 24 hours of the 1 min tier for CPU total, memory in use, the
//! busiest disk's active time and the network total, with the events and boots of those hours.
//! It is read again every 60 s and when an event was queued, and dropped when the timeline is
//! no longer shown.
//!
//! Events (ADR-017 "Event detector", UI-PRF-057): the sink also sends one `Reading` of the
//! published snapshot per second and each Logind `PrepareForSleep`. The worker runs the
//! `EventDetector` rules on each reading, the clock check on each tick, and queues every ended
//! event in the store. Closing the store queues the running events first. OOM kills and crashes
//! come from `JournalEvents`, fetched once after each open and then every 60 s while
//! `Demand.device_timeline` is true, in a blocking job of their own so a slow `journalctl` never
//! holds up the tick. Each open drops journal entries at or before the newest stored one of this
//! boot.
//!
//! Disk work runs on the blocking pool, under a mutex that each job locks.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use asupersync::channel::mpsc;
use asupersync::runtime::{JoinError, TaskHandle};
use asupersync::time;
use asupersync::{Cx, Outcome};
use rustc_hash::FxHashMap;
use rustix::time::{ClockId, Timespec, clock_gettime};
use tl_collect::counters::Clock;
use tl_history::metrics::{
    self, EventKind, HistEvent, MetricConfig, MetricHistory, MetricSlot, MetricSpan, Tier,
};
use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
use tl_model::devices::{DeviceCatalog, NetKind};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::journal::JournalAccess;
use tl_model::series::Sample;
use tl_model::snapshot::{
    DeviceHistory, DeviceTimeline, HistPoint, Snapshot, TimelineEvent, TimelineEventKind,
};
use tl_model::time::{BootId, TimeNs, now_boot_ns};

use super::event_detector::{EventDetector, JournalEnv, JournalEvents, JournalFetch, Reading};
use crate::client_session::ClientSessionId;
use crate::source::WorkerIo;
use crate::update::{DeviceHistories, EngineMsg, SourceUpdate, WorkerError};

/// Capacity of the channel each open creates.
pub const METRIC_CHANNEL_CAPACITY: usize = 512;
/// How long one `recv` waits before the loop re-reads demand and settings.
const RECV_WAIT: Duration = Duration::from_millis(500);
/// At most this many messages go into one blocking job.
const BATCH_MAX: usize = 64;
/// Series that arrive less than once a second; the sink repeats their last sample (crw.107).
const HELD: [MetricId; 2] = [MetricId::BATTERY_PCT, MetricId::BATTERY_POWER];
/// How long the sink repeats a held sample: the 60 s UPower resync plus its 0.5 s wait, with
/// room to spare.
const HOLD_NS: u64 = 90 * NS_PER_SEC;
/// The store is ticked this often: it writes due minutes, ages the ring and drops idle spans.
const TICK_NS: u64 = 1_000_000_000;
/// A store that failed to open is tried again after this long.
const RETRY_NS: u64 = 60_000_000_000;
/// `MetricHistory::open` stops loading the 1 s ring after this long (ADR-012 start budget).
const LOAD_BUDGET: Duration = Duration::from_secs(2);
const NS_PER_SEC: u64 = 1_000_000_000;
/// The timeline is read again this often while shown (ADR-017).
const TIMELINE_EVERY_NS: u64 = 60 * NS_PER_SEC;
/// The span the timeline covers.
const TIMELINE_SPAN_NS: i64 = 86_400 * NS_PER_SEC as i64;

/// One event for the `MetricHistory` worker.
pub enum MetricMsg {
    /// One system sample batch. Each sample carries its own `t_ns`.
    System(Box<[(SeriesId, Sample)]>),
    /// `DeviceCatalog` holds its entries in an `Arc`, so the clone is cheap.
    Catalog(DeviceCatalog),
    /// The event detector's values of one published snapshot, at most one per second.
    Reading(Box<Reading>),
    /// Logind `PrepareForSleep`.
    Sleep { entering: bool, t_ns: TimeNs },
    /// The device history range of every client session (ADR-018).
    Ranges(RangeList),
}

/// Each client session and its `Demand.device_history_range`, `None` for a session that shows no
/// range, in connect order with the local session first. The sink numbers the lists from 1; list
/// 0 is the empty one each worker starts with. The worker keeps the newest list it got and drops
/// an older one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RangeList {
    pub seq: u64,
    pub ranges: Arc<[(ClientSessionId, Option<DeviceHistoryRequest>)]>,
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

/// The last valid sample of one `HELD` series and the boottime second it was last sent for.
struct Held {
    id: SeriesId,
    sample: Sample,
    sent_sec: u64,
}

/// The reducer side: a thin forwarder to the worker's channel.
pub struct MetricSink {
    slot: MetricHistorySlot,
    /// The newest catalog the reducer published.
    catalog: Option<DeviceCatalog>,
    /// The sender generation that received `catalog`.
    catalog_sent_to: Option<u64>,
    /// The CLOCK_BOOTTIME second of the last `Reading` sent.
    reading_sec: Option<u64>,
    /// The `HELD` series the sink repeats.
    held: Vec<Held>,
    /// The client sessions' ranges and the sender generation that has them.
    ranges: RangeList,
    ranges_sent_to: Option<u64>,
}

impl MetricSink {
    pub fn new(slot: MetricHistorySlot) -> Self {
        MetricSink {
            slot,
            catalog: None,
            catalog_sent_to: None,
            reading_sec: None,
            held: Vec::new(),
            ranges: RangeList::default(),
            ranges_sent_to: None,
        }
    }

    /// Each client session's range, in connect order with the local session first, called after
    /// each reducer turn. A changed list gets the next number. The list goes to the open store's
    /// worker now and to each new one; a send the full queue refused is tried again at the next
    /// call or system sample.
    pub fn set_ranges(&mut self, ranges: &[(ClientSessionId, Option<DeviceHistoryRequest>)]) {
        if *self.ranges.ranges != *ranges {
            self.ranges = RangeList {
                seq: self.ranges.seq + 1,
                ranges: Arc::from(ranges),
            };
            self.ranges_sent_to = None;
        }
        let guard = self.slot.load();
        if let Some(tx) = guard.as_deref() {
            self.sync_ranges(tx);
        }
    }

    /// Sends the list to `tx` when it does not have it. List 0 is the one a new worker starts with.
    fn sync_ranges(&mut self, tx: &MetricTx) {
        if self.ranges_sent_to == Some(tx.generation) || self.ranges.seq == 0 {
            return;
        }
        if tx
            .tx
            .try_send(MetricMsg::Ranges(self.ranges.clone()))
            .is_ok()
        {
            self.ranges_sent_to = Some(tx.generation);
        }
    }

    /// Sends the detector's `Reading` of `snap` when a store is open and no reading was sent in
    /// the same second.
    pub fn on_snapshot(&mut self, snap: &Snapshot) {
        let guard = self.slot.load();
        let Some(tx) = guard.as_deref() else {
            return;
        };
        let sec = snap.published_ns / NS_PER_SEC;
        if self.reading_sec == Some(sec) {
            return;
        }
        let reading = Box::new(Reading::from_snapshot(snap));
        if tx.tx.try_send(MetricMsg::Reading(reading)).is_ok() {
            self.reading_sec = Some(sec);
        }
    }

    /// Forwards a Logind `PrepareForSleep`. A dropped one is left to the worker's clock check.
    pub fn on_sleep(&mut self, entering: bool, t_ns: TimeNs) {
        let guard = self.slot.load();
        if let Some(tx) = guard.as_deref() {
            let _ = tx.tx.try_send(MetricMsg::Sleep { entering, t_ns });
        }
    }

    /// Sends the batch when a store is open, with a copy at `t_ns` of each held sample not yet
    /// sent for this second. A store that has not received the catalog gets it first; without a
    /// catalog the samples cannot be keyed and are dropped.
    pub fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]) {
        let guard = self.slot.load();
        let Some(tx) = guard.as_deref() else {
            return;
        };
        if !self.sync_catalog(tx) {
            return;
        }
        self.sync_ranges(tx);
        let held = self.hold(t_ns, series);
        let batch: Box<[(SeriesId, Sample)]> = if held.is_empty() {
            series.into()
        } else {
            series.iter().copied().chain(held).collect()
        };
        let _ = tx.tx.try_send(MetricMsg::System(batch));
    }

    /// Keeps the newest valid sample of each `HELD` series in `series`; a sample that is not
    /// valid ends the hold. A batch without `HELD` series gets the held samples younger than
    /// `HOLD_NS` that were not sent for the second of `t_ns` yet.
    fn hold(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]) -> Vec<(SeriesId, Sample)> {
        let mut fresh = false;
        for &(id, sample) in series.iter().filter(|(id, _)| HELD.contains(&id.metric)) {
            fresh = true;
            self.held.retain(|h| h.id != id);
            if sample.is_valid() {
                let sent_sec = sample.t_ns / NS_PER_SEC;
                self.held.push(Held {
                    id,
                    sample,
                    sent_sec,
                });
            }
        }
        if fresh {
            return Vec::new();
        }
        self.held
            .retain(|h| t_ns.saturating_sub(h.sample.t_ns) <= HOLD_NS);
        let sec = t_ns / NS_PER_SEC;
        let mut out = Vec::new();
        for h in self.held.iter_mut().filter(|h| sec > h.sent_sec) {
            h.sent_sec = sec;
            out.push((h.id, Sample::valid(t_ns, h.sample.value)));
        }
        out
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
/// finest tier that still covers the picked time, the range's centre (UI-PRF-057 "Paused
/// view"), but never one finer than the range's own: a 24 h range centred 12 hours ago ends
/// now, and the 1 s tier would hold only its last hour.
pub fn plan(req: DeviceHistoryRequest, now_realtime_ns: i64) -> (Tier, i64, i64) {
    let span = req.range.span_ns() as i64;
    let (tier, to) = match req.end_realtime_ns {
        None => (range_tier(req.range), now_realtime_ns),
        Some(end) => {
            let to = end.min(now_realtime_ns);
            let centre = to.saturating_sub(span / 2);
            let tier = finest_tier_at(now_realtime_ns - centre).max(range_tier(req.range));
            (tier, to)
        }
    };
    let from = to.saturating_sub(span);
    (tier, from, to)
}

/// One published series: its id in the current catalog and its points.
type HistSeries = (SeriesId, Arc<[HistPoint]>);

/// What the last read of one range held, to reuse its `Arc`s and skip unchanged reads.
struct Published {
    request: DeviceHistoryRequest,
    tier: Tier,
    step_ns: u64,
    since: Option<i64>,
    series: Arc<[HistSeries]>,
    boots: Arc<[i64]>,
    /// Boottime of the read.
    read_at: TimeNs,
}

/// What the last timeline held.
struct PublishedTimeline {
    data: Arc<DeviceTimeline>,
    /// Boottime of the read.
    read_at: TimeNs,
    /// `State::events_queued` at the read.
    events_seen: u64,
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
    /// The last read of each wanted range.
    published: Vec<Published>,
    /// The answers sent last, one per wanted key in `wanted` order. It starts empty: the
    /// reducer may hold another generation's answers, so the first read answers every key.
    sent: Vec<(Option<DeviceHistoryRequest>, Arc<DeviceHistory>)>,
    /// The client sessions' ranges (`MetricMsg::Ranges`).
    ranges: RangeList,
    /// Store reads for ranges, for tests.
    range_reads: u64,
    detector: EventDetector,
    /// Counts installs, so a journal fetch started for one store does not queue into the next.
    opens: u64,
    /// What the last journal fetch could read, for the timeline's tooltip.
    journal_access: Option<JournalAccess>,
    timeline: Option<PublishedTimeline>,
    /// Events queued into the store so far, so an ended event reads the timeline again.
    events_queued: u64,
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
            published: Vec::new(),
            sent: Vec::new(),
            ranges: RangeList::default(),
            range_reads: 0,
            detector: EventDetector::new(),
            opens: 0,
            journal_access: None,
            timeline: None,
            events_queued: 0,
        }
    }

    /// Takes an opened store. `realtime_offset_ns` must be the one in its `MetricConfig`.
    pub fn install(&mut self, store: MetricHistory, realtime_offset_ns: i64) {
        self.store = Some(store);
        self.realtime_offset_ns = realtime_offset_ns;
        self.slots.clear();
        self.detector = EventDetector::new();
        self.opens += 1;
    }

    pub fn realtime_offset_ns(&self) -> i64 {
        self.realtime_offset_ns
    }

    /// What the last journal fetch could read; `None` before the first one.
    pub fn journal_access(&self) -> Option<JournalAccess> {
        self.journal_access
    }

    /// How many stores were installed. A journal fetch hands it back to `store_journal`.
    pub fn opens(&self) -> u64 {
        self.opens
    }

    /// Queues the events of a journal fetch started while install number `opens` was current.
    /// A fetch from an earlier store is dropped: the new store's own first fetch reads them.
    pub fn store_journal(&mut self, opens: u64, fetch: JournalFetch) {
        if opens != self.opens {
            return;
        }
        self.journal_access = Some(fetch.access);
        self.queue_events(fetch.events);
    }

    pub fn is_open(&self) -> bool {
        self.store.is_some()
    }

    /// The open store, for tests.
    pub fn store_mut(&mut self) -> Option<&mut MetricHistory> {
        self.store.as_mut()
    }

    /// Queues the running events with their duration so far, ends every point and writes every
    /// minute not yet written, then drops the store.
    pub fn close(&mut self, now_t_ns: TimeNs) -> Result<(), tl_history::HistoryError> {
        self.slots.clear();
        let running = self.detector.finish(now_t_ns);
        self.queue_events(running);
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
            MetricMsg::Reading(reading) => {
                let ended = self.detector.observe(&reading);
                self.queue_events(ended);
            }
            MetricMsg::Sleep { entering, t_ns } => {
                let span = self.detector.sleep(entering, t_ns);
                self.queue_events(span);
            }
            MetricMsg::Ranges(list) => {
                if list.seq > self.ranges.seq {
                    self.ranges = list;
                }
            }
        }
    }

    /// Applies the range lists among `msgs` and drops the rest, for a store that closes. The sink
    /// does not send a list again to the same sender generation, so the next open needs it.
    pub fn keep_ranges(&mut self, msgs: impl IntoIterator<Item = MetricMsg>) {
        for msg in msgs {
            if let MetricMsg::Ranges(_) = msg {
                self.apply(msg);
            }
        }
    }

    /// Applies the range lists among `msgs` and leaves `msgs` as they are, so the empty answer
    /// can go out before the batch is folded in. Applying a list again later does nothing.
    pub fn apply_ranges(&mut self, msgs: &[MetricMsg]) {
        for msg in msgs {
            if let MetricMsg::Ranges(list) = msg {
                self.apply(MetricMsg::Ranges(list.clone()));
            }
        }
    }

    /// The keys to answer: `local`, the local window's range from the demand watch, which is
    /// never older than the list's, then each other session's range that is not in the list yet.
    /// `None` is the empty answer for a session that shows no range.
    pub fn wanted(&self, local: Option<DeviceHistoryRequest>) -> Vec<Option<DeviceHistoryRequest>> {
        let mut wanted = vec![local];
        let others = self.ranges.ranges.iter();
        for &(_, r) in others.filter(|(id, _)| *id != ClientSessionId::LOCAL) {
            if !wanted.contains(&r) {
                wanted.push(r);
            }
        }
        wanted
    }

    /// Store reads for ranges so far.
    pub fn range_reads(&self) -> u64 {
        self.range_reads
    }

    /// The ADR-008 clock check: queues a sleep span the Logind signal did not report.
    pub fn check_clock(&mut self, clock: Clock) {
        let span = self.detector.check_clock(clock);
        self.queue_events(span);
    }

    /// Queues events for the open store's next 1 min block. Without a store they are dropped.
    pub fn queue_events(&mut self, events: impl IntoIterator<Item = HistEvent>) {
        if let Some(store) = self.store.as_mut() {
            for e in events {
                store.push_event(e);
                self.events_queued += 1;
            }
        }
    }

    /// The newest stored OOM kill and crash of this boot, as `(kind, CLOCK_REALTIME ns)`, for
    /// `JournalEvents::skip_until`. Reads the 1 min tier from boot to now.
    pub fn stored_journal_events(
        &mut self,
        now_t_ns: TimeNs,
    ) -> Result<Vec<(EventKind, i64)>, tl_history::HistoryError> {
        let offset = self.realtime_offset_ns;
        let Some(store) = self.store.as_mut() else {
            return Ok(Vec::new());
        };
        let now_rt = (now_t_ns as i64).saturating_add(offset);
        let span = store.read(Tier::M1, offset, now_rt, now_t_ns)?;
        let mut newest: Vec<(EventKind, i64)> = Vec::new();
        for e in &span.events {
            let kind = e.event.kind;
            if !matches!(kind, EventKind::OomKill | EventKind::Crash)
                || e.start_realtime_ns < offset
            {
                continue;
            }
            match newest.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, t)) => *t = (*t).max(e.start_realtime_ns),
                None => newest.push((kind, e.start_realtime_ns)),
            }
        }
        Ok(newest)
    }

    /// Writes due minutes and applies retention (`MetricHistory::tick`).
    pub fn tick(&mut self, now_t_ns: TimeNs) -> Result<(), tl_history::HistoryError> {
        match self.store.as_mut() {
            Some(store) => store.tick(now_t_ns),
            None => Ok(()),
        }
    }

    /// [`State::answer_with`] without the timeline.
    pub fn answer(
        &mut self,
        request: Option<DeviceHistoryRequest>,
        now_t_ns: TimeNs,
    ) -> Result<Option<DeviceHistory>, tl_history::HistoryError> {
        self.answer_with(request, false, now_t_ns)
    }

    /// [`State::answer_all`] for one range: its `DeviceHistory`, or `None` when nothing needs
    /// sending.
    pub fn answer_with(
        &mut self,
        request: Option<DeviceHistoryRequest>,
        timeline: bool,
        now_t_ns: TimeNs,
    ) -> Result<Option<DeviceHistory>, tl_history::HistoryError> {
        let all = self.answer_all(&[request], timeline, now_t_ns)?;
        Ok(all.map(|all| DeviceHistory::clone(&all[0].1)))
    }

    /// The answer for each key of `wanted` and, with `timeline`, the 24-hour timeline at
    /// `now_t_ns`, or `None` when nothing needs sending: the keys, the data and the timeline are
    /// unchanged, or the last reads are too recent. Equal keys are answered once. A key that is
    /// newly wanted gets an answer even when it is empty, so a cleared range and timeline send
    /// one empty `DeviceHistory`. After a failed read every key is answered again next time,
    /// except the key `None` while its sent answer still holds the current timeline.
    pub fn answer_all(
        &mut self,
        wanted: &[Option<DeviceHistoryRequest>],
        timeline: bool,
        now_t_ns: TimeNs,
    ) -> Result<Option<DeviceHistories>, tl_history::HistoryError> {
        let answer = self.answer_keys(wanted, timeline, now_t_ns);
        if answer.is_err() {
            let held = self.no_range_held();
            self.sent.retain(|(key, _)| held && key.is_none());
        }
        answer
    }

    /// The answer for the key `None` alone, for when the store cannot answer: before it opens,
    /// when it fails to open and after a failed read (ADR-018). It needs no store read, so a
    /// session that shows no range stops showing old points even while the store fails. The
    /// range list does not reach a closed store, so the key `None` is answered whether or not a
    /// session shows no range. The ranged keys keep what they show until the next good read.
    /// `None` when the reducer holds this answer already.
    pub fn answer_no_range(&mut self) -> Option<DeviceHistories> {
        if self.no_range_held() {
            return None;
        }
        let answer = Arc::new(self.current(None));
        self.sent = vec![(None, Arc::clone(&answer))];
        Some(Arc::from([(None, answer)]))
    }

    /// The answer for the key `None` with an open store, before the ranged keys are read
    /// (ADR-018): it needs no store read, so a slow or failing read never delays it. The other
    /// keys of `wanted` keep the answer sent last and a key not sent yet waits for its read, so
    /// each session that shows a range keeps what it shows. `None` when no key of `wanted` is
    /// `None` or the reducer holds this answer already.
    pub fn answer_no_range_first(
        &mut self,
        wanted: &[Option<DeviceHistoryRequest>],
    ) -> Option<DeviceHistories> {
        if !wanted.contains(&None) || self.no_range_held() {
            return None;
        }
        let empty = Arc::new(self.current(None));
        let mut next: Vec<(Option<DeviceHistoryRequest>, Arc<DeviceHistory>)> =
            Vec::with_capacity(wanted.len());
        for &key in wanted {
            if next.iter().any(|(k, _)| *k == key) {
                continue;
            }
            let answer = match key {
                None => Arc::clone(&empty),
                Some(_) => match self.sent.iter().find(|(k, _)| *k == key) {
                    Some((_, h)) => Arc::clone(h),
                    None => continue,
                },
            };
            next.push((key, answer));
        }
        self.sent = next.clone();
        Some(next.into())
    }

    /// True when the key `None` was sent last with the current timeline.
    fn no_range_held(&self) -> bool {
        let timeline = self.timeline.as_ref().map(|t| &t.data);
        self.sent.iter().any(|(key, h)| {
            key.is_none()
                && match (&h.timeline, timeline) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    _ => false,
                }
        })
    }

    fn answer_keys(
        &mut self,
        wanted: &[Option<DeviceHistoryRequest>],
        timeline: bool,
        now_t_ns: TimeNs,
    ) -> Result<Option<DeviceHistories>, tl_history::HistoryError> {
        let mut keys: Vec<Option<DeviceHistoryRequest>> = Vec::with_capacity(wanted.len());
        for &key in wanted {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        self.published.retain(|p| keys.contains(&Some(p.request)));
        // The timeline is shared: when it changes, every key is answered again.
        let timeline_changed = self.answer_timeline(timeline, now_t_ns)?;
        let mut changed = Vec::new();
        for request in keys.iter().flatten() {
            if self.answer_range(*request, now_t_ns)? {
                changed.push(*request);
            }
        }
        let same_keys = self.sent.len() == keys.len()
            && self.sent.iter().zip(&keys).all(|((k, _), key)| k == key);
        if !timeline_changed && changed.is_empty() && same_keys {
            return Ok(None);
        }
        let mut next = Vec::with_capacity(keys.len());
        for key in keys {
            let old = self
                .sent
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, h)| Arc::clone(h));
            let fresh = timeline_changed || key.is_some_and(|r| changed.contains(&r));
            let answer = match old {
                Some(old) if !fresh => old,
                _ => Arc::new(self.current(key)),
            };
            next.push((key, answer));
        }
        self.sent = next.clone();
        Ok(Some(next.into()))
    }

    /// The store closed: each key sent last gets the empty answer. `None` when nothing was
    /// published, so the reducer already has empty answers.
    pub fn answer_closed(&mut self) -> Option<DeviceHistories> {
        let had = !self.published.is_empty() || self.timeline.is_some();
        self.published.clear();
        self.timeline = None;
        if !had {
            return None;
        }
        let empty = Arc::new(self.empty());
        self.sent = self
            .sent
            .iter()
            .map(|(key, _)| (*key, Arc::clone(&empty)))
            .collect();
        Some(self.sent.clone().into())
    }

    /// Reads `request` when due. True when its published part changed.
    fn answer_range(
        &mut self,
        request: DeviceHistoryRequest,
        now_t_ns: TimeNs,
    ) -> Result<bool, tl_history::HistoryError> {
        let Some(store) = self.store.as_mut() else {
            return Ok(false);
        };
        let now_rt = (now_t_ns as i64).saturating_add(self.realtime_offset_ns);
        let (tier, from, to) = plan(request, now_rt);
        let at = self.published.iter().position(|p| p.request == request);
        if let Some(p) = at.map(|i| &self.published[i])
            && p.tier == tier
            && now_t_ns < p.read_at.saturating_add(tier.point_ns())
        {
            return Ok(false);
        }
        self.range_reads += 1;
        let span = store.read(tier, from, to, now_t_ns)?;
        let since = store.since_realtime_ns(tier);
        let mut boots = store.boots_realtime_ns();
        boots.retain(|&t| (from..=to).contains(&t));
        let prev = at.map(|i| &self.published[i]);
        let (series, reused) = self.map_series(span, prev.map(|p| &p.series));
        let step_ns = series_step(tier, from, to);
        let unchanged = prev.is_some_and(|p| {
            p.tier == tier
                && p.step_ns == step_ns
                && p.since == since
                && *p.boots == *boots
                && reused
                && p.series.len() == series.len()
        });
        if unchanged && let Some(i) = at {
            self.published[i].read_at = now_t_ns;
            return Ok(false);
        }
        let next = Published {
            request,
            tier,
            step_ns,
            since,
            series: series.into(),
            boots: boots.into(),
            read_at: now_t_ns,
        };
        match at {
            Some(i) => self.published[i] = next,
            None => self.published.push(next),
        }
        Ok(true)
    }

    /// Reads the timeline every 60 s and after a queued event while `want`; drops it otherwise.
    /// True when the published timeline changed.
    fn answer_timeline(
        &mut self,
        want: bool,
        now_t_ns: TimeNs,
    ) -> Result<bool, tl_history::HistoryError> {
        if !want {
            return Ok(self.timeline.take().is_some());
        }
        let offset = self.realtime_offset_ns;
        let (events_queued, journal) = (self.events_queued, self.journal_access);
        let Some(store) = self.store.as_mut() else {
            return Ok(false);
        };
        if let Some(t) = &self.timeline
            && t.events_seen == events_queued
            && now_t_ns < t.read_at.saturating_add(TIMELINE_EVERY_NS)
        {
            return Ok(false);
        }
        let to = (now_t_ns as i64).saturating_add(offset);
        let from = to.saturating_sub(TIMELINE_SPAN_NS);
        let span = store.read(Tier::M1, from, to, now_t_ns)?;
        let mut boots = store.boots_realtime_ns();
        boots.retain(|&t| (from..=to).contains(&t));
        let data = timeline_of(&span, to, boots, journal);
        self.timeline = Some(PublishedTimeline {
            data: Arc::new(data),
            read_at: now_t_ns,
            events_seen: events_queued,
        });
        Ok(true)
    }

    /// The answer for `key` from its published read and the timeline.
    fn current(&self, key: Option<DeviceHistoryRequest>) -> DeviceHistory {
        let mut h = self.empty();
        if let Some(p) = key.and_then(|r| self.published.iter().find(|p| p.request == r)) {
            h.request = Some(p.request);
            h.since_realtime_ns = p.since;
            h.step_ns = p.step_ns;
            h.series = Arc::clone(&p.series);
            h.boots = Arc::clone(&p.boots);
        }
        h.timeline = self.timeline.as_ref().map(|t| Arc::clone(&t.data));
        h
    }

    /// The span's series under their current `SeriesId`, sorted, with the `Arc` of `prev`, the
    /// range's last read, where the points are equal. The bool is true when every series was
    /// reused.
    fn map_series(
        &self,
        span: MetricSpan,
        prev: Option<&Arc<[HistSeries]>>,
    ) -> (Vec<HistSeries>, bool) {
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

/// The timeline of a 1 min tier `span` that ends at `to`: CPU total, memory in use, the
/// aggregate disk's active time (the busiest disk) and the aggregate network's download plus
/// upload, keyed as the reducer stores them. `net_kinds` holds the same sum for each
/// `aggregate:net:<kind>` total that is stored.
fn timeline_of(
    span: &MetricSpan,
    to: i64,
    boots: Vec<i64>,
    journal: Option<JournalAccess>,
) -> DeviceTimeline {
    let find = |key: &str, metric: MetricId| {
        span.series
            .iter()
            .find(|(k, _)| &*k.device_key == key && k.metric == metric)
            .map(|(_, p)| p)
    };
    let band = |key: &str, metric: MetricId| -> Arc<[HistPoint]> {
        find(key, metric).map_or_else(
            || Arc::from([]),
            |points| {
                points
                    .iter()
                    .map(|p| HistPoint {
                        t_realtime_ns: p.t_realtime_ns,
                        mean: p.mean,
                        min: p.min,
                        max: p.max,
                    })
                    .collect()
            },
        )
    };
    // Download plus upload of one network total; `None` when neither direction is stored.
    let net_of = |key: &str| -> Option<Arc<[HistPoint]>> {
        let (rx, tx) = (find(key, MetricId::NET_RX), find(key, MetricId::NET_TX));
        if rx.is_none() && tx.is_none() {
            return None;
        }
        let len = rx.map_or(0, |p| p.len()).max(tx.map_or(0, |p| p.len()));
        Some(
            (0..len)
                .map(|i| {
                    let (a, b) = (rx.and_then(|p| p.get(i)), tx.and_then(|p| p.get(i)));
                    let t = a.or(b).map_or(0, |p| p.t_realtime_ns);
                    let valid: f32 = [a, b]
                        .into_iter()
                        .flatten()
                        .filter(|p| !p.mean.is_nan())
                        .map(|p| p.mean)
                        .sum();
                    let gap = [a, b].into_iter().flatten().all(|p| p.mean.is_nan());
                    let mean = if gap { f32::NAN } else { valid };
                    HistPoint {
                        t_realtime_ns: t,
                        mean,
                        min: mean,
                        max: mean,
                    }
                })
                .collect(),
        )
    };
    let net = net_of("aggregate:net").unwrap_or_else(|| Arc::from([]));
    // Per-kind totals (tasklens-q4x76.22), so a window can take away the kinds it hides.
    let net_kinds: Arc<[(NetKind, Arc<[HistPoint]>)]> = NetKind::ALL
        .into_iter()
        .filter_map(|kind| {
            net_of(&format!("aggregate:net:{}", kind.key())).map(|points| (kind, points))
        })
        .collect();
    let events: Arc<[TimelineEvent]> = span
        .events
        .iter()
        .map(|e| TimelineEvent {
            kind: event_kind(e.event.kind),
            start_realtime_ns: e.start_realtime_ns,
            duration_ms: e.event.duration_ms,
            peak: e.event.peak,
            process: e.event.process.clone(),
        })
        .collect();
    DeviceTimeline {
        end_realtime_ns: to,
        step_ns: span.step_ns,
        bands: [
            band("cpu", MetricId::CPU_TOTAL),
            band("memory", MetricId::MEM_USED),
            band("aggregate:disk", MetricId::DISK_ACTIVE),
            net,
        ],
        net_kinds,
        events,
        boots: boots.into(),
        journal,
    }
}

fn event_kind(kind: EventKind) -> TimelineEventKind {
    match kind {
        EventKind::MemoryPressure => TimelineEventKind::MemoryPressure,
        EventKind::SwapStorm => TimelineEventKind::SwapStorm,
        EventKind::Heat => TimelineEventKind::Heat,
        EventKind::FullLoad => TimelineEventKind::FullLoad,
        EventKind::Suspend => TimelineEventKind::Suspend,
        EventKind::Boot => TimelineEventKind::Boot,
        EventKind::OomKill => TimelineEventKind::OomKill,
        EventKind::Crash => TimelineEventKind::Crash,
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
    // Replaced at each open, with the newest stored journal events of this boot skipped.
    let mut journal = Arc::new(Mutex::new(JournalEvents::new(JournalEnv::system())));
    let mut journal_job: Option<TaskHandle<()>> = None;

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
            let mut dropped = std::mem::take(&mut msgs);
            while let Ok(msg) = rx.try_recv() {
                dropped.push(msg);
            }
            let job_state = Arc::clone(&state);
            let job = spawn(&cx, move || {
                let mut st = lock(&job_state);
                st.keep_ranges(dropped);
                if let Err(e) = st.close(now_boot_ns()) {
                    tracing::debug!("device history close failed: {e}");
                }
                st.answer_closed()
            });
            let empty = match job {
                Ok(mut job) => match join_result(&cx, job.join(&cx).await) {
                    Ok(empty) => empty,
                    Err(outcome) => stop!(outcome),
                },
                Err(outcome) => stop!(outcome),
            };
            if let Some(empty) = empty
                && let Some(outcome) = send(&cx, &io, SourceUpdate::DeviceHistory(empty)).await
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
            // The empty answer needs no store: it goes out before the open and while it fails.
            let empty = lock(&state).answer_no_range();
            if let Some(empty) = empty
                && let Some(outcome) = send(&cx, &io, SourceUpdate::DeviceHistory(empty)).await
            {
                stop!(outcome);
            }
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
            let job_state = Arc::clone(&state);
            let job = spawn(&cx, move || {
                let (store, report) =
                    MetricHistory::open(config, now_boot_ns(), Instant::now() + LOAD_BUDGET)?;
                let mut st = lock(&job_state);
                st.install(store, offset);
                let seen = st.stored_journal_events(now_boot_ns()).unwrap_or_else(|e| {
                    tracing::debug!("device history event read failed: {e}");
                    Vec::new()
                });
                Ok::<_, tl_history::HistoryError>((report, seen))
            });
            let opened = match job {
                Ok(mut job) => match join_result(&cx, job.join(&cx).await) {
                    Ok(opened) => opened,
                    Err(outcome) => stop!(outcome),
                },
                Err(outcome) => stop!(outcome),
            };
            match opened {
                Ok((report, seen)) => {
                    if report.timed_out {
                        tracing::warn!("device history load timed out; the last hour starts empty");
                    }
                    let mut events = JournalEvents::new(JournalEnv::system());
                    for (kind, t) in seen {
                        events.skip_until(kind, t);
                    }
                    journal = Arc::new(Mutex::new(events));
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

        if let Some(job) = journal_job.as_mut() {
            match job.try_join() {
                Ok(None) => {}
                Ok(Some(())) => journal_job = None,
                Err(e) => match join_result(&cx, Err(e)) {
                    Ok(()) => journal_job = None,
                    Err(outcome) => stop!(outcome),
                },
            }
        }
        if journal_job.is_none() && lock(&journal).due(now_boot_ns(), demand.device_timeline) {
            let job_state = Arc::clone(&state);
            let job_journal = Arc::clone(&journal);
            let opens = lock(&state).opens();
            let job = cx.spawn_blocking(move |job_cx| {
                let _running = crate::bridge::job_running("metric_history.journal_fetch");
                let offset = lock(&job_state).realtime_offset_ns();
                let fetch = lock(&job_journal)
                    .fetch(now_boot_ns(), offset, || job_cx.is_cancel_requested());
                lock(&job_state).store_journal(opens, fetch);
            });
            match job {
                Ok(job) => journal_job = Some(job),
                Err(e) => stop!(Outcome::Err(WorkerError::Other(
                    format!("spawn: {e}").into()
                ))),
            }
        }

        let request = demand.device_history_range;
        // The empty answer needs no store read either: it goes out before the reads, so a slow
        // or failing read never delays it (ADR-018).
        let empty = {
            let mut st = lock(&state);
            st.apply_ranges(&msgs);
            let wanted = st.wanted(request);
            st.answer_no_range_first(&wanted)
        };
        if let Some(empty) = empty
            && let Some(outcome) = send(&cx, &io, SourceUpdate::DeviceHistory(empty)).await
        {
            stop!(outcome);
        }

        let now = cx.now().as_nanos();
        if msgs.is_empty() && now < next_tick {
            continue;
        }
        let tick = now >= next_tick;
        if tick {
            next_tick = now.saturating_add(TICK_NS);
        }
        let timeline = demand.device_timeline;
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
            if tick {
                st.check_clock(Clock::now());
            }
            let now_t_ns = now_boot_ns();
            let mut error = None;
            if tick && let Err(e) = st.tick(now_t_ns) {
                tracing::debug!("device history write failed: {e}");
                error = Some(());
            }
            let wanted = st.wanted(request);
            let answer = match st.answer_all(&wanted, timeline, now_t_ns) {
                Ok(answer) => answer,
                Err(e) => {
                    tracing::debug!("device history read failed: {e}");
                    st.answer_no_range()
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
            && let Some(outcome) = send(&cx, &io, SourceUpdate::DeviceHistory(history)).await
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
    cx.spawn_blocking(move |_| {
        let _running = crate::bridge::job_running("metric_history");
        f()
    })
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

    /// List `seq` of the sessions other than the local one, numbered from 1 in `ranges` order.
    fn others(seq: u64, ranges: &[Option<DeviceHistoryRequest>]) -> MetricMsg {
        let ranges = (1..)
            .map(ClientSessionId)
            .zip(ranges.iter().copied())
            .collect();
        MetricMsg::Ranges(RangeList { seq, ranges })
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
        // Paused: the finest tier that covers the range's centre, never finer than the range's.
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
        // 24 h centred 12 hours ago ends now; the 1 s tier would hold only its last hour.
        assert_eq!(
            plan(req(DeviceRange::H24, Some(now)), now),
            (Tier::S10, now - 24 * HOUR, now)
        );
        assert_eq!(
            plan(req(DeviceRange::H24, Some(now - 2 * HOUR)), now),
            (Tier::S10, now - 26 * HOUR, now - 2 * HOUR)
        );
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
        // One boot stored so far: no boot marker.
        assert!(h.boots.is_empty());
        // Closing writes the open seconds too.
        st.close(now).expect("close");

        // A reboot: boottime starts again and the disk is idx 7 now. The last hour comes back
        // from disk under the new number, with a gap for the time the machine was off.
        let offset2 = OFFSET + 1_020 * SEC as i64;
        let t1 = 5 * SEC;
        let mut st = open(dir.path(), 2, offset2, t1);
        // Catalog entries sit at their index, so idx 0 to 6 are the logical CPUs.
        let mut entries: Vec<DeviceEntry> = (0..7)
            .map(|i| {
                let identity = DeviceIdentity::CpuLogical { id: i };
                entry(i, identity, &format!("cpu:{i}"), true)
            })
            .collect();
        entries.push(entry(7, disk("sda"), "disk:WWN-A", true));
        st.set_catalog(catalog(1, entries));
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
        // The reboot lies inside the range, so it comes with its boot time.
        assert_eq!(*h.boots, [offset2]);
    }

    /// D-036, ADR-017 "Memory group series": a memory group's series is stored under
    /// `memgroup:<key>` with the group's display name in the block catalog, continues after an
    /// engine restart under a new `DeviceIdx`, and is a gap while the group is outside the top
    /// 20. The 24 h and 7 d reads return band means, not the per-second maximum.
    #[test]
    fn memory_group_series_survive_restart() {
        use tl_history::format::HEADER_LEN;

        let mem_group = MetricId::MEM_GROUP;
        let group = |idx: u16, key: &str, name: &str| DeviceEntry {
            display_name: Arc::from(name),
            ..entry(
                idx,
                DeviceIdentity::MemGroup {
                    key: Arc::from(key),
                },
                &format!("memgroup:{key}"),
                true,
            )
        };
        // 1 s values alternate between `low` and `low + 200`, so every 10 s, 1 min and read
        // bucket holds as many of each: the mean is `low + 100`, the maximum `low + 200`.
        let alternate = |i: u64, low: f32| {
            if i.is_multiple_of(2) {
                low
            } else {
                low + 200.0
            }
        };
        // Every bucket edge falls on a whole multiple of 1800 s (the 4 s, 90 s and 600 s read
        // steps of 1 h, 24 h and 7 d), so no read bucket mixes the two sessions.
        let offset = 1_699_999_200 * SEC as i64;
        let rt = |t: TimeNs| t as i64 + offset;
        let point = |h: &DeviceHistory, id: SeriesId, t: TimeNs| -> HistPoint {
            *h.get(id)
                .expect("series")
                .iter()
                .find(|p| p.t_realtime_ns == rt(t))
                .expect("bucket")
        };
        let valid = |h: &DeviceHistory, id: SeriesId, f: fn(&HistPoint) -> f32| -> Vec<f32> {
            h.get(id)
                .map(|p| p.iter().filter(|p| !p.is_gap()).map(f).collect())
                .unwrap_or_default()
        };

        // Session 1: rustc for 120 s; node in the top 20 for the first 60 s only.
        let dir = tempdir();
        let t0 = 1_800 * SEC;
        let mut st = open(dir.path(), 1, offset, t0);
        st.set_catalog(catalog(
            1,
            vec![
                entry(0, DeviceIdentity::Memory, "memory", true),
                group(1, "exe:rustc", "rustc"),
                group(2, "exe:node", "node"),
                group(3, "other", "Other apps"),
            ],
        ));
        for i in 0..120 {
            let t = t0 + i * SEC;
            let mut batch = vec![
                (sid(1, mem_group), sample(t, alternate(i, 100.0))),
                (sid(3, mem_group), sample(t, 50.0)),
            ];
            match i {
                0..60 => batch.push((sid(2, mem_group), sample(t, 40.0))),
                // The reducer's one gap sample when the group leaves the top 20.
                60 => batch.push((
                    sid(2, mem_group),
                    Sample {
                        t_ns: t,
                        value: 0.0,
                        flags: SampleFlags::empty(),
                    },
                )),
                _ => {}
            }
            st.push(&batch);
        }
        let now = t0 + 125 * SEC;
        let h = st
            .answer(live(DeviceRange::H24), now)
            .expect("read")
            .expect("sent");
        assert_eq!(h.step_ns, 90 * SEC);
        assert_eq!(valid(&h, sid(2, mem_group), |p| p.mean), [40.0]);
        assert!(point(&h, sid(2, mem_group), t0 + 90 * SEC).is_gap());
        assert!(!point(&h, sid(1, mem_group), t0 + 90 * SEC).is_gap());
        st.close(now).expect("close");

        // Session 2: the engine restarts 15 minutes later on the same boot. The new reducer
        // numbers its devices again, and node does not reach the top 20.
        let t1 = t0 + 900 * SEC;
        let mut st = open(dir.path(), 1, offset, t1);
        st.set_catalog(catalog(
            1,
            vec![
                entry(0, DeviceIdentity::Memory, "memory", true),
                group(1, "other", "Other apps"),
                group(2, "exe:rustc", "rustc"),
            ],
        ));
        // Second 120 stays open, so seconds 0 to 119 are readable.
        for i in 0..=120 {
            let t = t1 + i * SEC;
            st.push(&[
                (sid(1, mem_group), sample(t, 50.0)),
                (sid(2, mem_group), sample(t, alternate(i, 500.0))),
            ]);
        }
        let now = t1 + 125 * SEC;
        st.tick(now).expect("tick");

        // 1 h: the 1 s tier, in 4 s buckets of two low and two high values.
        let h = st
            .answer(live(DeviceRange::H1), now)
            .expect("read")
            .expect("sent");
        assert_eq!(h.step_ns, 4 * SEC);
        let rustc = sid(2, mem_group);
        let mut want = vec![200.0; 30];
        want.extend([600.0; 30]);
        assert_eq!(valid(&h, rustc, |p| p.mean), want);
        assert!(point(&h, rustc, t0 + 600 * SEC).is_gap());
        assert_eq!(valid(&h, sid(1, mem_group), |p| p.mean), [50.0; 60]);

        // 24 h: the 10 s tier in 90 s buckets, each session's from its own segment.
        let h = st
            .answer(live(DeviceRange::H24), now)
            .expect("read")
            .expect("sent");
        assert_eq!(h.step_ns, 90 * SEC);
        assert_eq!(valid(&h, rustc, |p| p.mean), [200.0, 200.0, 600.0, 600.0]);
        assert_eq!(valid(&h, rustc, |p| p.min), [100.0, 100.0, 500.0, 500.0]);
        assert_eq!(valid(&h, rustc, |p| p.max), [300.0, 300.0, 700.0, 700.0]);

        // 7 d: the 1 min tier in 600 s buckets.
        let h = st
            .answer(live(DeviceRange::D7), now)
            .expect("read")
            .expect("sent");
        assert_eq!(h.step_ns, 600 * SEC);
        assert_eq!(valid(&h, rustc, |p| p.mean), [200.0, 600.0]);
        assert_eq!(valid(&h, rustc, |p| p.max), [300.0, 700.0]);

        // The first block of each 1 min segment names the groups that had samples in its
        // session: node only in the first.
        let store = st.store_mut().expect("open");
        let catalogs: Vec<Vec<(String, String)>> = store
            .segments(Tier::M1)
            .iter()
            .map(|(path, _, _)| {
                let bytes = std::fs::read(path).expect("read segment");
                let (block, _) =
                    metrics::decode_metric_block(&bytes[HEADER_LEN..], 0).expect("first block");
                let mut names: Vec<(String, String)> = block
                    .catalog
                    .expect("catalog in the first block")
                    .iter()
                    .filter(|s| s.device_key.starts_with("memgroup:"))
                    .map(|s| (s.device_key.to_string(), s.device_name.to_string()))
                    .collect();
                names.sort();
                names
            })
            .collect();
        let pair = |k: &str, n: &str| (k.to_string(), n.to_string());
        assert_eq!(
            catalogs,
            [
                vec![
                    pair("memgroup:exe:node", "node"),
                    pair("memgroup:exe:rustc", "rustc"),
                    pair("memgroup:other", "Other apps"),
                ],
                vec![
                    pair("memgroup:exe:rustc", "rustc"),
                    pair("memgroup:other", "Other apps"),
                ],
            ]
        );
    }

    /// tasklens-o55.56: a 100 ms boost of one core's clock survives every tier, and so does
    /// the mean. The 1 s tier keeps each second's mean and maximum, so every bucket reads the
    /// mean of its samples with the boost as its maximum.
    #[test]
    fn core_clock_boost_survives_downsampling() {
        let clock = sid(0, MetricId::CPU_CORE_CLOCK);
        let offset = 1_699_999_200 * SEC as i64;
        let dir = tempdir();
        let t0 = 1_800 * SEC;
        let mut st = open(dir.path(), 1, offset, t0);
        st.set_catalog(catalog(
            1,
            vec![entry(
                0,
                DeviceIdentity::CpuLogical { id: 0 },
                "cpu:0",
                true,
            )],
        ));
        // 10 Hz at 3000 MHz for 121 s, with one sample at 5000 MHz in second 30.
        let boost = t0 + 30 * SEC + SEC / 2;
        for i in 0..1_210 {
            let t = t0 + i * SEC / 10;
            let mhz = if t == boost { 5000.0 } else { 3000.0 };
            st.push(&[(clock, sample(t, mhz))]);
        }
        let now = t0 + 125 * SEC;
        st.tick(now).expect("tick");
        // The mean of the 10 samples of each of `secs` seconds, one of them the boost.
        let mean = |secs: f32| (5000.0 + (10.0 * secs - 1.0) * 3000.0) / (10.0 * secs);
        let read = |st: &mut State, range| -> Vec<HistPoint> {
            let h = st.answer(live(range), now).expect("read").expect("sent");
            h.get(clock)
                .expect("series")
                .iter()
                .filter(|p| !p.is_gap())
                .copied()
                .collect()
        };

        // 1 h: one 4 s bucket holds the boost as its maximum, beside the mean of its 40
        // samples, 3050 MHz. Second 120 is still open, so 120 seconds are readable.
        let h1 = read(&mut st, DeviceRange::H1);
        assert_eq!(h1.len(), 30);
        let peaks: Vec<&HistPoint> = h1.iter().filter(|p| p.max == 5000.0).collect();
        assert_eq!(peaks.len(), 1);
        assert_eq!((peaks[0].mean, mean(4.0)), (3050.0, 3050.0));
        assert!(h1.iter().all(|p| p.min == 3000.0));
        assert!(
            h1.iter()
                .all(|p| p.max == 5000.0 || (p.mean, p.max) == (3000.0, 3000.0))
        );

        // 24 h: the 10 s tier in 90 s buckets, the second one partial.
        let h24 = read(&mut st, DeviceRange::H24);
        let max: Vec<f32> = h24.iter().map(|p| p.max).collect();
        assert_eq!(max, [5000.0, 3000.0]);
        assert!((h24[0].mean - mean(90.0)).abs() < 0.01, "{:?}", h24[0]);
        assert_eq!(h24[1].mean, 3000.0);

        // 7 d: the 1 min tier in one 600 s bucket.
        let d7 = read(&mut st, DeviceRange::D7);
        assert_eq!(d7.len(), 1);
        assert_eq!(d7[0].max, 5000.0);
        assert!((d7[0].mean - mean(120.0)).abs() < 0.01, "{:?}", d7[0]);
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

    /// ADR-018: sessions on 24 h and 7 d each get the answer for their own range, and a timeline
    /// change answers every range again.
    #[test]
    fn each_range_gets_its_own_answer() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        st.set_catalog(catalog(
            1,
            vec![entry(0, DeviceIdentity::CpuPackage, "cpu", true)],
        ));
        for i in 0..5 {
            st.push(&[(sid(0, MetricId::CPU_TOTAL), sample(t0 + i * SEC, 1.0))]);
        }
        let (day, week) = (live(DeviceRange::H24), live(DeviceRange::D7));
        let now = t0 + 6 * SEC;
        let all = st
            .answer_all(&[day, week], false, now)
            .expect("read")
            .expect("sent");
        let keys: Vec<_> = all.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, [day, week]);
        assert_eq!(all[0].1.request, day);
        assert_eq!(all[1].1.request, week);
        assert_eq!(
            all[0].1.step_ns,
            90 * SEC,
            "24 h: the 10 s tier in 90 s buckets"
        );
        assert_eq!(
            all[1].1.step_ns,
            600 * SEC,
            "7 d: the 1 min tier in 10 min buckets"
        );
        assert_eq!(st.range_reads(), 2);

        // A session leaves the 7 d range: one answer, its `Arc` kept.
        let one = st
            .answer_all(&[day], false, now + SEC)
            .expect("read")
            .expect("sent");
        assert_eq!(one.len(), 1);
        assert!(Arc::ptr_eq(&one[0].1, &all[0].1));

        // The timeline turns on: the 24 h answer and the no-range answer both carry it.
        let with = st
            .answer_all(&[day, None], true, now + 2 * SEC)
            .expect("read")
            .expect("sent");
        assert_eq!(with.len(), 2);
        assert!(with.iter().all(|(_, h)| h.timeline.is_some()));
        assert_eq!(with[0].1.request, day);
        assert_eq!(with[1].1.request, None);
        assert!(with[1].1.series.is_empty());
    }

    /// ADR-018: two sessions on one range cause one read and share one answer; the other
    /// sessions' ranges come as `MetricMsg::Ranges` after the local one.
    #[test]
    fn two_sessions_on_one_range_read_once() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        st.set_catalog(catalog(
            1,
            vec![entry(0, DeviceIdentity::CpuPackage, "cpu", true)],
        ));
        let (day, hour) = (live(DeviceRange::H24), live(DeviceRange::H1));
        st.apply(others(1, &[day, hour, day]));
        assert_eq!(st.wanted(day), [day, hour]);
        assert_eq!(st.wanted(None), [None, day, hour]);
        // An older list is dropped; the local session's entry yields to the demand watch.
        st.apply(others(0, &[]));
        assert_eq!(st.wanted(None), [None, day, hour]);
        st.apply(MetricMsg::Ranges(RangeList {
            seq: 2,
            ranges: Arc::from([(ClientSessionId::LOCAL, hour), (ClientSessionId(1), day)]),
        }));
        assert_eq!(st.wanted(None), [None, day]);
        st.apply(others(3, &[day, hour, day]));

        let now = t0 + 2 * SEC;
        let all = st
            .answer_all(&[day, day], false, now)
            .expect("read")
            .expect("sent");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1.request, day);
        assert_eq!(st.range_reads(), 1);

        // The local window and one other session on 24 h, one more on 1 h: two reads.
        let wanted = st.wanted(day);
        let all = st
            .answer_all(&wanted, false, now + 10 * SEC)
            .expect("read")
            .expect("sent");
        assert_eq!(all.len(), 2);
        assert_eq!(st.range_reads(), 3);

        // The store closes: each key gets the empty answer once.
        let closed = st.answer_closed().expect("sent");
        assert_eq!(closed.len(), 2);
        assert!(
            closed
                .iter()
                .all(|(_, h)| h.request.is_none() && h.series.is_empty())
        );
        assert!(st.answer_closed().is_none());
    }

    /// ADR-018: while the local window is on 24 h, another session that clears its range gets
    /// the empty answer under the key `None`, so it stops showing its old points.
    #[test]
    fn a_session_without_a_range_gets_the_empty_answer() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        st.set_catalog(catalog(
            1,
            vec![entry(0, DeviceIdentity::CpuPackage, "cpu", true)],
        ));
        let (day, week) = (live(DeviceRange::H24), live(DeviceRange::D7));
        st.apply(others(1, &[week]));
        let now = t0 + 2 * SEC;
        let wanted = st.wanted(day);
        let all = st
            .answer_all(&wanted, false, now)
            .expect("read")
            .expect("sent");
        assert_eq!(all.len(), 2);

        st.apply(others(2, &[None]));
        assert_eq!(st.wanted(day), [day, None]);
        let wanted = st.wanted(day);
        let all = st
            .answer_all(&wanted, false, now + SEC)
            .expect("read")
            .expect("sent");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].1.request, day);
        assert_eq!(all[1].0, None);
        assert_eq!(all[1].1.request, None);
        assert!(all[1].1.series.is_empty());
    }

    /// UI-PRF-057, ADR-017: the timeline carries the four bands of the last 24 hours (the
    /// network band is download plus upload), the queued events and the journal access. It is
    /// read again after 60 s or a new event, and dropped once when the timeline is not shown.
    #[test]
    fn timeline_reads_bands_events_and_journal() {
        use tl_model::devices::AggregateKind;

        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        st.set_catalog(catalog(
            1,
            vec![
                entry(0, DeviceIdentity::CpuPackage, "cpu", true),
                entry(1, DeviceIdentity::Memory, "memory", true),
                entry(
                    2,
                    DeviceIdentity::Aggregate(AggregateKind::Disks),
                    "aggregate:disk",
                    true,
                ),
                entry(
                    3,
                    DeviceIdentity::Aggregate(AggregateKind::Nets),
                    "aggregate:net",
                    true,
                ),
                entry(
                    4,
                    DeviceIdentity::Aggregate(AggregateKind::NetsOf(NetKind::Virtual)),
                    "aggregate:net:virtual",
                    true,
                ),
            ],
        ));
        for i in 0..180 {
            let t = t0 + i * SEC;
            st.push(&[
                (sid(0, MetricId::CPU_TOTAL), sample(t, 40.0)),
                (sid(1, MetricId::MEM_USED), sample(t, 4096.0)),
                (sid(2, MetricId::DISK_ACTIVE), sample(t, 25.0)),
                (sid(3, MetricId::NET_RX), sample(t, 100.0)),
                (sid(3, MetricId::NET_TX), sample(t, 50.0)),
                (sid(4, MetricId::NET_RX), sample(t, 30.0)),
                (sid(4, MetricId::NET_TX), sample(t, 10.0)),
            ]);
        }
        st.queue_events([HistEvent {
            kind: EventKind::Heat,
            start_t_ns: t0 + 10 * SEC,
            duration_ms: 30_000,
            peak: 97.0,
            device_key: Some(Arc::from("cpu")),
            process: Some(Arc::from("blender")),
        }]);
        st.store_journal(
            st.opens(),
            JournalFetch {
                events: Vec::new(),
                access: JournalAccess::OwnOnly,
            },
        );
        let now = t0 + 185 * SEC;
        st.tick(now).expect("tick");
        let h = st
            .answer_with(None, true, now)
            .expect("read")
            .expect("sent");
        assert_eq!(h.request, None, "the timeline needs no range request");
        let tl = h.timeline.clone().expect("a timeline");
        assert_eq!(tl.end_realtime_ns, now as i64 + OFFSET);
        let valid = |band: usize| -> Vec<f32> {
            tl.bands[band]
                .iter()
                .filter(|p| !p.is_gap())
                .map(|p| p.mean)
                .collect()
        };
        for (band, want) in [(0, 40.0), (1, 4096.0), (2, 25.0), (3, 150.0)] {
            let means = valid(band);
            assert!(!means.is_empty(), "band {band} has points");
            assert!(means.iter().all(|&m| m == want), "band {band}: {means:?}");
        }
        // One per-kind band per stored per-kind total (tasklens-q4x76.22), on the band's times.
        assert_eq!(tl.net_kinds.len(), 1, "{:?}", tl.net_kinds);
        let (kind, virt) = &tl.net_kinds[0];
        assert_eq!(*kind, NetKind::Virtual);
        assert_eq!(virt.len(), tl.bands[3].len());
        let virt_means: Vec<f32> = virt
            .iter()
            .filter(|p| !p.is_gap())
            .map(|p| p.mean)
            .collect();
        assert!(!virt_means.is_empty());
        assert!(virt_means.iter().all(|&m| m == 40.0), "{virt_means:?}");
        assert_eq!(tl.events.len(), 1);
        let e = &tl.events[0];
        assert_eq!(e.kind, TimelineEventKind::Heat);
        assert_eq!(e.start_realtime_ns, (t0 + 10 * SEC) as i64 + OFFSET);
        assert_eq!(e.process.as_deref(), Some("blender"));
        assert_eq!(tl.journal, Some(JournalAccess::OwnOnly));

        // Within 60 s nothing is read again, unless an event ended.
        assert!(
            st.answer_with(None, true, now + SEC)
                .expect("read")
                .is_none()
        );
        st.queue_events([HistEvent {
            kind: EventKind::Suspend,
            start_t_ns: t0 + 100 * SEC,
            duration_ms: 1_000,
            peak: 0.0,
            device_key: None,
            process: None,
        }]);
        let h = st
            .answer_with(None, true, now + 2 * SEC)
            .expect("read")
            .expect("sent");
        assert_eq!(h.timeline.expect("a timeline").events.len(), 2);
        assert!(
            st.answer_with(None, true, now + 3 * SEC)
                .expect("read")
                .is_none()
        );
        let h = st.answer_with(None, true, now + 62 * SEC).expect("read");
        assert!(h.is_some(), "read again after 60 s");

        // The timeline is no longer shown: one empty component, then nothing.
        let h = st
            .answer_with(None, false, now + 63 * SEC)
            .expect("read")
            .expect("sent");
        assert!(h.timeline.is_none());
        assert!(
            st.answer_with(None, false, now + 64 * SEC)
                .expect("read")
                .is_none()
        );
    }

    fn psi(t_ns: TimeNs, pct: f32) -> MetricMsg {
        MetricMsg::Reading(Box::new(Reading {
            t_ns,
            psi_some_pct: Some(pct),
            ..Reading::default()
        }))
    }

    fn event(kind: EventKind, start_t_ns: TimeNs) -> HistEvent {
        HistEvent {
            kind,
            start_t_ns,
            duration_ms: 0,
            peak: 0.,
            device_key: None,
            process: None,
        }
    }

    /// ADR-017 "Event detector": rule events, the Logind sleep span, journal events and the
    /// event still running at close all reach the 1 min tier.
    #[test]
    fn detector_events_reach_the_store() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        // Memory pressure for 40 s, then 61 s under: the event ends.
        for s in 0..101 {
            st.apply(psi(t0 + s * SEC, if s < 40 { 20. } else { 1. }));
        }
        st.apply(MetricMsg::Sleep {
            entering: true,
            t_ns: t0 + 200 * SEC,
        });
        st.apply(MetricMsg::Sleep {
            entering: false,
            t_ns: t0 + 500 * SEC,
        });
        // A journal fetch for this store is kept; one from an earlier store is dropped.
        let fetch = |kind, t| JournalFetch {
            access: JournalAccess::All,
            events: vec![event(kind, t)],
        };
        st.store_journal(st.opens(), fetch(EventKind::OomKill, t0 + 550 * SEC));
        st.store_journal(st.opens() - 1, fetch(EventKind::Crash, t0 + 560 * SEC));
        assert_eq!(st.journal_access(), Some(JournalAccess::All));
        // Pressure again from 600 s, still running at close.
        for s in 600..650 {
            st.apply(psi(t0 + s * SEC, 30.));
        }
        let end = t0 + 650 * SEC;
        st.close(end).expect("close");

        let mut st = open(dir.path(), 1, OFFSET, end + SEC);
        let now = end + 2 * SEC;
        let now_rt = now as i64 + OFFSET;
        let span = st
            .store_mut()
            .expect("open")
            .read(Tier::M1, OFFSET, now_rt, now)
            .expect("read");
        let got: Vec<(EventKind, u64, u32)> = span
            .events
            .iter()
            .map(|e| (e.event.kind, e.event.start_t_ns, e.event.duration_ms))
            .collect();
        assert_eq!(
            got,
            [
                (EventKind::MemoryPressure, t0, 40_000),
                (EventKind::Suspend, t0 + 200 * SEC, 300_000),
                (EventKind::OomKill, t0 + 550 * SEC, 0),
                (EventKind::MemoryPressure, t0 + 600 * SEC, 50_000),
            ]
        );
        // The next open skips journal entries up to the stored OOM kill.
        assert_eq!(
            st.stored_journal_events(now).expect("read"),
            [(EventKind::OomKill, (t0 + 550 * SEC) as i64 + OFFSET)]
        );
    }

    #[test]
    fn sink_sends_one_reading_per_second_and_each_sleep() {
        let slot: MetricHistorySlot = Arc::new(ArcSwapOption::empty());
        let mut sink = MetricSink::new(Arc::clone(&slot));
        let mut snap = Snapshot::empty(BootId(1), 1);
        snap.published_ns = SEC;
        // No store open: nothing is queued.
        sink.on_snapshot(&snap);
        sink.on_sleep(true, SEC);

        let (tx, mut rx) = mpsc::channel::<MetricMsg>(8);
        slot.store(Some(Arc::new(MetricTx { generation: 1, tx })));
        for t in [
            SEC + SEC / 2,
            SEC + SEC / 10 * 9,
            2 * SEC,
            2 * SEC + 1,
            4 * SEC,
        ] {
            snap.published_ns = t;
            sink.on_snapshot(&snap);
        }
        sink.on_sleep(true, 5 * SEC);
        let got: Vec<(&str, TimeNs)> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| match m {
                MetricMsg::Reading(r) => ("reading", r.t_ns),
                MetricMsg::Sleep { t_ns, .. } => ("sleep", t_ns),
                _ => ("other", 0),
            })
            .collect();
        assert_eq!(
            got,
            [
                ("reading", SEC + SEC / 2),
                ("reading", 2 * SEC),
                ("reading", 4 * SEC),
                ("sleep", 5 * SEC),
            ]
        );
    }

    /// crw.107: a battery sample is sent again once per second with the next system batches,
    /// up to `HOLD_NS` after it. A sample that is not valid ends the hold of its series only.
    #[test]
    fn sink_repeats_battery_samples_each_second() {
        let slot: MetricHistorySlot = Arc::new(ArcSwapOption::empty());
        let mut sink = MetricSink::new(Arc::clone(&slot));
        sink.on_device_catalog(&catalog(1, Vec::new()));
        let (tx, mut rx) = mpsc::channel::<MetricMsg>(16);
        slot.store(Some(Arc::new(MetricTx { generation: 1, tx })));
        let pct = sid(1, MetricId::BATTERY_PCT);
        let power = sid(1, MetricId::BATTERY_POWER);
        let cpu = |t: TimeNs| [(sid(0, MetricId::CPU_TOTAL), sample(t, 1.0))];
        let gap = Sample {
            t_ns: 3 * SEC,
            value: 0.0,
            flags: SampleFlags::empty(),
        };

        sink.on_system_sample(SEC, &[(pct, sample(SEC, 80.0)), (power, sample(SEC, -9.5))]);
        for t in [SEC + SEC / 2, 2 * SEC, 2 * SEC + SEC / 2] {
            sink.on_system_sample(t, &cpu(t));
        }
        sink.on_system_sample(3 * SEC, &[(pct, gap)]);
        for t in [4 * SEC, SEC + HOLD_NS, 2 * SEC + HOLD_NS] {
            sink.on_system_sample(t, &cpu(t));
        }
        let got: Vec<Vec<(SeriesId, TimeNs, f32)>> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|m| match m {
                MetricMsg::System(b) => Some(
                    b.iter()
                        .filter(|(id, _)| id.device == DeviceIdx(1))
                        .map(|(id, s)| (*id, s.t_ns, s.value))
                        .collect(),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(
            got,
            [
                vec![(pct, SEC, 80.0), (power, SEC, -9.5)],
                // Second 1 already has them.
                vec![],
                vec![(pct, 2 * SEC, 80.0), (power, 2 * SEC, -9.5)],
                vec![],
                vec![(pct, 3 * SEC, 0.0)],
                vec![(power, 4 * SEC, -9.5)],
                vec![(power, SEC + HOLD_NS, -9.5)],
                // Past `HOLD_NS`.
                vec![],
            ]
        );
    }

    /// crw.107: the battery charge and power series round-trip through the store. UPower
    /// publishes once per 60 s and the sink holds the values between, so the 1 h and 24 h reads
    /// have a value in every bucket, and the 1 h read gets them back after a restart.
    #[test]
    fn battery_series_round_trip() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        let battery = DeviceIdentity::Battery {
            native_path: Arc::from("BAT0"),
        };
        let cat = catalog(
            1,
            vec![
                entry(0, battery.clone(), "battery:BAT0", true),
                entry(1, DeviceIdentity::CpuPackage, "cpu", true),
            ],
        );
        st.set_catalog(cat.clone());
        let slot: MetricHistorySlot = Arc::new(ArcSwapOption::empty());
        let mut sink = MetricSink::new(Arc::clone(&slot));
        let (tx, mut rx) = mpsc::channel::<MetricMsg>(512);
        slot.store(Some(Arc::new(MetricTx { generation: 1, tx })));
        sink.on_device_catalog(&cat);
        let pct = sid(0, MetricId::BATTERY_PCT);
        let power = sid(0, MetricId::BATTERY_POWER);
        for i in 0..120 {
            let t = t0 + i * SEC;
            if i % 60 == 0 {
                let charge = 80.0 - (i / 60) as f32;
                sink.on_system_sample(t, &[(pct, sample(t, charge)), (power, sample(t, -10.0))]);
            }
            let t = t + SEC / 10;
            sink.on_system_sample(t, &[(sid(1, MetricId::CPU_TOTAL), sample(t, 5.0))]);
        }
        while let Ok(msg) = rx.try_recv() {
            st.apply(msg);
        }
        let now = t0 + 125 * SEC;
        st.tick(now).expect("tick");

        // 1 h: 4 s buckets. Second 119 is still open, so seconds 0 to 118 fill 30 buckets.
        let h = st
            .answer(live(DeviceRange::H1), now)
            .expect("read")
            .expect("sent");
        let mut charge_h1 = vec![80.0; 15];
        charge_h1.extend([79.0; 15]);
        assert_eq!(means(&h, pct), charge_h1);
        assert_eq!(means(&h, power), [-10.0; 30]);
        for id in [pct, power] {
            let points = h.get(id).expect("series");
            let first = points.iter().position(|p| !p.is_gap()).expect("valid");
            let last = points.iter().rposition(|p| !p.is_gap()).expect("valid");
            assert!(points[first..=last].iter().all(|p| !p.is_gap()), "{id:?}");
        }

        // 24 h: the 10 s tier in 90 s buckets, the second one partial.
        let h = st
            .answer(live(DeviceRange::H24), now)
            .expect("read")
            .expect("sent");
        assert_eq!(means(&h, power), [-10.0, -10.0]);
        let charge = means(&h, pct);
        assert_eq!(charge.len(), 2);
        assert!((charge[0] - (60.0 * 80.0 + 30.0 * 79.0) / 90.0).abs() < 0.01);
        assert_eq!(charge[1], 79.0);
        st.close(now).expect("close");

        // A restart on a new boot: the battery is idx 4 now and its last hour comes back.
        let offset2 = OFFSET + 1_200 * SEC as i64;
        let t1 = 5 * SEC;
        let mut st = open(dir.path(), 2, offset2, t1);
        let mut entries: Vec<DeviceEntry> = (0..4)
            .map(|i| {
                let identity = DeviceIdentity::CpuLogical { id: i };
                entry(i, identity, &format!("cpu:{i}"), true)
            })
            .collect();
        entries.push(entry(4, battery, "battery:BAT0", true));
        st.set_catalog(catalog(1, entries));
        st.tick(t1).expect("tick");
        let h = st
            .answer(live(DeviceRange::H1), t1)
            .expect("read")
            .expect("sent");
        assert_eq!(means(&h, sid(4, MetricId::BATTERY_PCT)), charge_h1);
        assert_eq!(means(&h, sid(4, MetricId::BATTERY_POWER)), [-10.0; 30]);
    }

    /// ADR-018, level-triggered range list: 2000 random steps in which sessions open, change
    /// their range, clear it and close, the worker restarts, and the store fails to open, closes
    /// and opens again, through a queue of 2 that the worker drains at random, so range lists are
    /// dropped. The reducer keeps the answers of the generation before a restart. After each
    /// settled step with the store open every open session finds the answer for its own range,
    /// the worker's list holds no closed session, and the worker keeps no read for a range no
    /// open session shows. Every session that shows no range finds the empty answer: with the
    /// store not open, and with it open before the last settling turn reads the store.
    #[test]
    fn range_list_model() {
        let mut rng: u64 = 0x2545_f491_4f6c_dd1d;
        let mut below = |n: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % n
        };
        let choices = [
            None,
            live(DeviceRange::H1),
            live(DeviceRange::H24),
            live(DeviceRange::D7),
        ];
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut now = t0;
        let mut st = open(dir.path(), 1, OFFSET, now);
        let slot: MetricHistorySlot = Arc::new(ArcSwapOption::empty());
        let mut generation = 1;
        let (tx, mut rx) = mpsc::channel::<MetricMsg>(2);
        // Empty sample batches stand in for the other traffic that fills the queue.
        let mut samples = tx.clone();
        slot.store(Some(Arc::new(MetricTx { generation, tx })));
        let mut sink = MetricSink::new(Arc::clone(&slot));
        // The open sessions and their ranges, the local session first, as in `ClientSessions`.
        let mut sessions: Vec<(ClientSessionId, Option<DeviceHistoryRequest>)> =
            vec![(ClientSessionId::LOCAL, None)];
        let mut next_session = 1;
        // The reducer's `device_histories`; a restart keeps it.
        let mut shown: DeviceHistories = Arc::from([]);
        let (mut refused, mut settled, mut restarts) = (0, 0, 0);
        let (mut failed_opens, mut closes, mut settled_closed) = (0, 0, 0);

        // One worker turn: up to `n` messages, the empty answer with no store read, then the
        // reads. Without a store the slot is empty, so no message comes, and the worker answers
        // the key `None` alone. With `check`, the open sessions, each one that shows no range
        // must find the empty answer before the reads.
        let turn =
            |st: &mut State,
             rx: &mut mpsc::Receiver<MetricMsg>,
             shown: &mut DeviceHistories,
             local: Option<DeviceHistoryRequest>,
             now: TimeNs,
             n: usize,
             check: Option<&[(ClientSessionId, Option<DeviceHistoryRequest>)]>| {
                if !st.is_open() {
                    if let Some(answer) = st.answer_no_range() {
                        *shown = answer;
                    }
                    return;
                }
                let msgs: Vec<MetricMsg> =
                    std::iter::from_fn(|| rx.try_recv().ok()).take(n).collect();
                st.apply_ranges(&msgs);
                let reads = st.range_reads();
                let wanted = st.wanted(local);
                if let Some(answer) = st.answer_no_range_first(&wanted) {
                    *shown = answer;
                }
                assert_eq!(st.range_reads(), reads, "the empty answer read the store");
                for (id, _) in check
                    .iter()
                    .flat_map(|s| s.iter())
                    .filter(|(_, k)| k.is_none())
                {
                    let found = shown.iter().find(|(k, _)| k.is_none());
                    let Some((_, answer)) = found else {
                        panic!("no empty answer for session {id:?} before the reads");
                    };
                    assert!(answer.request.is_none() && answer.series.is_empty());
                }
                for msg in msgs {
                    st.apply(msg);
                }
                let wanted = st.wanted(local);
                if let Some(answer) = st.answer_all(&wanted, false, now).expect("read") {
                    *shown = answer;
                }
            };

        for _ in 0..2000 {
            now += SEC;
            let pick = choices[below(choices.len() as u64) as usize];
            match below(20) {
                0..=4 => {
                    sessions.push((ClientSessionId(next_session), pick));
                    next_session += 1;
                }
                5..=10 => {
                    let i = below(sessions.len() as u64) as usize;
                    sessions[i].1 = pick;
                }
                11..=12 => {
                    let i = below(sessions.len() as u64) as usize;
                    sessions[i].1 = None;
                }
                13..=17 if sessions.len() > 1 => {
                    let i = 1 + below(sessions.len() as u64 - 1) as usize;
                    sessions.remove(i);
                }
                18 => {
                    // The worker restarts: a new generation with a new channel and state. Its
                    // store fails to open half the time, and then the slot stays empty.
                    drop(std::mem::take(&mut st));
                    generation += 1;
                    let (tx, new_rx) = mpsc::channel::<MetricMsg>(2);
                    rx = new_rx;
                    samples = tx.clone();
                    if below(2) == 0 {
                        slot.store(None);
                        failed_opens += 1;
                    } else {
                        st = open(dir.path(), 1, OFFSET, now);
                        slot.store(Some(Arc::new(MetricTx { generation, tx })));
                    }
                    restarts += 1;
                }
                19 if st.is_open() => {
                    // The store closes, as when no window shows device history, and fails to
                    // open again. The worker keeps the range lists still in the queue.
                    slot.store(None);
                    st.keep_ranges(std::iter::from_fn(|| rx.try_recv().ok()));
                    st.close(now).expect("close");
                    if let Some(answer) = st.answer_closed() {
                        shown = answer;
                    }
                    failed_opens += 1;
                    closes += 1;
                }
                19 => {
                    // The store opens at last, for the same generation and channel.
                    let opened = open(dir.path(), 1, OFFSET, now);
                    st.install(opened.store.expect("store"), OFFSET);
                    let tx = samples.clone();
                    slot.store(Some(Arc::new(MetricTx { generation, tx })));
                }
                _ => {}
            }
            if slot.load().is_some() {
                for _ in 0..below(3) {
                    let _ = samples.try_send(MetricMsg::System(Box::new([])));
                }
            }
            // The end of the reducer turn.
            sink.set_ranges(&sessions);
            if sink.ranges_sent_to != Some(generation) {
                refused += 1;
            }
            let n = below(3) as usize;
            turn(&mut st, &mut rx, &mut shown, sessions[0].1, now, n, None);

            if below(4) == 0 {
                // Settle: the worker catches up and the next turn sends the list again.
                let local = sessions[0].1;
                turn(&mut st, &mut rx, &mut shown, local, now, usize::MAX, None);
                sink.set_ranges(&sessions);
                let check = Some(sessions.as_slice());
                turn(&mut st, &mut rx, &mut shown, local, now, usize::MAX, check);
                settled += 1;
                if !st.is_open() {
                    settled_closed += 1;
                    for (id, _) in sessions.iter().filter(|(_, key)| key.is_none()) {
                        let found = shown.iter().find(|(k, _)| k.is_none());
                        let Some((_, answer)) = found else {
                            panic!("no empty answer for session {id:?} while the store is closed");
                        };
                        assert!(answer.request.is_none() && answer.series.is_empty());
                    }
                    continue;
                }
                for (id, key) in &sessions {
                    let found = shown.iter().find(|(k, _)| k == key);
                    let Some((_, answer)) = found else {
                        panic!("no answer for session {id:?} on {key:?}");
                    };
                    assert_eq!(answer.request, *key);
                }
                for (id, _) in st.ranges.ranges.iter() {
                    assert!(
                        sessions.iter().any(|(open, _)| open == id),
                        "the worker holds closed session {id:?}"
                    );
                }
                for p in &st.published {
                    assert!(
                        sessions.iter().any(|(_, r)| *r == Some(p.request)),
                        "the worker keeps a read of {:?}",
                        p.request
                    );
                }
            }
        }
        assert!(refused > 50, "the queue refused {refused} range lists");
        assert!(settled > 400, "{settled} settled steps");
        assert!(restarts > 50, "{restarts} restarts");
        assert!(failed_opens > 50, "{failed_opens} failed opens");
        assert!(closes > 20, "{closes} closes");
        assert!(
            settled_closed > 50,
            "{settled_closed} settled steps with the store closed"
        );
    }

    /// A new worker generation answers the key `None` at its first read, though nothing was read:
    /// the reducer may still hold the points of the generation before it.
    #[test]
    fn a_new_worker_answers_no_range_at_once() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        let first = st.answer_all(&[None], false, t0).expect("read");
        let first = first.expect("the first read answers every key");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, None);
        assert!(first[0].1.series.is_empty());
        assert!(
            st.answer_all(&[None], false, t0 + SEC)
                .expect("read")
                .is_none()
        );
    }

    /// With the store open, a session that clears its range gets the empty answer before any
    /// store read. The other ranges keep the `Arc` sent last, so their sessions show no change,
    /// and the read that follows has nothing new to send.
    #[test]
    fn no_range_goes_out_before_the_reads() {
        let dir = tempdir();
        let t0 = 1_000 * SEC;
        let mut st = open(dir.path(), 1, OFFSET, t0);
        let day = live(DeviceRange::H24);
        let first = st
            .answer_all(&[day], false, t0)
            .expect("read")
            .expect("sent");
        let reads = st.range_reads();
        assert!(st.answer_no_range_first(&[day]).is_none());

        let early = st.answer_no_range_first(&[day, None]).expect("sent");
        assert_eq!(st.range_reads(), reads);
        assert_eq!(early.len(), 2);
        assert!(Arc::ptr_eq(&early[0].1, &first[0].1));
        assert_eq!(early[1].0, None);
        assert!(early[1].1.series.is_empty() && early[1].1.request.is_none());
        assert!(st.answer_no_range_first(&[day, None]).is_none());
        assert!(
            st.answer_all(&[day, None], false, t0 + SEC)
                .expect("read")
                .is_none()
        );

        // A range not sent yet waits for its read; the empty answer does not.
        let week = live(DeviceRange::D7);
        let dir = tempdir();
        let mut st = open(dir.path(), 1, OFFSET, t0);
        let early = st.answer_no_range_first(&[week, None]).expect("sent");
        assert_eq!(early.len(), 1);
        assert_eq!(early[0].0, None);
        assert_eq!(st.range_reads(), 0);
    }

    /// A worker whose store does not open answers the key `None` once, with no store read.
    #[test]
    fn a_store_that_fails_to_open_answers_no_range() {
        let mut st = State::new();
        let first = st.answer_no_range().expect("the first call answers");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, None);
        assert!(first[0].1.series.is_empty() && first[0].1.request.is_none());
        assert!(st.answer_no_range().is_none());
        assert_eq!(st.range_reads(), 0);
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
                MetricMsg::Reading(_) | MetricMsg::Sleep { .. } => "event",
                MetricMsg::Ranges(_) => "ranges",
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
