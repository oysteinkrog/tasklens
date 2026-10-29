//! Snapshot deltas (ADR-018 Snapshot deltas, Implementation contracts "Delta flow").
//!
//! Server side: one `DeltaEncoder` per session. It keeps the snapshot of the newest acknowledged
//! frame and at most one frame in flight, and builds each frame from the newest publish against
//! the acknowledged snapshot. Client side: `apply` builds a complete `Snapshot` from the installed
//! one and a frame.
//!
//! What a frame carries, per part of `Snapshot`:
//! - `processes`: rows keyed by `ProcessKey`. Added rows whole, changed records as a
//!   `ProcessRecordPatch`, changed extras rows whole, removed keys. `coverage`, `apps` and
//!   `low_memory` whole when they differ.
//! - `table`: rows keyed by `NodeId`. Added and changed rows whole, and the node order whenever
//!   the order or the set of rows changed. `groups` and `subtree` whole when they differ.
//! - `series` and `process_series`: per series id, the samples after the base's last sample.
//!   A series whose base samples left the ring, or whose ring was reset, goes whole.
//! - `device_history`: whole when `request`, `step_ns` or `realtime_offset_ns` changed, or when a
//!   point the client already has changed (decimation). Otherwise per series the appended points;
//!   the base's last point may be sent again with a new value.
//! - Every other part: whole when its `Arc` is not the base's.
//!
//! Component headers are copied as sent, so the client's component versions are the server's.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use tl_model::ids::{NodeId, ProcessSeriesId};
use tl_model::process::{ProcessExtras, ProcessExtrasP2, ProcessRecord, ProcessSnapshot};
use tl_model::remote_proto::{
    DeviceHistoryDelta, HEADER_LEN, KeyedDelta, MAX_FRAME_LEN, Part, ProcessChange,
    ProcessRecordPatch, ProcessRow, ProcessSeriesDelta, ProcessesDelta, ProtoError, SeriesSetDelta,
    SnapshotFrame, TableDelta, Tail, ViewTail, ViewsDelta, encode_message,
};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{DeviceHistory, HistPoint, ProcessSeriesSet, SeriesSet, Snapshot};
use tl_model::table::{ProcessTableView, TableRow};
use tl_model::time::TimeNs;

/// A complete frame goes out at least this often: 60 s.
pub const COMPLETE_EVERY_NS: TimeNs = 60_000_000_000;
/// The default size limit of one encoded frame: the envelope's 64 MiB less room for the
/// `ServerMsg` tag around the frame.
pub const FRAME_LIMIT: usize = MAX_FRAME_LEN - 16;

#[derive(Debug)]
pub enum DeltaError {
    /// The frame is a delta against a frame the client has not installed.
    WrongBase {
        base: u64,
        installed: u64,
    },
    /// An `Ack` for a frame that is not in flight.
    UnknownAck {
        frame: u64,
    },
    /// The frame does not fit its base: a key that is not there, a list out of order, a range
    /// past the end.
    Malformed(&'static str),
    /// The frame is over the size limit even without `process_series` and `device_history`.
    TooLarge {
        len: usize,
        max: usize,
    },
    Proto(ProtoError),
}

impl fmt::Display for DeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeltaError::WrongBase { base, installed } => {
                write!(
                    f,
                    "frame is a delta against {base}, installed is {installed}"
                )
            }
            DeltaError::UnknownAck { frame } => write!(f, "ack for frame {frame}, not in flight"),
            DeltaError::Malformed(what) => write!(f, "bad snapshot frame: {what}"),
            DeltaError::TooLarge { len, max } => {
                write!(
                    f,
                    "snapshot frame of {len} bytes exceeds the limit of {max}"
                )
            }
            DeltaError::Proto(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for DeltaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeltaError::Proto(e) => Some(e),
            _ => None,
        }
    }
}

// ---- Server side ----

/// One frame from `DeltaEncoder::next_frame`.
#[derive(Debug)]
pub struct BuiltFrame {
    pub frame: SnapshotFrame,
    /// The full frame was over the size limit, so this one leaves out `process_series` and
    /// `device_history`; later frames carry them. The caller logs this.
    pub trimmed: bool,
}

/// Builds the snapshot frames of one session.
#[derive(Debug)]
pub struct DeltaEncoder {
    limit: usize,
    next: u64,
    /// The newest acknowledged frame and the snapshot the client built from it.
    acked: Option<(u64, Snapshot)>,
    /// The frame sent and not yet acknowledged, with the snapshot the client will build from it.
    in_flight: Option<(u64, Snapshot)>,
    last_complete_ns: Option<TimeNs>,
    force_complete: bool,
}

impl Default for DeltaEncoder {
    fn default() -> Self {
        Self::new(FRAME_LIMIT)
    }
}

impl DeltaEncoder {
    /// `limit` is the largest encoded frame in bytes, without the envelope header.
    pub fn new(limit: usize) -> Self {
        DeltaEncoder {
            limit,
            next: 1,
            acked: None,
            in_flight: None,
            last_complete_ns: None,
            force_complete: false,
        }
    }

    /// The frame sent and not yet acknowledged.
    pub fn in_flight(&self) -> Option<u64> {
        self.in_flight.as_ref().map(|(n, _)| *n)
    }

    /// The next frame is complete. Call it after an engine instance change.
    pub fn force_complete(&mut self) {
        self.force_complete = true;
    }

    /// The next frame, built from `newest`, or `None` while a frame is in flight. The frame is
    /// complete when it is the first, after `force_complete`, and when the last complete frame is
    /// 60 s older than `now_ns`; otherwise it is a delta against the acknowledged frame.
    pub fn next_frame(
        &mut self,
        newest: &Snapshot,
        now_ns: TimeNs,
    ) -> Result<Option<BuiltFrame>, DeltaError> {
        if self.in_flight.is_some() {
            return Ok(None);
        }
        let number = self.next;
        let complete = self.force_complete
            || self.acked.is_none()
            || self
                .last_complete_ns
                .is_none_or(|t| now_ns.saturating_sub(t) >= COMPLETE_EVERY_NS);
        let base = if complete {
            None
        } else {
            self.acked.as_ref().map(|(n, s)| (*n, s))
        };
        let mut frame = encode(base, newest, number);
        let mut pending = newest.clone();
        let mut trimmed = false;
        if encoded_len(&frame)? > self.limit {
            frame.process_series = None;
            frame.device_history = None;
            let len = encoded_len(&frame)?;
            if len > self.limit {
                return Err(DeltaError::TooLarge {
                    len,
                    max: self.limit,
                });
            }
            // The client keeps the base's parts, or starts from defaults on a complete frame.
            // The next delta must be built against what the client has.
            match base {
                Some((_, b)) => {
                    pending.process_series = b.process_series.clone();
                    pending.device_history = b.device_history.clone();
                }
                None => {
                    pending.process_series = Arc::default();
                    pending.device_history = Arc::default();
                }
            }
            trimmed = true;
        }
        if complete {
            self.last_complete_ns = Some(now_ns);
            self.force_complete = false;
        }
        self.next += 1;
        self.in_flight = Some((number, pending));
        Ok(Some(BuiltFrame { frame, trimmed }))
    }

    /// The client has installed `frame`. It becomes the base of the next frame.
    pub fn on_ack(&mut self, frame: u64) -> Result<(), DeltaError> {
        match self.in_flight.take() {
            Some((n, snapshot)) if n == frame => {
                self.acked = Some((n, snapshot));
                Ok(())
            }
            other => {
                self.in_flight = other;
                Err(DeltaError::UnknownAck { frame })
            }
        }
    }
}

/// The payload length of `frame` as one session frame. Over 64 MiB counts as its real length.
fn encoded_len(frame: &SnapshotFrame) -> Result<usize, DeltaError> {
    let mut bytes = Vec::new();
    match encode_message(frame, &mut bytes) {
        Ok(()) => Ok(bytes.len() - HEADER_LEN),
        Err(ProtoError::TooLarge { len, .. }) => Ok(len),
        Err(e) => Err(DeltaError::Proto(e)),
    }
}

