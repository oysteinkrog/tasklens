//! Graph series store: the fast and 1 Hz rings and the published `SeriesSet` (ADR-003, ADR-008).
//!
//! The reducer owns two `SeriesRing`s per series and publishes immutable `SeriesView`s. The 1 Hz
//! tier holds the per-second maximum of the fast tier (ADR-012) and serves the 5 and 10 minute
//! ranges. A view is
//! cached until its series changes, so an unchanged series keeps the same `Arc` from one
//! `SeriesSet` to the next (PERF-017). The chart never joins across `GAP_BEFORE`, a non-valid
//! sample or a stale publish (PERF-022).

use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::ids::SeriesId;
use tl_model::series::{Sample, SampleFlags, SeriesRing, SeriesView};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

/// A series is stale after max(3 × period, 1.5 s) without a sample (ADR-008).
const STALE_MIN_NS: u64 = 1_500_000_000;
const STALE_PERIODS: u64 = 3;

/// 1 Hz tier capacity: 10 min + 2.
pub const SLOW_CAPACITY: u32 = 602;
pub const SLOW_PERIOD_NS: u64 = 1_000_000_000;

pub struct SeriesStore {
    rings: FxHashMap<SeriesId, Entry>,
    /// Set by `mark_gap_all`; a series created before the next `publish` also starts with
    /// `GAP_BEFORE`.
    pending_gap: bool,
    /// The last published set, returned again when nothing changed.
    current: Option<Arc<SeriesSet>>,
}

struct Entry {
    ring: SeriesRing,
    period_ns: u64,
    /// Cached until the next append or stale change. `None` means the series changed.
    view: Option<Arc<SeriesView>>,
    /// The next appended sample gets `GAP_BEFORE`.
    gap_next: bool,
    /// The newest sample is older than the stale threshold. The view shows it with `STALE`.
    stale: bool,
    /// Stale changes so far. Added to the ring version so a stale change gives the view a new
    /// version and the chart rebuilds its geometry.
    stale_changes: u64,
    /// 1 Hz tier: one sample per closed second.
    slow: SeriesRing,
    /// Cached like `view`. `None` means the slow tier changed.
    slow_view: Option<Arc<SeriesView>>,
    /// A second is open in the bucket fields below.
    bucket_open: bool,
    /// Start of the open second.
    bucket_start_ns: TimeNs,
    /// Maximum over the open second's valid samples. `None` gives a gap sample.
    bucket_max: Option<f32>,
    /// OR of the open second's sample flags, without `VALID`.
    bucket_flags: SampleFlags,
}

impl Entry {
    fn new(period_ns: u64, gap_next: bool) -> Entry {
        Entry {
            ring: SeriesRing::with_capacity(SeriesStore::fast_capacity(period_ns)),
            period_ns,
            view: None,
            gap_next,
            stale: false,
            stale_changes: 0,
            slow: SeriesRing::with_capacity(SLOW_CAPACITY),
            slow_view: None,
            bucket_open: false,
            bucket_start_ns: 0,
            bucket_max: None,
            bucket_flags: SampleFlags::empty(),
        }
    }

    /// Folds a fast sample into the open second. A sample in a later second first closes the
    /// open one into `slow`. A sample from an earlier second is left out of the 1 Hz tier.
    fn fold(&mut self, s: Sample) {
        let start = s.t_ns - s.t_ns % SLOW_PERIOD_NS;
        if self.bucket_open {
            if start < self.bucket_start_ns {
                return;
            }
            if start > self.bucket_start_ns {
                self.close_bucket();
            }
        }
        if !self.bucket_open {
            self.bucket_open = true;
            self.bucket_start_ns = start;
            self.bucket_max = None;
            self.bucket_flags = SampleFlags::empty();
        }
        self.bucket_flags |= s.flags - SampleFlags::VALID;
        if s.is_valid() {
            self.bucket_max = Some(self.bucket_max.map_or(s.value, |m| m.max(s.value)));
        }
    }

    fn close_bucket(&mut self) {
        let sample = match self.bucket_max {
            Some(value) => Sample {
                t_ns: self.bucket_start_ns,
                value,
                flags: self.bucket_flags | SampleFlags::VALID,
            },
            None => Sample {
                t_ns: self.bucket_start_ns,
                value: 0.0,
                flags: self.bucket_flags,
            },
        };
        self.slow.push(sample);
        self.slow_view = None;
        self.bucket_open = false;
    }