/// Frame `frame` for `new`: a delta against `base` (frame number and snapshot), or complete when
/// `base` is `None`.
pub fn encode(base: Option<(u64, &Snapshot)>, new: &Snapshot, frame: u64) -> SnapshotFrame {
    let (base_no, b) = match base {
        Some((n, s)) => (n, Some(s)),
        None => (0, None),
    };
    let mut f = SnapshotFrame::unchanged(frame, base_no, new.epoch, new.boot_id, new.published_ns);
    f.system = whole(b.map(|b| &b.system), &new.system);
    f.series = part(b.map(|b| &b.series), &new.series, series_delta);
    f.processes = part(b.map(|b| &b.processes), &new.processes, processes_delta);
    f.table = part(b.map(|b| &b.table), &new.table, table_delta);
    f.connections = whole(b.map(|b| &b.connections), &new.connections);
    f.services = whole(b.map(|b| &b.services), &new.services);
    f.startup = whole(b.map(|b| &b.startup), &new.startup);
    f.inventory = whole(b.map(|b| &b.inventory), &new.inventory);
    f.devices = whole(b.map(|b| &b.devices), &new.devices);
    f.health = whole(b.map(|b| &b.health), &new.health);
    f.windows = whole(b.map(|b| &b.windows), &new.windows);
    f.details = whole(b.map(|b| &b.details), &new.details);
    f.history = whole(b.map(|b| &b.history), &new.history);
    f.process_series = part(
        b.map(|b| &b.process_series),
        &new.process_series,
        process_series_delta,
    );
    f.device_details = whole(b.map(|b| &b.device_details), &new.device_details);
    f.device_history = part(
        b.map(|b| &b.device_history),
        &new.device_history,
        device_history_delta,
    );
    f
}

/// The delta cannot express the change; the part goes whole.
struct SendWhole;

fn whole<T: ?Sized>(base: Option<&Arc<T>>, new: &Arc<T>) -> Option<Arc<T>> {
    match base {
        Some(b) if Arc::ptr_eq(b, new) => None,
        _ => Some(new.clone()),
    }
}

fn part<W, D>(
    base: Option<&Arc<W>>,
    new: &Arc<W>,
    delta: impl FnOnce(&W, &W) -> Result<D, SendWhole>,
) -> Option<Part<Arc<W>, D>> {
    match base {
        None => Some(Part::Whole(new.clone())),
        Some(b) if Arc::ptr_eq(b, new) => None,
        Some(b) => Some(delta(b, new).map_or_else(|_| Part::Whole(new.clone()), Part::Delta)),
    }
}

/// `new` when it differs from `base`.
fn changed<T: ?Sized + PartialEq>(base: &Arc<T>, new: &Arc<T>) -> Option<Arc<T>> {
    (!Arc::ptr_eq(base, new) && **base != **new).then(|| new.clone())
}

fn changed_opt<T: ?Sized + PartialEq>(
    base: &Option<Arc<T>>,
    new: &Option<Arc<T>>,
) -> Option<Option<Arc<T>>> {
    let same = match (base, new) {
        (Some(b), Some(n)) => Arc::ptr_eq(b, n) || **b == **n,
        (None, None) => true,
        _ => false,
    };
    (!same).then(|| new.clone())
}

/// Changes between two lists sorted by key, walked by index. `change(i, j)` compares base row `i`
/// with new row `j` of the same key and returns `None` when they are the same. Lists that are not
/// strictly sorted send the part whole.
fn keyed_delta<K: Ord, W, D>(
    lens: (usize, usize),
    base_key: impl Fn(usize) -> K,
    new_key: impl Fn(usize) -> K,
    mut change: impl FnMut(usize, usize) -> Result<Option<Part<W, D>>, SendWhole>,
    added: impl Fn(usize) -> W,
) -> Result<KeyedDelta<K, W, D>, SendWhole> {
    let (base_len, new_len) = lens;
    if !sorted(base_len, &base_key) || !sorted(new_len, &new_key) {
        return Err(SendWhole);
    }
    let mut out = KeyedDelta {
        updates: Vec::new(),
        removed: Vec::new(),
    };
    let (mut i, mut j) = (0, 0);
    while i < base_len || j < new_len {
        let order = if i == base_len {
            std::cmp::Ordering::Greater
        } else if j == new_len {
            std::cmp::Ordering::Less
        } else {
            base_key(i).cmp(&new_key(j))
        };
        match order {
            std::cmp::Ordering::Less => {
                out.removed.push(base_key(i));
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.updates.push((new_key(j), Part::Whole(added(j))));
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                if let Some(p) = change(i, j)? {
                    out.updates.push((new_key(j), p));
                }
                i += 1;
                j += 1;
            }
        }
    }
    Ok(out)
}

fn sorted<K: Ord>(len: usize, key: impl Fn(usize) -> K) -> bool {
    (1..len).all(|i| key(i - 1) < key(i))
}

fn same_sample(a: &Sample, b: &Sample) -> bool {
    a.t_ns == b.t_ns && a.value.to_bits() == b.value.to_bits() && a.flags == b.flags
}

fn same_point(a: &HistPoint, b: &HistPoint) -> bool {
    a.t_realtime_ns == b.t_realtime_ns
        && a.mean.to_bits() == b.mean.to_bits()
        && a.min.to_bits() == b.min.to_bits()
        && a.max.to_bits() == b.max.to_bits()
}

/// `new` as the base's samples from its last one back, plus the samples after it. `None` when
/// the base's last sample is not in `new`, or the samples before it differ (eviction past the
/// base, reset).
fn sample_tail(base: &[Sample], new: &[Sample]) -> Option<Tail<Sample>> {
    let last = base.last()?;
    let keep = new.iter().rposition(|s| same_sample(s, last))? + 1;
    let skip = base.len().checked_sub(keep)?;
    let kept_same = new[..keep]
        .iter()
        .zip(&base[skip..])
        .all(|(a, b)| same_sample(a, b));
    kept_same.then(|| Tail {
        skip: skip as u32,
        keep: keep as u32,
        appended: Arc::from(&new[keep..]),
    })
}

fn view_part(
    base: &Arc<SeriesView>,
    new: &Arc<SeriesView>,
) -> Option<Part<Arc<SeriesView>, ViewTail>> {
    if Arc::ptr_eq(base, new) {
        return None;
    }
    let tail = (base.id == new.id)
        .then(|| sample_tail(&base.samples, &new.samples))
        .flatten();
    let Some(samples) = tail else {
        return Some(Part::Whole(new.clone()));
    };
    let same = samples.skip == 0
        && samples.keep as usize == base.samples.len()
        && samples.appended.is_empty()
        && base.version == new.version
        && base.period_ns == new.period_ns;
    (!same).then_some(Part::Delta(ViewTail {
        version: new.version,
        period_ns: new.period_ns,
        samples,
    }))
}

fn views_delta<T, K: Ord>(
    base: &[T],
    new: &[T],
    key: impl Fn(&T) -> K,
    view: impl Fn(&T) -> &Arc<SeriesView>,
) -> Result<ViewsDelta<K>, SendWhole> {
    keyed_delta(
        (base.len(), new.len()),
        |i| key(&base[i]),
        |j| key(&new[j]),
        |i, j| Ok(view_part(view(&base[i]), view(&new[j]))),
        |j| view(&new[j]).clone(),
    )
}

fn series_delta(b: &SeriesSet, n: &SeriesSet) -> Result<SeriesSetDelta, SendWhole> {
    Ok(SeriesSetDelta {
        header: n.header,
        series: views_delta(&b.series, &n.series, |v| v.id, |v| v)?,
        slow: views_delta(&b.slow, &n.slow, |v| v.id, |v| v)?,
    })
}

fn process_series_delta(
    b: &ProcessSeriesSet,
    n: &ProcessSeriesSet,
) -> Result<ProcessSeriesDelta, SendWhole> {
    Ok(ProcessSeriesDelta {
        header: n.header,
        series: views_delta(&b.series, &n.series, |(k, _)| k.clone(), |(_, v)| v)?,
        slow: views_delta(&b.slow, &n.slow, |(k, _)| k.clone(), |(_, v)| v)?,
    })
}

/// `extras` and `extras_p2` are index aligned with `records`.
fn aligned(p: &ProcessSnapshot) -> bool {
    p.extras.len() == p.records.len() && p.extras_p2.len() == p.records.len()
}