    fn build_view(&self, id: SeriesId) -> SeriesView {
        self.stale_view(self.ring.to_view(id, self.period_ns))
    }

    fn build_slow_view(&self, id: SeriesId) -> SeriesView {
        let slow_id = SeriesId {
            device: id.device,
            metric: id.metric.with_tier_1hz(),
        };
        self.stale_view(self.slow.to_view(slow_id, SLOW_PERIOD_NS))
    }

    /// Adds the stale changes to the version and `STALE` to the newest sample while stale.
    fn stale_view(&self, mut view: SeriesView) -> SeriesView {
        view.version += self.stale_changes;
        if self.stale
            && let Some(last) = view.samples.last().copied()
        {
            let mut samples = view.samples.to_vec();
            if let Some(s) = samples.last_mut() {
                s.flags = last.flags | SampleFlags::STALE;
            }
            view.samples = Arc::from(samples);
        }
        view
    }
}

impl Default for SeriesStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SeriesStore {
    pub fn new() -> SeriesStore {
        SeriesStore {
            rings: FxHashMap::default(),
            pending_gap: false,
            current: None,
        }
    }

    /// Fast-tier capacity: ceil(60 s / period_ns) + 2. 100 ms gives 602, 1 s gives 62.
    pub fn fast_capacity(period_ns: u64) -> u32 {
        SeriesRing::fast_capacity(period_ns)
    }

    /// Appends; creates the ring on first use; a period change sets `gap_next` and keeps the
    /// ring. Flags without `VALID` append a gap sample (value 0.0).
    pub fn append(
        &mut self,
        id: SeriesId,
        t_ns: TimeNs,
        value: f32,
        period_ns: u64,
        flags: SampleFlags,
    ) {
        let pending_gap = self.pending_gap;
        let entry = self
            .rings
            .entry(id)
            .or_insert_with(|| Entry::new(period_ns, pending_gap));
        if entry.period_ns != period_ns {
            entry.period_ns = period_ns;
            entry.gap_next = true;
        }
        if entry.stale {
            // The stale run ends here; do not join the line across it.
            entry.stale = false;
            entry.stale_changes += 1;
            entry.gap_next = true;
        }
        let mut s = Sample { t_ns, value, flags };
        if !s.is_valid() {
            s.value = 0.0;
        }
        if entry.gap_next {
            s.flags |= SampleFlags::GAP_BEFORE;
            entry.gap_next = false;
        }
        entry.ring.push(s);
        entry.view = None;
        entry.fold(s);
    }

    /// Appends a gap sample (`VALID` clear) at the series' current period. Does nothing for a
    /// series that has no ring yet, since it has no period.
    pub fn append_gap(&mut self, id: SeriesId, t_ns: TimeNs) {
        if let Some(period_ns) = self.rings.get(&id).map(|e| e.period_ns) {
            self.append(id, t_ns, 0.0, period_ns, SampleFlags::empty());
        }
    }

    /// The next sample of every series gets `GAP_BEFORE` (suspend, resume, session restart).
    pub fn mark_gap_all(&mut self) {
        self.pending_gap = true;
        for entry in self.rings.values_mut() {
            entry.gap_next = true;
        }
    }

    /// Marks a series stale when its newest sample is older than max(3 × period, 1.5 s). Its
    /// view is rebuilt with `STALE` on the newest sample.
    pub fn mark_stale(&mut self, now_ns: TimeNs) {
        for entry in self.rings.values_mut() {
            if entry.stale {
                continue;
            }
            let Some(last) = entry.ring.last() else {
                continue;
            };
            let threshold = STALE_MIN_NS.max(STALE_PERIODS.saturating_mul(entry.period_ns));
            if now_ns.saturating_sub(last.t_ns) > threshold {
                entry.stale = true;
                entry.stale_changes += 1;
                entry.view = None;
                entry.slow_view = None;
            }
        }
    }