fn processes_delta(b: &ProcessSnapshot, n: &ProcessSnapshot) -> Result<ProcessesDelta, SendWhole> {
    if !aligned(b) || !aligned(n) {
        return Err(SendWhole);
    }
    let rows = keyed_delta(
        (b.records.len(), n.records.len()),
        |i| b.records[i].key,
        |j| n.records[j].key,
        |i, j| {
            let change = ProcessChange {
                record: ProcessRecordPatch::diff(&b.records[i], &n.records[j]),
                extras: (b.extras[i] != n.extras[j]).then(|| n.extras[j].clone()),
                extras_p2: (b.extras_p2[i] != n.extras_p2[j]).then(|| n.extras_p2[j].clone()),
            };
            let same =
                change.record.is_none() && change.extras.is_none() && change.extras_p2.is_none();
            Ok((!same).then_some(Part::Delta(change)))
        },
        |j| ProcessRow {
            record: n.records[j].clone(),
            extras: n.extras[j].clone(),
            extras_p2: n.extras_p2[j].clone(),
        },
    )?;
    let coverage_same = Arc::ptr_eq(&b.coverage.pss_uss, &n.coverage.pss_uss)
        || b.coverage.pss_uss == n.coverage.pss_uss;
    Ok(ProcessesDelta {
        header: n.header,
        rows,
        coverage: (!coverage_same).then(|| n.coverage.clone()),
        apps: changed(&b.apps, &n.apps),
        low_memory: changed_opt(&b.low_memory, &n.low_memory),
    })
}

fn table_delta(b: &ProcessTableView, n: &ProcessTableView) -> Result<TableDelta, SendWhole> {
    let index: HashMap<&NodeId, usize> = b
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| (&r.node, i))
        .collect();
    if index.len() != b.rows.len() {
        return Err(SendWhole);
    }
    let same_order = b.rows.len() == n.rows.len()
        && b.rows
            .iter()
            .zip(n.rows.iter())
            .all(|(x, y)| x.node == y.node);
    if !same_order && n.rows.iter().map(|r| &r.node).collect::<HashSet<_>>().len() != n.rows.len() {
        return Err(SendWhole);
    }
    let rows = n
        .rows
        .iter()
        .filter(|r| index.get(&r.node).is_none_or(|&i| b.rows[i] != **r))
        .cloned()
        .collect();
    Ok(TableDelta {
        header: n.header,
        query_generation: n.query_generation,
        rows,
        order: (!same_order).then(|| n.rows.iter().map(|r| r.node.clone()).collect()),
        groups: changed(&b.groups, &n.groups),
        subtree: changed(&b.subtree, &n.subtree),
        totals: n.totals.clone(),
        total: n.total,
        shown: n.shown,
        restricted: n.restricted,
    })
}

/// The points of `new` as a tail of `base`. `SendWhole` when a point the client has changed,
/// other than the base's last one.
fn hist_tail(
    base: &[HistPoint],
    new: &Arc<[HistPoint]>,
) -> Result<Part<Arc<[HistPoint]>, Tail<HistPoint>>, SendWhole> {
    let (Some(first), Some(last)) = (new.first(), base.last()) else {
        return Ok(Part::Whole(new.clone()));
    };
    if first.t_realtime_ns > last.t_realtime_ns {
        return Ok(Part::Whole(new.clone()));
    }
    let skip = base
        .binary_search_by_key(&first.t_realtime_ns, |p| p.t_realtime_ns)
        .map_err(|_| SendWhole)?;
    let keep = new
        .iter()
        .zip(&base[skip..])
        .take_while(|(a, b)| same_point(a, b))
        .count();
    if base.len() - skip - keep > 1 {
        return Err(SendWhole);
    }
    Ok(Part::Delta(Tail {
        skip: skip as u32,
        keep: keep as u32,
        appended: Arc::from(&new[keep..]),
    }))
}

fn device_history_delta(
    b: &DeviceHistory,
    n: &DeviceHistory,
) -> Result<DeviceHistoryDelta, SendWhole> {
    if b.request != n.request
        || b.step_ns != n.step_ns
        || b.realtime_offset_ns != n.realtime_offset_ns
    {
        return Err(SendWhole);
    }
    let series = keyed_delta(
        (b.series.len(), n.series.len()),
        |i| b.series[i].0,
        |j| n.series[j].0,
        |i, j| {
            let (bp, np) = (&b.series[i].1, &n.series[j].1);
            if Arc::ptr_eq(bp, np) {
                Ok(None)
            } else {
                hist_tail(bp, np).map(Some)
            }
        },
        |j| n.series[j].1.clone(),
    )?;
    Ok(DeviceHistoryDelta {
        header: n.header,
        since_realtime_ns: n.since_realtime_ns,
        series,
        boots: changed(&b.boots, &n.boots),
        timeline: changed_opt(&b.timeline, &n.timeline),
    })
}

// ---- Client side ----

/// Builds the snapshot of `frame`. A complete frame (`base` 0) starts from
/// `Snapshot::empty`; a delta needs `installed_frame` to be its base and starts from `installed`,
/// reusing the installed `Arc` of every part the frame does not carry. `sequence` is the client's
/// own (ADR-005 hub check across engine restarts).
pub fn apply(
    installed: &Snapshot,
    installed_frame: u64,
    frame: &SnapshotFrame,
    sequence: u64,
) -> Result<Snapshot, DeltaError> {
    let empty;
    let base = if frame.base == 0 {
        empty = Snapshot::empty(frame.boot_id, frame.epoch);
        &empty
    } else if frame.base == installed_frame {
        installed
    } else {
        return Err(DeltaError::WrongBase {
            base: frame.base,
            installed: installed_frame,
        });
    };
    Ok(Snapshot {
        sequence,
        epoch: frame.epoch,
        boot_id: frame.boot_id,
        published_ns: frame.published_ns,
        system: pick(&base.system, &frame.system),
        series: apply_part(&base.series, &frame.series, apply_series)?,
        processes: apply_part(&base.processes, &frame.processes, apply_processes)?,
        table: apply_part(&base.table, &frame.table, apply_table)?,
        connections: pick(&base.connections, &frame.connections),
        services: pick(&base.services, &frame.services),
        startup: pick(&base.startup, &frame.startup),
        inventory: pick(&base.inventory, &frame.inventory),
        devices: pick(&base.devices, &frame.devices),
        health: pick(&base.health, &frame.health),
        windows: pick(&base.windows, &frame.windows),
        details: pick(&base.details, &frame.details),
        history: pick(&base.history, &frame.history),
        process_series: apply_part(
            &base.process_series,
            &frame.process_series,
            apply_process_series,
        )?,
        device_details: pick(&base.device_details, &frame.device_details),
        device_history: apply_part(
            &base.device_history,
            &frame.device_history,
            apply_device_history,
        )?,
    })
}

fn pick<T>(base: &Arc<T>, sent: &Option<Arc<T>>) -> Arc<T> {
    sent.as_ref().unwrap_or(base).clone()
}

fn apply_part<W, D>(
    base: &Arc<W>,
    sent: &Option<Part<Arc<W>, D>>,
    delta: impl FnOnce(&W, &D) -> Result<W, DeltaError>,
) -> Result<Arc<W>, DeltaError> {
    match sent {
        None => Ok(base.clone()),
        Some(Part::Whole(w)) => Ok(w.clone()),
        Some(Part::Delta(d)) => delta(base, d).map(Arc::new),
    }
}

/// Walks a base list sorted by key together with `delta`. Calls `emit(key, base index, update)`
/// for every row of the result in key order: `(Some(i), None)` keeps base row `i`,
/// `(Some(i), Some(p))` updates it and `(None, Some(Part::Whole))` adds a row.
fn apply_keyed<K: Ord, W, D>(
    base_len: usize,
    base_key: impl Fn(usize) -> K,
    delta: &KeyedDelta<K, W, D>,
    mut emit: impl FnMut(&K, Option<usize>, Option<&Part<W, D>>) -> Result<(), DeltaError>,
) -> Result<(), DeltaError> {
    let updates = &delta.updates;
    if !sorted(updates.len(), |i| &updates[i].0)
        || !sorted(delta.removed.len(), |i| &delta.removed[i])
    {
        return Err(DeltaError::Malformed("delta keys out of order"));
    }
    let mut u = updates.iter().peekable();
    let mut r = delta.removed.iter().peekable();
    for i in 0..base_len {
        let k = base_key(i);
        while let Some((uk, p)) = u.next_if(|(uk, _)| *uk < k) {
            emit(uk, None, Some(added(p)?))?;
        }
        if r.next_if(|rk| **rk < k).is_some() {
            return Err(DeltaError::Malformed("removed key not in base"));
        }
        let removed = r.next_if(|rk| **rk == k).is_some();
        let update = u.next_if(|(uk, _)| *uk == k).map(|(_, p)| p);
        match (removed, update) {
            (true, Some(_)) => return Err(DeltaError::Malformed("key removed and updated")),
            (true, None) => {}
            (false, update) => emit(&k, Some(i), update)?,
        }
    }
    for (uk, p) in u {
        emit(uk, None, Some(added(p)?))?;
    }
    if r.next().is_some() {
        return Err(DeltaError::Malformed("removed key not in base"));
    }
    Ok(())
}