    /// Series whose view the next `publish` rebuilds, sorted by id.
    pub fn changed(&self) -> SmallVec<[SeriesId; 64]> {
        let mut ids: SmallVec<[SeriesId; 64]> = self
            .rings
            .iter()
            .filter(|(_, e)| e.view.is_none() || e.slow_view.is_none())
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Builds both tiers sorted by `SeriesId`. Unchanged series reuse their cached views; when
    /// no series changed, the previous set is returned as is. The header's observed range comes
    /// from the fast tier.
    pub fn publish(&mut self) -> Arc<SeriesSet> {
        self.pending_gap = false;
        let any_changed = self
            .rings
            .values()
            .any(|e| e.view.is_none() || e.slow_view.is_none());
        if !any_changed && let Some(current) = &self.current {
            return Arc::clone(current);
        }

        let mut views: Vec<Arc<SeriesView>> = Vec::with_capacity(self.rings.len());
        let mut slow_views: Vec<Arc<SeriesView>> = Vec::with_capacity(self.rings.len());
        let mut observed_start = TimeNs::MAX;
        let mut observed_end: TimeNs = 0;
        for (&id, entry) in &mut self.rings {
            if entry.view.is_none() {
                entry.view = Some(Arc::new(entry.build_view(id)));
            }
            let Some(view) = &entry.view else {
                unreachable!("view was just built");
            };
            if let (Some(first), Some(last)) = (view.samples.first(), view.samples.last()) {
                observed_start = observed_start.min(first.t_ns);
                observed_end = observed_end.max(last.t_ns);
            }
            views.push(Arc::clone(view));
            if entry.slow_view.is_none() {
                entry.slow_view = Some(Arc::new(entry.build_slow_view(id)));
            }
            if let Some(slow_view) = &entry.slow_view {
                slow_views.push(Arc::clone(slow_view));
            }
        }
        views.sort_unstable_by_key(|v| v.id);
        slow_views.sort_unstable_by_key(|v| v.id);

        let prev_version = self.current.as_ref().map_or(0, |s| s.header.version);
        let header = if observed_end == 0 {
            ComponentHeader {
                version: prev_version + 1,
                ..ComponentHeader::default()
            }
        } else {
            ComponentHeader {
                version: prev_version + 1,
                observed_start,
                observed_end,
                validity: Validity::Valid,
            }
        };
        let set = Arc::new(SeriesSet {
            header,
            series: Arc::from(views),
            slow: Arc::from(slow_views),
        });
        self.current = Some(Arc::clone(&set));
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::ids::{DeviceIdx, MetricId};

    const MS: u64 = 1_000_000;
    const SEC: u64 = 1_000_000_000;

    fn sid(device: u16, metric: MetricId) -> SeriesId {
        SeriesId {
            device: DeviceIdx(device),
            metric,
        }
    }

    fn view(set: &SeriesSet, id: SeriesId) -> &Arc<SeriesView> {
        set.get(id).expect("series published")
    }

    #[test]
    fn capacity_rule() {
        assert_eq!(SeriesStore::fast_capacity(100 * MS), 602);
        assert_eq!(SeriesStore::fast_capacity(SEC), 62);
    }

    #[test]
    fn ring_keeps_newest_602() {
        let id = sid(0, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        for i in 0..700u64 {
            store.append(id, i * 100 * MS, i as f32, 100 * MS, SampleFlags::VALID);
        }
        let set = store.publish();
        let v = view(&set, id);
        assert_eq!(v.samples.len(), 602);
        let ts: Vec<u64> = v.samples.iter().map(|s| s.t_ns).collect();
        let want: Vec<u64> = (98..700u64).map(|i| i * 100 * MS).collect();
        assert_eq!(ts, want);
        assert_eq!(v.version, 700);
        assert_eq!(v.period_ns, 100 * MS);
    }

    #[test]
    fn rate_change_marks_gap_before() {
        let id = sid(0, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        for i in 0..5u64 {
            store.append(id, i * 100 * MS, 1.0, 100 * MS, SampleFlags::VALID);
        }
        store.append(id, 2 * SEC, 2.0, SEC, SampleFlags::VALID);
        store.append(id, 3 * SEC, 3.0, SEC, SampleFlags::VALID);
        let set = store.publish();
        let v = view(&set, id);
        assert_eq!(v.samples.len(), 7);
        assert_eq!(v.period_ns, SEC);
        let gaps: Vec<bool> = v
            .samples
            .iter()
            .map(|s| s.flags.contains(SampleFlags::GAP_BEFORE))
            .collect();
        assert_eq!(gaps, vec![false, false, false, false, false, true, false]);
        assert!(v.samples.iter().all(|s| s.is_valid()));
    }

    #[test]
    fn unchanged_series_reuse_arc() {
        let a = sid(0, MetricId::CPU_TOTAL);
        let b = sid(1, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        store.append(b, 0, 1.0, 100 * MS, SampleFlags::VALID);
        store.append(a, 0, 1.0, 100 * MS, SampleFlags::VALID);
        let first = store.publish();
        assert_eq!(
            first.series.iter().map(|v| v.id).collect::<Vec<_>>(),
            vec![a, b]
        );
        assert!(store.changed().is_empty());

        store.append(a, 100 * MS, 2.0, 100 * MS, SampleFlags::VALID);
        assert_eq!(store.changed().as_slice(), &[a]);
        let second = store.publish();
        assert!(Arc::ptr_eq(view(&first, b), view(&second, b)));
        assert!(!Arc::ptr_eq(view(&first, a), view(&second, a)));
        assert!(second.header.version > first.header.version);

        // Nothing changed: the same set comes back.
        let third = store.publish();
        assert!(Arc::ptr_eq(&second, &third));
    }

    #[test]
    fn gap_all_then_append() {
        let a = sid(0, MetricId::CPU_TOTAL);
        let b = sid(1, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        for id in [a, b] {
            store.append(id, 0, 1.0, 100 * MS, SampleFlags::VALID);
        }
        store.publish();
        store.mark_gap_all();
        for id in [a, b] {
            store.append(id, 5 * SEC, 2.0, 100 * MS, SampleFlags::VALID);
        }
        let set = store.publish();
        for id in [a, b] {
            let v = view(&set, id);
            assert!(v.samples[1].flags.contains(SampleFlags::GAP_BEFORE));
            assert!(!v.samples[0].flags.contains(SampleFlags::GAP_BEFORE));
        }

        // A gap append clears VALID and zeroes the value.
        store.append_gap(a, 6 * SEC);
        let set = store.publish();
        let last = *view(&set, a).samples.last().unwrap();
        assert!(!last.is_valid());
        assert_eq!(last.value, 0.0);
        assert!(!last.flags.contains(SampleFlags::GAP_BEFORE));
    }

    #[test]
    fn stale_threshold() {
        let slow = sid(0, MetricId::CPU_TOTAL);
        let fast = sid(1, MetricId::CPU_TOTAL);
        let now = 100 * SEC;
        let mut store = SeriesStore::new();
        store.append(slow, now - 4 * SEC, 1.0, SEC, SampleFlags::VALID);
        store.append(fast, now - 1400 * MS, 1.0, 100 * MS, SampleFlags::VALID);
        let before = store.publish();

        store.mark_stale(now);
        assert_eq!(store.changed().as_slice(), &[slow]);
        let after = store.publish();
        let v = view(&after, slow);
        assert!(v.samples.last().unwrap().flags.contains(SampleFlags::STALE));
        assert!(v.version > view(&before, slow).version);
        assert!(Arc::ptr_eq(view(&before, fast), view(&after, fast)));
        assert!(
            !view(&after, fast)
                .samples
                .last()
                .unwrap()
                .flags
                .contains(SampleFlags::STALE)
        );

        // Marking again does not rebuild.
        store.mark_stale(now);
        assert!(store.changed().is_empty());

        // The next sample ends the stale run with a gap and no STALE flag.
        store.append(slow, now + 2 * SEC, 2.0, SEC, SampleFlags::VALID);
        let set = store.publish();
        let samples = &view(&set, slow).samples;
        assert_eq!(samples.len(), 2);
        assert!(!samples[0].flags.contains(SampleFlags::STALE));
        assert!(samples[1].flags.contains(SampleFlags::GAP_BEFORE));
    }

    fn slow_id(id: SeriesId) -> SeriesId {
        SeriesId {
            device: id.device,
            metric: id.metric.with_tier_1hz(),
        }
    }

    #[test]
    fn slow_tier_per_second_max() {
        let id = sid(0, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        // Peak in the middle of the second, so the max is not just the last value.
        for (i, v) in [1, 2, 3, 4, 10, 5, 6, 7, 8, 9].into_iter().enumerate() {
            store.append(
                id,
                5 * SEC + i as u64 * 100 * MS,
                v as f32,
                100 * MS,
                SampleFlags::VALID,
            );
        }
        let set = store.publish();
        assert!(view(&set, slow_id(id)).samples.is_empty());

        store.append(id, 6 * SEC, 1.0, 100 * MS, SampleFlags::VALID);
        let set = store.publish();
        let v = view(&set, slow_id(id));
        assert_eq!(v.samples.len(), 1);
        assert_eq!(v.samples[0], Sample::valid(5 * SEC, 10.0));
        assert_eq!(v.period_ns, SLOW_PERIOD_NS);
    }

    #[test]
    fn slow_tier_capacity_602() {
        let id = sid(0, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        for i in 0..700u64 {
            store.append(id, i * SEC, i as f32, SEC, SampleFlags::VALID);
        }
        let set = store.publish();
        let slow = view(&set, slow_id(id));
        assert_eq!(slow.samples.len(), SLOW_CAPACITY as usize);
        assert_eq!(view(&set, id).samples.len(), 62);
        // Second 699 is still open; the newest closed second is 698.
        assert_eq!(slow.samples.last().unwrap().t_ns, 698 * SEC);
        assert_eq!(slow.samples[0].t_ns, 97 * SEC);
    }

    #[test]
    fn slow_tier_gap_bucket() {
        let id = sid(0, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        store.append(id, 0, 3.0, 100 * MS, SampleFlags::VALID);
        store.append_gap(id, SEC);
        store.append_gap(id, SEC + 500 * MS);
        store.append(id, 2 * SEC, 4.0, 100 * MS, SampleFlags::VALID);
        store.append(id, 3 * SEC, 5.0, 100 * MS, SampleFlags::VALID);
        let set = store.publish();
        let samples = &view(&set, slow_id(id)).samples;
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0], Sample::valid(0, 3.0));
        assert!(!samples[1].is_valid());
        assert_eq!(samples[1].t_ns, SEC);
        assert_eq!(samples[1].value, 0.0);
        assert_eq!(samples[2], Sample::valid(2 * SEC, 4.0));
    }

    #[test]
    fn slow_tier_gap_before_after_mark() {
        let id = sid(0, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        store.append(id, 0, 1.0, 100 * MS, SampleFlags::VALID);
        store.append(id, SEC, 2.0, 100 * MS, SampleFlags::VALID);
        store.mark_gap_all();
        store.append(id, 10 * SEC, 3.0, 100 * MS, SampleFlags::VALID);
        store.append(id, 11 * SEC, 4.0, 100 * MS, SampleFlags::VALID);
        let set = store.publish();
        let samples = &view(&set, slow_id(id)).samples;
        let ts: Vec<u64> = samples.iter().map(|s| s.t_ns).collect();
        assert_eq!(ts, vec![0, SEC, 10 * SEC]);
        let gaps: Vec<bool> = samples
            .iter()
            .map(|s| s.flags.contains(SampleFlags::GAP_BEFORE))
            .collect();
        assert_eq!(gaps, vec![false, false, true]);
        assert!(samples.iter().all(|s| s.is_valid()));
    }

    #[test]
    fn slow_tier_arc_reuse() {
        let a = sid(0, MetricId::CPU_TOTAL);
        let b = sid(1, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        for id in [a, b] {
            store.append(id, 0, 1.0, 100 * MS, SampleFlags::VALID);
            store.append(id, SEC, 1.0, 100 * MS, SampleFlags::VALID);
        }
        let first = store.publish();
        let second = store.publish();
        assert!(Arc::ptr_eq(
            view(&first, slow_id(a)),
            view(&second, slow_id(a))
        ));

        // A fast append within the open second leaves the slow view as is.
        store.append(a, SEC + 100 * MS, 2.0, 100 * MS, SampleFlags::VALID);
        let third = store.publish();
        assert!(!Arc::ptr_eq(view(&second, a), view(&third, a)));
        assert!(Arc::ptr_eq(
            view(&second, slow_id(a)),
            view(&third, slow_id(a))
        ));
        assert!(Arc::ptr_eq(
            view(&second, slow_id(b)),
            view(&third, slow_id(b))
        ));
    }

    #[test]
    fn slow_tier_id_has_tier_bit() {
        let id = sid(2, MetricId::CPU_TOTAL);
        let mut store = SeriesStore::new();
        store.append(id, 0, 1.0, 100 * MS, SampleFlags::VALID);
        let set = store.publish();
        assert_eq!(set.slow.len(), 1);
        let slow = &set.slow[0];
        assert!(slow.id.metric.is_tier_1hz());
        assert_eq!(slow.id.metric.base(), MetricId::CPU_TOTAL);
        assert_eq!(slow.id.device, DeviceIdx(2));
        assert!(Arc::ptr_eq(view(&set, slow_id(id)), slow));
        assert!(!set.series[0].id.metric.is_tier_1hz());
    }
}