/// An update of a key the base does not have must be whole.
fn added<W, D>(p: &Part<W, D>) -> Result<&Part<W, D>, DeltaError> {
    match p {
        Part::Whole(_) => Ok(p),
        Part::Delta(_) => Err(DeltaError::Malformed(
            "delta for a key the base does not have",
        )),
    }
}

fn apply_tail<T: Copy>(base: &[T], tail: &Tail<T>) -> Result<Arc<[T]>, DeltaError> {
    let (skip, keep) = (tail.skip as usize, tail.keep as usize);
    let kept = skip
        .checked_add(keep)
        .and_then(|end| base.get(skip..end))
        .ok_or(DeltaError::Malformed("tail past the end of the base"))?;
    Ok(kept.iter().chain(tail.appended.iter()).copied().collect())
}

fn apply_views<T: Clone, K: Ord + Clone>(
    base: &[T],
    key: impl Fn(&T) -> K,
    view: impl Fn(&T) -> &Arc<SeriesView>,
    make: impl Fn(&K, Arc<SeriesView>) -> T,
    delta: &ViewsDelta<K>,
) -> Result<Arc<[T]>, DeltaError> {
    let mut out = Vec::with_capacity(base.len() + delta.updates.len());
    apply_keyed(
        base.len(),
        |i| key(&base[i]),
        delta,
        |k, i, p| {
            out.push(match (i, p) {
                (Some(i), None) => base[i].clone(),
                (_, Some(Part::Whole(v))) => make(k, v.clone()),
                (Some(i), Some(Part::Delta(t))) => {
                    let b = view(&base[i]);
                    make(
                        k,
                        Arc::new(SeriesView {
                            id: b.id,
                            version: t.version,
                            period_ns: t.period_ns,
                            samples: apply_tail(&b.samples, &t.samples)?,
                        }),
                    )
                }
                (None, _) => {
                    return Err(DeltaError::Malformed(
                        "delta for a key the base does not have",
                    ));
                }
            });
            Ok(())
        },
    )?;
    Ok(out.into())
}

fn apply_series(b: &SeriesSet, d: &SeriesSetDelta) -> Result<SeriesSet, DeltaError> {
    Ok(SeriesSet {
        header: d.header,
        series: apply_views(&b.series, |v| v.id, |v| v, |_, v| v, &d.series)?,
        slow: apply_views(&b.slow, |v| v.id, |v| v, |_, v| v, &d.slow)?,
    })
}

fn apply_process_series(
    b: &ProcessSeriesSet,
    d: &ProcessSeriesDelta,
) -> Result<ProcessSeriesSet, DeltaError> {
    let key = |(k, _): &(ProcessSeriesId, Arc<SeriesView>)| k.clone();
    Ok(ProcessSeriesSet {
        header: d.header,
        series: apply_views(&b.series, key, |(_, v)| v, |k, v| (k.clone(), v), &d.series)?,
        slow: apply_views(&b.slow, key, |(_, v)| v, |k, v| (k.clone(), v), &d.slow)?,
    })
}

fn apply_processes(b: &ProcessSnapshot, d: &ProcessesDelta) -> Result<ProcessSnapshot, DeltaError> {
    if !aligned(b) {
        return Err(DeltaError::Malformed(
            "process extras not aligned with records",
        ));
    }
    let cap = b.records.len() + d.rows.updates.len();
    let mut records: Vec<ProcessRecord> = Vec::with_capacity(cap);
    let mut extras: Vec<ProcessExtras> = Vec::with_capacity(cap);
    let mut extras_p2: Vec<ProcessExtrasP2> = Vec::with_capacity(cap);
    apply_keyed(
        b.records.len(),
        |i| b.records[i].key,
        &d.rows,
        |key, i, p| {
            match (i, p) {
                (Some(i), None) => {
                    records.push(b.records[i].clone());
                    extras.push(b.extras[i].clone());
                    extras_p2.push(b.extras_p2[i].clone());
                }
                (_, Some(Part::Whole(row))) => {
                    if row.record.key != *key {
                        return Err(DeltaError::Malformed("process row under another key"));
                    }
                    records.push(row.record.clone());
                    extras.push(row.extras.clone());
                    extras_p2.push(row.extras_p2.clone());
                }
                (Some(i), Some(Part::Delta(c))) => {
                    let mut record = b.records[i].clone();
                    if let Some(patch) = &c.record {
                        patch.apply(&mut record);
                    }
                    records.push(record);
                    extras.push(c.extras.as_ref().unwrap_or(&b.extras[i]).clone());
                    extras_p2.push(c.extras_p2.as_ref().unwrap_or(&b.extras_p2[i]).clone());
                }
                (None, _) => {
                    return Err(DeltaError::Malformed(
                        "delta for a key the base does not have",
                    ));
                }
            }
            Ok(())
        },
    )?;
    Ok(ProcessSnapshot {
        header: d.header,
        records: records.into(),
        coverage: d.coverage.as_ref().unwrap_or(&b.coverage).clone(),
        extras: extras.into(),
        apps: d.apps.as_ref().unwrap_or(&b.apps).clone(),
        extras_p2: extras_p2.into(),
        low_memory: d.low_memory.as_ref().unwrap_or(&b.low_memory).clone(),
    })
}

fn apply_table(b: &ProcessTableView, d: &TableDelta) -> Result<ProcessTableView, DeltaError> {
    let base: HashMap<&NodeId, usize> = b
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| (&r.node, i))
        .collect();
    let rows: Arc<[TableRow]> = match &d.order {
        None if d.rows.is_empty() => b.rows.clone(),
        None => {
            let mut rows = b.rows.to_vec();
            for row in &d.rows {
                let i = base
                    .get(&row.node)
                    .ok_or(DeltaError::Malformed("changed table row not in base"))?;
                rows[*i] = row.clone();
            }
            rows.into()
        }
        Some(order) => {
            let sent: HashMap<&NodeId, &TableRow> = d.rows.iter().map(|r| (&r.node, r)).collect();
            order
                .iter()
                .map(|node| {
                    sent.get(node)
                        .copied()
                        .or_else(|| base.get(node).map(|&i| &b.rows[i]))
                        .cloned()
                        .ok_or(DeltaError::Malformed(
                            "table order names a row that was not sent",
                        ))
                })
                .collect::<Result<_, _>>()?
        }
    };
    Ok(ProcessTableView {
        header: d.header,
        query_generation: d.query_generation,
        rows,
        groups: d.groups.as_ref().unwrap_or(&b.groups).clone(),
        subtree: d.subtree.as_ref().unwrap_or(&b.subtree).clone(),
        totals: d.totals.clone(),
        total: d.total,
        shown: d.shown,
        restricted: d.restricted,
    })
}

fn apply_device_history(
    b: &DeviceHistory,
    d: &DeviceHistoryDelta,
) -> Result<DeviceHistory, DeltaError> {
    let mut series = Vec::with_capacity(b.series.len() + d.series.updates.len());
    apply_keyed(
        b.series.len(),
        |i| b.series[i].0,
        &d.series,
        |id, i, p| {
            let points = match (i, p) {
                (Some(i), None) => b.series[i].1.clone(),
                (_, Some(Part::Whole(points))) => points.clone(),
                (Some(i), Some(Part::Delta(tail))) => apply_tail(&b.series[i].1, tail)?,
                (None, _) => {
                    return Err(DeltaError::Malformed(
                        "delta for a key the base does not have",
                    ));
                }
            };
            series.push((*id, points));
            Ok(())
        },
    )?;
    Ok(DeviceHistory {
        header: d.header,
        request: b.request,
        realtime_offset_ns: b.realtime_offset_ns,
        since_realtime_ns: d.since_realtime_ns,
        step_ns: b.step_ns,
        series: series.into(),
        boots: d.boots.as_ref().unwrap_or(&b.boots).clone(),
        timeline: d.timeline.as_ref().unwrap_or(&b.timeline).clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proptest::prelude::*;
    use tl_model::actions::{CgroupRef, SchedPolicy};
    use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
    use tl_model::details::{AffinityInfo, DetailsSnapshot};
    use tl_model::ids::{
        ColumnId, DeviceIdx, GraphTarget, GroupKey, MetricId, ProcessKey, SeriesId,
    };
    use tl_model::process::{
        AppCgroupSample, Coverage, GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState,
        ProcessMeta,
    };
    use tl_model::remote_proto::{RawFrame, decode_message, read_frame};
    use tl_model::snapshot::{
        ComponentHeader, DeviceTimeline, ServiceBackend, ServiceStatus, ServicesSnapshot,
        StartupType, SystemSnapshot, UnitRecord,
    };
    use tl_model::system::CpuSet;
    use tl_model::table::{Manager, Total, UnitRef, UnitType};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};

    use super::*;

    const BOOT: BootId = BootId(7);
    const SEC: u64 = 1_000_000_000;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 100 + pid as u64 * 3,
            boot_id: BOOT,
            pid,
        }
    }

    fn header(version: u64) -> ComponentHeader {
        ComponentHeader {
            version,
            observed_start: version * SEC,
            observed_end: version * SEC + 1,
            validity: Validity::Valid,
        }
    }

    fn text(s: String) -> Arc<str> {
        Arc::from(s)
    }

    fn meta(pid: u32) -> Arc<ProcessMeta> {
        Arc::new(ProcessMeta {
            comm: text(format!("proc{pid}")),
            name: text(format!("Process {pid}")),
            exe: Some(text(format!("/usr/bin/proc{pid}"))),
            cmdline: Some(text(format!("proc{pid} --flag"))),
            user: Arc::from("user"),
            cgroup: Some(Arc::from("user.slice/app.slice")),
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: text(format!("process {pid}")),
        })
    }

    fn record(pid: u32) -> ProcessRecord {
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: meta(pid),
            group: GroupKey(pid as u64 % 3),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 4,
            cpu_pct: pid as f32 / 100.0,
            rss_anon_bytes: pid as u64 * 4096,
            rss_bytes: pid as u64 * 8192,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 1.5,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn extras(pid: u32) -> ProcessExtras {
        ProcessExtras {
            cpu_kernel_pct: 0.25,
            cpu_user_pct: 0.5,
            cpu_relative_pct: 1.0,
            utime_ticks: pid as u64,
            stime_ticks: 3,
            start_boot_ns: pid as u64 * SEC,
            rss_peak_bytes: 1 << 20,
            rss_shared_bytes: 1 << 16,
            commit_bytes: 1 << 21,
            vsize_bytes: 1 << 24,
            vsize_peak_bytes: 1 << 25,
            page_faults: 10,
            page_faults_delta: 0,
            major_faults: 0,
            policy: SchedPolicy::Other,
            rt_prio: 0,
            affinity: CpuSet::default(),
            handles: Some(12),
            exe_name: text(format!("proc{pid}")),
            exe_path: text(format!("/usr/bin/proc{pid}")),
            exe_deleted: false,
            unit: None,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
            pgrp: pid,
            sid: pid,
            last_cpu: pid % 16,
        }
    }

    fn extras_p2(pid: u32) -> ProcessExtrasP2 {
        if pid % 5 == 0 {
            ProcessExtrasP2 {
                key: Some(key(pid)),
                oom_score: Some(pid as i32),
                cwd: Some(Arc::from("/home/user")),
                ..ProcessExtrasP2::default()
            }
        } else {
            ProcessExtrasP2::default()
        }
    }

    fn group_row(n: u64) -> GroupRow {
        GroupRow {
            node: NodeId::Group(GroupKey(n)),
            key: GroupKey(n),
            kind: GroupRowKind::App,
            class: GroupKind::App {
                app_id: text(format!("app{n}")),
            },
            name: text(format!("App {n}")),
            icon: None,
            members: 3,
            pids: 3,
            note: None,
            memory_bytes: n << 20,
            disk_bps: 0.0,
            net_bps: 0.0,
            gpu_pct: 0.0,
            cpu_pct: 2.5,
            rss_anon_bytes: n << 20,
            rss_bytes: n << 21,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            threads: 9,
            cpu_kernel_pct: 0.5,
            cpu_user_pct: 2.0,
            cpu_relative_pct: 2.5,
            utime_ticks: 7,
            stime_ticks: 2,
            commit_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            handles: Some(30),
            desktop_path: None,
            description: None,
            user: None,
            frozen: false,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn unit_record(name: &str) -> UnitRecord {
        UnitRecord {
            unit: UnitRef {
                manager: Manager::User,
                name: Arc::from(name),
            },
            kind: UnitType::Service,
            description: Arc::from("A service"),
            status: ServiceStatus::Running,
            sub_state: Arc::from("running"),
            load_state: Arc::from("loaded"),
            startup: StartupType::Enabled,
            main_pid: 42,
            user: Arc::from("user"),
            path: Arc::from("/org/freedesktop/systemd1/unit/a"),
            unit_file: None,
            active_since_us: Some(5),
            triggered_by: Arc::from([]),
            control_group: None,
            can_reload: false,
            has_cgroup_procs: true,
            cgroup_pids: Arc::from([key(1)]),
            cpu_pct: Total {
                value: 1.0,
                validity: Validity::Valid,
            },
            memory_bytes: Total::default(),
            disk_bps: Total::default(),
            tasks: Total::default(),
        }
    }

    /// Samples at 1 s from `start`, valid, `n` of them.
    fn samples(start: u64, n: u64) -> Arc<[Sample]> {
        (start..start + n)
            .map(|t| Sample::valid(t * SEC, (t % 97) as f32))
            .collect()
    }

    fn view(id: SeriesId, start: u64, n: u64) -> Arc<SeriesView> {
        Arc::new(SeriesView {
            id,
            version: start + n,
            period_ns: SEC,
            samples: samples(start, n),
        })
    }

    fn sid(metric: u16) -> SeriesId {
        SeriesId {
            device: DeviceIdx(0),
            metric: MetricId(metric),
        }
    }

    fn points(start: i64, n: i64) -> Arc<[HistPoint]> {
        (start..start + n)
            .map(|t| HistPoint {
                t_realtime_ns: t * SEC as i64,
                mean: (t % 50) as f32,
                min: 0.0,
                max: 50.0,
            })
            .collect()
    }

    fn processes(pids: &[u32], version: u64) -> ProcessSnapshot {
        ProcessSnapshot {
            header: header(version),
            records: pids.iter().map(|&p| record(p)).collect(),
            coverage: Coverage {
                pss_uss: pids.iter().take(3).map(|&p| key(p)).collect(),
            },
            extras: pids.iter().map(|&p| extras(p)).collect(),
            apps: Arc::from([AppCgroupSample {
                group: GroupKey(1),
                anon_bytes: 1 << 20,
                swap_bytes: 0,
                io_read_bps: 0.0,
                io_write_bps: 0.0,
                rios: 1,
                wios: 2,
                pids: 3,
                frozen: false,
                cgroup: CgroupRef {
                    rel_path: Arc::from("user.slice/app-a.scope"),
                    cgroup_id: 9,
                },
                validity: FieldValidity::all(Validity::Valid),
            }]),
            extras_p2: pids.iter().map(|&p| extras_p2(p)).collect(),
            low_memory: None,
        }
    }

    /// The table for `p`: a group row, then one row per process, in key order or reversed.
    fn table(p: &ProcessSnapshot, reversed: bool, version: u64) -> ProcessTableView {
        let mut rows = vec![TableRow {
            node: NodeId::Group(GroupKey(1)),
            depth: 0,
            expanded: true,
            has_children: true,
            record: None,
            group: Some(0),
            descendants: p.records.len() as u32,
            subtree: None,
        }];
        let mut procs: Vec<TableRow> = p
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| TableRow {
                node: NodeId::Process(r.key),
                depth: 1,
                expanded: false,
                has_children: false,
                record: Some(i as u32),
                group: None,
                descendants: 0,
                subtree: None,
            })
            .collect();
        if reversed {
            procs.reverse();
        }
        rows.extend(procs);
        ProcessTableView {
            header: header(version),
            query_generation: 1,
            rows: rows.into(),
            groups: Arc::from([group_row(1)]),
            subtree: Arc::from([]),
            totals: Default::default(),
            total: p.records.len() as u32,
            shown: p.records.len() as u32,
            restricted: 0,
        }
    }

    /// A snapshot with `n` processes and every kind of part filled.
    fn fixture(n: u32) -> Snapshot {
        let pids: Vec<u32> = (1..=n).collect();
        let p = processes(&pids, 1);
        let mut s = Snapshot::empty(BOOT, 3);
        s.sequence = 1;
        s.published_ns = 5 * SEC;
        s.system = Arc::new(SystemSnapshot {
            header: header(1),
            boot_time_s: 1_700_000_000,
            ..SystemSnapshot::default()
        });
        s.table = Arc::new(table(&p, false, 1));
        s.processes = Arc::new(p);
        s.series = Arc::new(SeriesSet {
            header: header(1),
            series: (1..=4).map(|m| view(sid(m), 0, 62)).collect(),
            slow: (1..=2)
                .map(|m| {
                    view(
                        SeriesId {
                            device: DeviceIdx(0),
                            metric: MetricId(m).with_tier_1hz(),
                        },
                        0,
                        100,
                    )
                })
                .collect(),
        });
        s.process_series = Arc::new(ProcessSeriesSet {
            header: header(1),
            series: Arc::from([(
                ProcessSeriesId {
                    target: GraphTarget::Process(key(1)),
                    column: ColumnId::Cpu,
                },
                view(sid(1), 0, 62),
            )]),
            slow: Arc::from([(
                ProcessSeriesId {
                    target: GraphTarget::Process(key(1)),
                    column: ColumnId::Cpu,
                },
                view(sid(1), 0, 100),
            )]),
        });
        s.device_history = Arc::new(DeviceHistory {
            header: header(1),
            request: Some(DeviceHistoryRequest {
                range: DeviceRange::H1,
                end_realtime_ns: None,
            }),
            realtime_offset_ns: 1_000,
            since_realtime_ns: Some(0),
            step_ns: SEC,
            series: (1..=3).map(|m| (sid(m), points(0, 300))).collect(),
            boots: Arc::from([0i64]),
            timeline: Some(Arc::new(DeviceTimeline::default())),
        });
        s.services = Arc::new(ServicesSnapshot {
            header: header(1),
            units: Arc::from([unit_record("a.service"), unit_record("b.service")]),
            backend: ServiceBackend::Systemd,
            ..ServicesSnapshot::default()
        });
        s.details = Arc::new(DetailsSnapshot {
            header: header(1),
            affinity: (1..=4)
                .map(|p| {
                    (
                        key(p),
                        AffinityInfo {
                            mask: CpuSet([p as u64; 16]),
                            cpuset_limit: CpuSet::default(),
                        },
                    )
                })
                .collect::<HashMap<_, _>>(),
            ..DetailsSnapshot::default()
        });
        s
    }

    /// Sends `frame` through the session encoding and back.
    fn wire(frame: &SnapshotFrame) -> SnapshotFrame {
        let mut bytes = Vec::new();
        encode_message(frame, &mut bytes).unwrap();
        let raw: RawFrame = read_frame(&mut &bytes[..]).unwrap();
        decode_message(&raw).unwrap()
    }

    /// Every part of `got` equals `want`'s, and the scalars are `want`'s.
    fn assert_same(got: &Snapshot, want: &Snapshot) {
        macro_rules! parts {
            ($($f:ident),*) => {
                $(assert!(*got.$f == *want.$f, "part {} differs", stringify!($f));)*
            };
        }
        parts!(
            system,
            series,
            processes,
            table,
            connections,
            services,
            startup,
            inventory,
            devices,
            health,
            windows,
            details,
            history,
            process_series,
            device_details,
            device_history
        );
        assert_eq!(
            (got.epoch, got.boot_id, got.published_ns),
            (want.epoch, want.boot_id, want.published_ns)
        );
    }

    /// Encodes `new` against `base`, sends it through the wire and applies it.
    fn round_trip(base: &Snapshot, new: &Snapshot) -> (SnapshotFrame, Snapshot) {
        let frame = wire(&encode(Some((4, base)), new, 5));
        let got = apply(base, 4, &frame, 99).unwrap();
        assert_eq!(got.sequence, 99);
        assert_same(&got, new);
        (frame, got)
    }

    // ---- Edits ----

    fn edit_processes(s: &mut Snapshot, f: impl FnOnce(&mut Vec<u32>, &mut ProcessSnapshot)) {
        let mut p = (*s.processes).clone();
        let mut pids: Vec<u32> = p.records.iter().map(|r| r.key.pid).collect();
        f(&mut pids, &mut p);
        p.header.version += 1;
        s.table = Arc::new(table(&p, false, s.table.header.version + 1));
        s.processes = Arc::new(p);
    }

    fn rebuild(p: &mut ProcessSnapshot, pids: &[u32]) {
        let old = p.clone();
        let find = |pid: u32| old.records.iter().position(|r| r.key.pid == pid);
        p.records = pids
            .iter()
            .map(|&q| find(q).map_or_else(|| record(q), |i| old.records[i].clone()))
            .collect();
        p.extras = pids
            .iter()
            .map(|&q| find(q).map_or_else(|| extras(q), |i| old.extras[i].clone()))
            .collect();
        p.extras_p2 = pids
            .iter()
            .map(|&q| find(q).map_or_else(|| extras_p2(q), |i| old.extras_p2[i].clone()))
            .collect();
    }

    fn push_samples(v: &Arc<SeriesView>, n: u64, cap: usize) -> Arc<SeriesView> {
        let last = v.samples.last().map_or(0, |s| s.t_ns / SEC + 1);
        let mut all: Vec<Sample> = v.samples.to_vec();
        all.extend(samples(last, n).iter().copied());
        let from = all.len().saturating_sub(cap);
        Arc::new(SeriesView {
            id: v.id,
            version: v.version + n,
            period_ns: v.period_ns,
            samples: Arc::from(&all[from..]),
        })
    }

    /// One random edit of `s`, chosen by `kind`, with `a` and `b` as its numbers.
    fn edit(s: &mut Snapshot, kind: u8, a: u32, b: u32) {
        match kind % 12 {
            0 => edit_processes(s, |pids, p| {
                let pid = 1000 + a % 500;
                if !pids.contains(&pid) {
                    pids.push(pid);
                    pids.sort_unstable();
                }
                rebuild(p, pids);
            }),
            1 => edit_processes(s, |pids, p| {
                if !pids.is_empty() {
                    pids.remove(a as usize % pids.len());
                }
                rebuild(p, pids);
            }),
            2 => edit_processes(s, |_, p| {
                let mut records = p.records.to_vec();
                if let Some(r) = records.get_mut(a as usize % records.len().max(1)) {
                    r.cpu_pct = b as f32 / 7.0;
                    r.rss_anon_bytes = b as u64;
                    r.state = ProcState::Running;
                }
                p.records = records.into();
            }),
            3 => edit_processes(s, |_, p| {
                let mut records = p.records.to_vec();
                if let Some(r) = records.get_mut(a as usize % records.len().max(1)) {
                    r.meta = meta(b);
                }
                p.records = records.into();
            }),
            4 => edit_processes(s, |_, p| {
                let mut ex = p.extras.to_vec();
                if let Some(e) = ex.get_mut(a as usize % ex.len().max(1)) {
                    e.page_faults_delta = b as u64;
                }
                p.extras = ex.into();
                let mut p2 = p.extras_p2.to_vec();
                if let Some(e) = p2.get_mut(b as usize % p2.len().max(1)) {
                    e.power_w = Some(a as f32);
                }
                p.extras_p2 = p2.into();
            }),
            5 => {
                let mut set = (*s.series).clone();
                let n = 1 + (b % 70) as u64;
                set.series = set.series.iter().map(|v| push_samples(v, n, 62)).collect();
                set.slow = set
                    .slow
                    .iter()
                    .map(|v| push_samples(v, n / 2, 100))
                    .collect();
                set.header.version += 1;
                s.series = Arc::new(set);
            }
            6 => {
                // Reset: a new ring with other samples.
                let mut set = (*s.series).clone();
                let mut views = set.series.to_vec();
                let i = a as usize % views.len();
                views[i] = view(views[i].id, 10_000 + b as u64 % 50, 5);
                set.series = views.into();
                s.series = Arc::new(set);
            }
            7 => {
                let mut t = (*s.table).clone();
                let mut rows = t.rows.to_vec();
                rows[1..].reverse();
                t.rows = rows.into();
                s.table = Arc::new(t);
            }
            8 => {
                let mut h = (*s.device_history).clone();
                let n = 1 + (b % 20) as i64;
                h.series = h
                    .series
                    .iter()
                    .map(|(id, pts)| {
                        let last = pts.last().map_or(0, |p| p.t_realtime_ns / SEC as i64 + 1);
                        let mut all = pts.to_vec();
                        all.extend(points(last, n).iter().copied());
                        (*id, Arc::from(&all[n as usize..]))
                    })
                    .collect();
                h.header.version += 1;
                s.device_history = Arc::new(h);
            }
            9 => {
                let mut h = (*s.device_history).clone();
                h.step_ns = SEC * (1 + (a % 3) as u64);
                s.device_history = Arc::new(h);
            }
            10 => {
                let mut ps = (*s.process_series).clone();
                ps.series = ps
                    .series
                    .iter()
                    .map(|(k, v)| (k.clone(), push_samples(v, 1 + (a % 5) as u64, 62)))
                    .collect();
                if b % 2 == 0 {
                    ps.slow = Arc::from([]);
                }
                s.process_series = Arc::new(ps);
            }
            _ => {
                let mut sys = (*s.system).clone();
                sys.boot_time_s += a as i64;
                sys.header.version += 1;
                s.system = Arc::new(sys);
                s.published_ns += SEC;
            }
        }
    }

    // ---- Tests ----

    #[test]
    fn snapshot_round_trips_through_postcard() {
        let s = fixture(40);
        let mut bytes = Vec::new();
        encode_message(&s, &mut bytes).unwrap();
        let back: Snapshot = decode_message(&read_frame(&mut &bytes[..]).unwrap()).unwrap();
        assert_same(&back, &s);
        let frame = wire(&encode(None, &s, 1));
        assert_same(
            &apply(&Snapshot::empty(BootId(1), 0), 0, &frame, 1).unwrap(),
            &s,
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn round_trip_random_edits(
            n in 0u32..40,
            rounds in proptest::collection::vec(
                proptest::collection::vec((any::<u8>(), any::<u32>(), any::<u32>()), 0..6),
                1..5,
            ),
        ) {
            let mut enc = DeltaEncoder::default();
            let mut client = Snapshot::empty(BootId(1), 0);
            let mut installed = 0;
            let mut cur = fixture(n);
            for (round, edits) in rounds.iter().enumerate() {
                for &(kind, a, b) in edits {
                    edit(&mut cur, kind, a, b);
                }
                let built = enc.next_frame(&cur, round as u64 * SEC).unwrap().unwrap();
                prop_assert!(!built.trimmed);
                prop_assert_eq!(built.frame.base, if round == 0 { 0 } else { installed });
                let frame = wire(&built.frame);
                client = apply(&client, installed, &frame, round as u64 + 1).unwrap();
                installed = frame.frame;
                enc.on_ack(frame.frame).unwrap();
                assert_same(&client, &cur);
            }
        }
    }

    #[test]
    fn round_trip_fixture_pairs() {
        let base = fixture(1500);
        let mut new = base.clone();
        // 200 processes change their CPU, 5 end and 5 start, every series grows by one sample
        // and every history series by one point.
        edit_processes(&mut new, |pids, p| {
            let mut records = p.records.to_vec();
            for r in records.iter_mut().step_by(7).take(200) {
                r.cpu_pct += 1.0;
            }
            p.records = records.into();
            pids.retain(|&q| !(100..105).contains(&q));
            pids.extend(2000..2005);
            rebuild(p, pids);
        });
        edit(&mut new, 5, 0, 0);
        edit(&mut new, 8, 0, 0);
        edit(&mut new, 10, 0, 1);
        let (frame, _) = round_trip(&base, &new);
        assert!(matches!(frame.processes, Some(Part::Delta(_))));
        assert!(matches!(frame.table, Some(Part::Delta(_))));
        assert!(matches!(frame.series, Some(Part::Delta(_))));
        assert!(matches!(frame.process_series, Some(Part::Delta(_))));
        assert!(matches!(frame.device_history, Some(Part::Delta(_))));
        assert!(frame.services.is_none() && frame.details.is_none());
        let Some(Part::Delta(p)) = &frame.processes else {
            unreachable!()
        };
        assert_eq!(p.rows.removed.len(), 5);
        let added = p.rows.updates.iter();
        assert_eq!(
            added.filter(|(_, u)| matches!(u, Part::Whole(_))).count(),
            5
        );
        // A delta of this size is a small part of the complete frame. The table rows after the
        // removed processes change their record index, so most of the table goes again.
        let delta = encoded_len(&frame).unwrap();
        let complete = encoded_len(&encode(None, &new, 6)).unwrap();
        assert!(
            delta * 2 < complete,
            "delta {delta} bytes, complete {complete}"
        );
    }

    #[test]
    fn series_eviction_sends_tail() {
        let base = fixture(3);
        let mut new = base.clone();
        let mut set = (*base.series).clone();
        set.series = set.series.iter().map(|v| push_samples(v, 3, 62)).collect();
        new.series = Arc::new(set);
        let (frame, _) = round_trip(&base, &new);
        let Some(Part::Delta(d)) = &frame.series else {
            panic!("series delta expected")
        };
        assert_eq!(d.series.updates.len(), 4);
        assert!(d.slow.updates.is_empty() && d.series.removed.is_empty());
        for (_, u) in &d.series.updates {
            let Part::Delta(t) = u else {
                panic!("tail expected")
            };
            assert_eq!((t.samples.skip, t.samples.keep), (3, 59));
            assert_eq!(t.samples.appended.len(), 3);
        }
    }

    #[test]
    fn series_reset_sends_whole() {
        let base = fixture(3);
        let mut new = base.clone();
        edit(&mut new, 6, 1, 0);
        let (frame, _) = round_trip(&base, &new);
        let Some(Part::Delta(d)) = &frame.series else {
            panic!("series delta expected")
        };
        assert_eq!(d.series.updates.len(), 1);
        assert!(matches!(d.series.updates[0].1, Part::Whole(_)));

        // Pushing more samples than the ring holds leaves no base sample: whole too.
        let mut new = base.clone();
        let mut set = (*base.series).clone();
        set.series = set.series.iter().map(|v| push_samples(v, 70, 62)).collect();
        new.series = Arc::new(set);
        let (frame, _) = round_trip(&base, &new);
        let Some(Part::Delta(d)) = &frame.series else {
            panic!("series delta expected")
        };
        assert!(
            d.series
                .updates
                .iter()
                .all(|(_, u)| matches!(u, Part::Whole(_)))
        );
    }

    #[test]
    fn table_reorder_only_sends_order() {
        let base = fixture(20);
        let mut new = base.clone();
        edit(&mut new, 7, 0, 0);
        let (frame, _) = round_trip(&base, &new);
        let Some(Part::Delta(t)) = &frame.table else {
            panic!("table delta expected")
        };
        assert!(t.rows.is_empty());
        assert_eq!(t.order.as_ref().map(|o| o.len()), Some(21));
        assert!(t.groups.is_none() && t.subtree.is_none());
    }

    #[test]
    fn device_history_appends_points() {
        let base = fixture(3);
        let mut new = base.clone();
        edit(&mut new, 8, 0, 2);
        let (frame, _) = round_trip(&base, &new);
        let Some(Part::Delta(d)) = &frame.device_history else {
            panic!("history delta expected")
        };
        assert!(d.boots.is_none() && d.timeline.is_none());
        for (_, u) in &d.series.updates {
            let Part::Delta(t) = u else {
                panic!("tail expected")
            };
            assert_eq!((t.skip, t.keep, t.appended.len()), (3, 297, 3));
        }
    }

    #[test]
    fn device_history_gap_points_keep_tail() {
        let gap = |t: i64| HistPoint {
            t_realtime_ns: t,
            mean: f32::NAN,
            min: f32::NAN,
            max: f32::NAN,
        };
        let base: Arc<[HistPoint]> = Arc::from([gap(1), gap(2)]);
        let new: Arc<[HistPoint]> = Arc::from([gap(1), gap(2), gap(3)]);
        let Ok(Part::Delta(t)) = hist_tail(&base, &new) else {
            panic!("tail expected")
        };
        assert_eq!((t.skip, t.keep, t.appended.len()), (0, 2, 1));
        let back = apply_tail(&base, &t).unwrap();
        assert!(back.iter().zip(new.iter()).all(|(a, b)| same_point(a, b)));
    }

    #[test]
    fn device_history_decimation_goes_whole() {
        let base = fixture(3);
        let mut new = base.clone();
        let mut h = (*base.device_history).clone();
        let mut pts = h.series[1].1.to_vec();
        pts[10].mean += 1.0;
        let mut series = h.series.to_vec();
        series[1].1 = pts.into();
        h.series = series.into();
        new.device_history = Arc::new(h);
        let (frame, _) = round_trip(&base, &new);
        assert!(matches!(frame.device_history, Some(Part::Whole(_))));

        // A new value for the base's last point is sent again, not whole.
        let mut h = (*base.device_history).clone();
        let mut pts = h.series[1].1.to_vec();
        pts.last_mut().unwrap().mean += 1.0;
        let mut series = h.series.to_vec();
        series[1].1 = pts.into();
        h.series = series.into();
        new.device_history = Arc::new(h);
        let (frame, _) = round_trip(&base, &new);
        assert!(matches!(frame.device_history, Some(Part::Delta(_))));
    }

    #[test]
    fn device_history_request_change_goes_whole() {
        let base = fixture(3);
        for change in 0..3 {
            let mut h = (*base.device_history).clone();
            match change {
                0 => h.request = None,
                1 => h.step_ns *= 2,
                _ => h.realtime_offset_ns += 1,
            }
            let mut new = base.clone();
            new.device_history = Arc::new(h);
            let (frame, _) = round_trip(&base, &new);
            assert!(matches!(frame.device_history, Some(Part::Whole(_))));
        }
    }

    #[test]
    fn oversize_frame_drops_process_series_and_device_history() {
        let s = fixture(200);
        let full = encoded_len(&encode(None, &s, 1)).unwrap();
        let mut trimmed = encode(None, &s, 1);
        trimmed.process_series = None;
        trimmed.device_history = None;
        let small = encoded_len(&trimmed).unwrap();
        assert!(
            small * 3 > full * 2,
            "the rest must outweigh the dropped parts"
        );

        let mut enc = DeltaEncoder::new((small + full) / 2);
        let first = enc.next_frame(&s, 0).unwrap().unwrap();
        assert!(first.trimmed);
        assert!(first.frame.process_series.is_none() && first.frame.device_history.is_none());
        let client = apply(&Snapshot::empty(BOOT, 0), 0, &wire(&first.frame), 1).unwrap();
        assert_eq!(*client.process_series, ProcessSeriesSet::default());
        assert_eq!(*client.device_history, DeviceHistory::default());
        enc.on_ack(1).unwrap();

        let second = enc.next_frame(&s, SEC).unwrap().unwrap();
        assert!(!second.trimmed);
        assert_eq!(second.frame.base, 1);
        assert!(second.frame.process_series.is_some() && second.frame.device_history.is_some());
        let client = apply(&client, 1, &wire(&second.frame), 2).unwrap();
        assert_same(&client, &s);
    }

    #[test]
    fn oversize_without_the_dropped_parts_is_an_error() {
        let s = fixture(50);
        let mut enc = DeltaEncoder::new(64);
        assert!(matches!(
            enc.next_frame(&s, 0),
            Err(DeltaError::TooLarge { max: 64, .. })
        ));
        assert_eq!(enc.in_flight(), None);
    }

    #[test]
    fn one_frame_in_flight() {
        let s = fixture(5);
        let mut enc = DeltaEncoder::default();
        let first = enc.next_frame(&s, 0).unwrap().unwrap();
        assert_eq!((first.frame.frame, first.frame.base), (1, 0));
        assert_eq!(enc.in_flight(), Some(1));
        assert!(enc.next_frame(&s, SEC).unwrap().is_none());
        assert!(matches!(
            enc.on_ack(2),
            Err(DeltaError::UnknownAck { frame: 2 })
        ));
        assert_eq!(enc.in_flight(), Some(1));
        enc.on_ack(1).unwrap();
        assert!(matches!(
            enc.on_ack(1),
            Err(DeltaError::UnknownAck { frame: 1 })
        ));

        // Frames built while one was in flight never become a base: the next is against 1.
        let mut newer = s.clone();
        edit(&mut newer, 11, 3, 0);
        let second = enc.next_frame(&newer, 2 * SEC).unwrap().unwrap();
        assert_eq!((second.frame.frame, second.frame.base), (2, 1));
        assert!(second.frame.system.is_some() && second.frame.processes.is_none());
    }

    #[test]
    fn complete_first_and_every_60s() {
        let s = fixture(5);
        let mut enc = DeltaEncoder::default();
        let mut bases = Vec::new();
        for t in [0, SEC, 59 * SEC, 60 * SEC, 61 * SEC] {
            let built = enc.next_frame(&s, t).unwrap().unwrap();
            bases.push(built.frame.base);
            enc.on_ack(built.frame.frame).unwrap();
        }
        assert_eq!(bases, [0, 1, 2, 0, 4]);

        enc.force_complete();
        let built = enc.next_frame(&s, 62 * SEC).unwrap().unwrap();
        assert_eq!(built.frame.base, 0);
        assert!(built.frame.health.is_some() && built.frame.services.is_some());
        enc.on_ack(built.frame.frame).unwrap();
        let built = enc.next_frame(&s, 63 * SEC).unwrap().unwrap();
        assert_eq!(built.frame.base, 6);
    }

    #[test]
    fn apply_rejects_wrong_base() {
        let base = fixture(5);
        let frame = encode(Some((4, &base)), &base, 5);
        assert!(matches!(
            apply(&base, 3, &frame, 1),
            Err(DeltaError::WrongBase {
                base: 4,
                installed: 3
            })
        ));
        // A complete frame fits any installed snapshot.
        let complete = encode(None, &base, 6);
        let got = apply(&Snapshot::empty(BootId(9), 1), 3, &complete, 2).unwrap();
        assert_same(&got, &base);
    }

    #[test]
    fn apply_rejects_bad_deltas() {
        let base = fixture(5);
        let mut new = base.clone();
        edit(&mut new, 1, 0, 0);
        edit(&mut new, 5, 0, 3);
        let frame = encode(Some((4, &base)), &new, 5);

        let mut bad = frame.clone();
        if let Some(Part::Delta(p)) = &mut bad.processes {
            p.rows.removed.push(key(9999));
        }
        assert!(matches!(
            apply(&base, 4, &bad, 1),
            Err(DeltaError::Malformed(_))
        ));

        let mut bad = frame.clone();
        if let Some(Part::Delta(d)) = &mut bad.series
            && let Some((_, Part::Delta(t))) = d.series.updates.first_mut()
        {
            t.samples.keep = 1000;
        }
        assert!(matches!(
            apply(&base, 4, &bad, 1),
            Err(DeltaError::Malformed(_))
        ));

        let mut bad = frame;
        if let Some(Part::Delta(d)) = &mut bad.series {
            d.series.updates.reverse();
        }
        assert!(matches!(
            apply(&base, 4, &bad, 1),
            Err(DeltaError::Malformed(_))
        ));
    }

    #[test]
    fn component_versions_copied() {
        let base = fixture(5);
        let mut new = base.clone();
        // Only the headers change: same rows, newer versions.
        let mut p = (*base.processes).clone();
        p.header.version = 40;
        new.processes = Arc::new(p);
        let mut t = (*base.table).clone();
        t.header.version = 41;
        new.table = Arc::new(t);
        let mut s = (*base.series).clone();
        s.header.version = 42;
        new.series = Arc::new(s);
        let mut h = (*base.device_history).clone();
        h.header.version = 43;
        new.device_history = Arc::new(h);
        let (_, got) = round_trip(&base, &new);
        assert_eq!(got.processes.header.version, 40);
        assert_eq!(got.table.header.version, 41);
        assert_eq!(got.series.header.version, 42);
        assert_eq!(got.device_history.header.version, 43);
        assert_eq!(got.system.header, base.system.header);
        assert!(
            Arc::ptr_eq(&got.system, &base.system),
            "unsent parts reuse the installed Arc"
        );
    }
}
