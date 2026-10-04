//! Samples, series rings and published series views (ADR-003).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::SeriesId;
use crate::time::TimeNs;

bitflags::bitflags! {
    /// Per-sample flags. The chart never joins across a gap, reset or stale publish (PERF-022).
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
    pub struct SampleFlags: u32 {
        /// Value is meaningful. If clear, the sample marks a gap.
        const VALID      = 1 << 0;
        /// Do not connect to the previous sample.
        const GAP_BEFORE = 1 << 1;
        /// Counter baseline was reset at this sample.
        const RESET      = 1 << 2;
        /// Published while the source was stale.
        const STALE      = 1 << 3;
    }
}

/// One graph point: 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub t_ns: TimeNs,
    pub value: f32,
    pub flags: SampleFlags,
}

impl Sample {
    /// A sample with a meaningful value (flags = VALID).
    pub const fn valid(t_ns: TimeNs, value: f32) -> Sample {
        Sample {
            t_ns,
            value,
            flags: SampleFlags::VALID,
        }
    }

    /// A gap marker: value 0.0, no flags (not VALID).
    pub const fn gap(t_ns: TimeNs) -> Sample {
        Sample {
            t_ns,
            value: 0.0,
            flags: SampleFlags::empty(),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.flags.contains(SampleFlags::VALID)
    }
}

/// Fast-tier window in nanoseconds (60 s).
const FAST_WINDOW_NS: u64 = 60_000_000_000;

/// Engine-owned ring. Not shared with the UI.
pub struct SeriesRing {
    samples: Box<[Sample]>,
    /// Slot the next push writes.
    next: u32,
    len: u32,
    version: u64,
    /// Set by `mark_gap`; the next push ORs GAP_BEFORE into its sample.
    gap_pending: bool,
}

impl SeriesRing {
    /// Fast-tier capacity for a sampling period: ceil(60 s / period_ns) + 2.
    /// 100 ms gives 602, 1 s gives 62. Panics if period_ns == 0.
    pub const fn fast_capacity(period_ns: u64) -> u32 {
        assert!(period_ns != 0, "period_ns must not be 0");
        FAST_WINDOW_NS.div_ceil(period_ns) as u32 + 2
    }

    /// Ring with room for `capacity` samples (minimum 2; smaller values are raised to 2).
    pub fn with_capacity(capacity: u32) -> SeriesRing {
        let capacity = capacity.max(2) as usize;
        SeriesRing {
            samples: vec![Sample::gap(0); capacity].into_boxed_slice(),
            next: 0,
            len: 0,
            version: 0,
            gap_pending: false,
        }
    }

    /// Appends, overwriting the oldest sample when full. Increments version by 1.
    pub fn push(&mut self, mut s: Sample) {
        if self.gap_pending {
            s.flags |= SampleFlags::GAP_BEFORE;
            self.gap_pending = false;
        }
        let cap = self.samples.len() as u32;
        self.samples[self.next as usize] = s;
        self.next = if self.next + 1 == cap {
            0
        } else {
            self.next + 1
        };
        if self.len < cap {
            self.len += 1;
        }
        self.version += 1;
    }

    /// Sets GAP_BEFORE on the next pushed sample (used after suspend, rate change, restart).
    pub fn mark_gap(&mut self) {
        self.gap_pending = true;
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn last(&self) -> Option<Sample> {
        if self.len == 0 {
            return None;
        }
        let i = if self.next == 0 {
            self.samples.len() - 1
        } else {
            self.next as usize - 1
        };
        Some(self.samples[i])
    }

    /// Oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = Sample> + '_ {
        let (head, tail) = self.chronological_parts();
        head.iter().chain(tail).copied()
    }

    /// Chronological copy for publishing. One allocation of `len` samples.
    pub fn to_view(&self, id: SeriesId, period_ns: u64) -> SeriesView {
        let (head, tail) = self.chronological_parts();
        let mut samples = Vec::with_capacity(self.len());
        samples.extend_from_slice(head);
        samples.extend_from_slice(tail);
        SeriesView {
            id,
            version: self.version,
            period_ns,
            samples: Arc::from(samples),
        }
    }

    /// The stored samples as two slices, oldest first.
    fn chronological_parts(&self) -> (&[Sample], &[Sample]) {
        let len = self.len as usize;
        let next = self.next as usize;
        if len < self.samples.len() {
            // Not wrapped yet: samples live in 0..len.
            (&self.samples[..len], &[])
        } else {
            (&self.samples[next..], &self.samples[..next])
        }
    }
}

/// Capacity of the 1 Hz tier: 10 min + 2.
pub const SLOW_TIER_CAPACITY: u32 = 602;

const NS_PER_SEC: u64 = 1_000_000_000;

/// Flags a 1 Hz bucket keeps from its folded samples (VALID is handled apart).
const BUCKET_OR_FLAGS: SampleFlags = SampleFlags::GAP_BEFORE
    .union(SampleFlags::RESET)
    .union(SampleFlags::STALE);

/// Fast ring plus the 1 Hz tier built from it. Each 1 Hz sample is the
/// per-second maximum of the fast tier (ADR-012).
pub struct TieredRing {
    pub fast: SeriesRing,
    /// Capacity SLOW_TIER_CAPACITY.
    pub slow: SeriesRing,
    /// `t_ns / 1_000_000_000` of the open bucket.
    bucket_sec: Option<u64>,
    /// Running maximum of the open second.
    bucket: Option<Sample>,
}

impl TieredRing {
    /// Fast capacity = `SeriesRing::fast_capacity(period_ns)`; slow capacity = 602.
    pub fn new(period_ns: u64) -> TieredRing {
        TieredRing {
            fast: SeriesRing::with_capacity(SeriesRing::fast_capacity(period_ns)),
            slow: SeriesRing::with_capacity(SLOW_TIER_CAPACITY),
            bucket_sec: None,
            bucket: None,
        }
    }

    /// Pushes into the fast ring and folds the sample into the open 1 Hz bucket.
    /// A sample from an earlier second than the open bucket goes to the fast ring only.
    pub fn push(&mut self, s: Sample) {
        self.fast.push(s);
        let sec = s.t_ns / NS_PER_SEC;
        match self.bucket_sec {
            Some(open) if sec < open => return,
            Some(open) if sec > open => {
                if let Some(b) = self.bucket.take() {
                    self.slow.push(b);
                }
                self.bucket_sec = Some(sec);
            }
            // mark_gap already closed this second; do not emit it twice.
            Some(_) if self.bucket.is_none() => return,
            Some(_) => {}
            None => self.bucket_sec = Some(sec),
        }
        let b = self.bucket.get_or_insert(Sample::gap(sec * NS_PER_SEC));
        b.flags |= s.flags & BUCKET_OR_FLAGS;
        if s.is_valid() {
            b.value = if b.is_valid() {
                b.value.max(s.value)
            } else {
                s.value
            };
            b.flags |= SampleFlags::VALID;
        }
    }

    /// Closes the open 1 Hz bucket, then calls `mark_gap` on both tiers.
    /// The gap lies after the open bucket, so that bucket does not get GAP_BEFORE.
    /// Later samples from the closed second go to the fast ring only.
    pub fn mark_gap(&mut self) {
        if let Some(b) = self.bucket.take() {
            self.slow.push(b);
        }
        self.fast.mark_gap();
        self.slow.mark_gap();
    }
}

/// Published, immutable, chronological copy of a ring.
#[derive(Clone, Debug)]
pub struct SeriesView {
    pub id: SeriesId,
    /// Bumps on every append.
    pub version: u64,
    /// Current nominal sampling period.
    pub period_ns: u64,
    pub samples: Arc<[Sample]>,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::ids::{DeviceIdx, MetricId};

    fn s(t: u64) -> Sample {
        Sample::valid(t, t as f32)
    }

    #[test]
    fn fast_capacity_values() {
        assert_eq!(SeriesRing::fast_capacity(100_000_000), 602);
        assert_eq!(SeriesRing::fast_capacity(250_000_000), 242);
        assert_eq!(SeriesRing::fast_capacity(1_000_000_000), 62);
        assert_eq!(SeriesRing::fast_capacity(7_000_000_000), 11);
    }

    #[test]
    #[should_panic]
    fn fast_capacity_zero_panics() {
        let _ = SeriesRing::fast_capacity(std::hint::black_box(0));
    }

    #[test]
    fn ring_wraps_oldest_first() {
        let mut r = SeriesRing::with_capacity(4);
        for t in 1..=6 {
            r.push(s(t));
        }
        let ts: Vec<u64> = r.iter().map(|x| x.t_ns).collect();
        assert_eq!(ts, vec![3, 4, 5, 6]);
        assert_eq!(r.len(), 4);
        assert_eq!(r.last(), Some(s(6)));
    }

    #[test]
    fn ring_version_counts_pushes() {
        let mut r = SeriesRing::with_capacity(4);
        assert!(r.is_empty());
        assert_eq!(r.last(), None);
        r.push(s(1));
        assert_eq!(r.version(), 1);
        assert_eq!(r.last(), Some(s(1)));
        for t in 2..=10 {
            r.push(s(t));
        }
        assert_eq!(r.version(), 10);
    }

    #[test]
    fn mark_gap_sets_once() {
        let mut r = SeriesRing::with_capacity(8);
        r.push(s(1));
        r.mark_gap();
        r.mark_gap();
        r.push(s(2));
        r.push(s(3));
        let flags: Vec<bool> = r
            .iter()
            .map(|x| x.flags.contains(SampleFlags::GAP_BEFORE))
            .collect();
        assert_eq!(flags, vec![false, true, false]);
        assert!(r.iter().all(|x| x.is_valid()));
    }

    #[test]
    fn view_is_chronological_copy() {
        let mut r = SeriesRing::with_capacity(2);
        for t in 1..=3 {
            r.push(s(t));
        }
        let mut r3 = SeriesRing::with_capacity(5);
        for t in 1..=3 {
            r3.push(s(t));
        }
        let id = SeriesId {
            device: DeviceIdx(1),
            metric: MetricId(2),
        };
        let v = r3.to_view(id, 500_000_000);
        assert_eq!(&*v.samples, &[s(1), s(2), s(3)]);
        assert_eq!(v.version, r3.version());
        assert_eq!(v.id, id);
        assert_eq!(v.period_ns, 500_000_000);

        // Wrapped ring is also oldest first.
        let w = r.to_view(id, 1);
        assert_eq!(&*w.samples, &[s(2), s(3)]);
    }

    #[test]
    fn min_capacity_two() {
        assert_eq!(SeriesRing::with_capacity(0).capacity(), 2);
        assert_eq!(SeriesRing::with_capacity(1).capacity(), 2);
        assert_eq!(SeriesRing::with_capacity(3).capacity(), 3);
    }

    #[test]
    fn gap_sample_is_not_valid() {
        assert!(!Sample::gap(5).is_valid());
        assert!(Sample::valid(5, 1.0).is_valid());
    }

    proptest! {
        #[test]
        fn ring_matches_vec_model(cap in 2u32..=700, pushes in proptest::collection::vec(any::<u64>(), 0..=2000)) {
            let mut r = SeriesRing::with_capacity(cap);
            let mut model = Vec::new();
            for &t in &pushes {
                r.push(s(t));
                model.push(s(t));
            }
            let start = model.len().saturating_sub(cap as usize);
            let got: Vec<Sample> = r.iter().collect();
            prop_assert_eq!(&got[..], &model[start..]);
            prop_assert_eq!(r.len(), model.len() - start);
            prop_assert_eq!(r.version(), pushes.len() as u64);
            prop_assert_eq!(r.last(), model.last().copied());
        }
    }
}

#[cfg(test)]
mod tier_tests {
    use super::*;

    const SEC: u64 = 1_000_000_000;

    fn slow(r: &TieredRing) -> Vec<Sample> {
        r.slow.iter().collect()
    }

    #[test]
    fn slow_tier_per_second_max() {
        let mut r = TieredRing::new(100_000_000);
        for i in 0..10u64 {
            r.push(Sample::valid(5 * SEC + i * 100_000_000, (i + 1) as f32));
        }
        assert!(r.slow.is_empty());
        r.push(Sample::valid(6 * SEC, 1.0));
        assert_eq!(slow(&r), vec![Sample::valid(5 * SEC, 10.0)]);
        assert_eq!(r.fast.len(), 11);
    }

    #[test]
    fn slow_tier_all_gap_bucket_invalid() {
        let mut r = TieredRing::new(100_000_000);
        for i in 0..10u64 {
            r.push(Sample::gap(5 * SEC + i * 100_000_000));
        }
        r.push(Sample::valid(6 * SEC, 1.0));
        let got = slow(&r);
        assert_eq!(got.len(), 1);
        assert!(!got[0].is_valid());
        assert_eq!(got[0].value, 0.0);
        assert_eq!(got[0].t_ns, 5 * SEC);
    }

    #[test]
    fn slow_tier_flags_ored() {
        let mut r = TieredRing::new(100_000_000);
        for i in 0..10u64 {
            let mut s = Sample::valid(5 * SEC + i * 100_000_000, 1.0);
            if i == 4 {
                s.flags |= SampleFlags::GAP_BEFORE;
            }
            if i == 7 {
                s.flags |= SampleFlags::STALE;
            }
            r.push(s);
        }
        r.push(Sample::valid(6 * SEC, 1.0));
        let got = slow(&r);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].flags,
            SampleFlags::VALID | SampleFlags::GAP_BEFORE | SampleFlags::STALE
        );
        assert!(!got[0].flags.contains(SampleFlags::RESET));
    }

    #[test]
    fn slow_tier_ignores_invalid_values_in_max() {
        let mut r = TieredRing::new(100_000_000);
        let mut bad = Sample::gap(5 * SEC);
        bad.value = 99.0;
        r.push(bad);
        r.push(Sample::valid(5 * SEC + 100_000_000, 3.0));
        r.push(Sample::valid(6 * SEC, 1.0));
        assert_eq!(slow(&r), vec![Sample::valid(5 * SEC, 3.0)]);
    }

    #[test]
    fn slow_tier_capacities() {
        let r = TieredRing::new(100_000_000);
        assert_eq!(r.fast.capacity(), 602);
        assert_eq!(r.slow.capacity(), 602);
        let r = TieredRing::new(SEC);
        assert_eq!(r.fast.capacity(), 62);
        assert_eq!(r.slow.capacity(), SLOW_TIER_CAPACITY as usize);
    }

    #[test]
    fn slow_tier_no_fill_for_skipped_seconds() {
        let mut r = TieredRing::new(100_000_000);
        r.push(Sample::valid(5 * SEC, 2.0));
        r.push(Sample::valid(9 * SEC, 4.0));
        assert_eq!(slow(&r), vec![Sample::valid(5 * SEC, 2.0)]);
        r.push(Sample::valid(10 * SEC, 1.0));
        assert_eq!(
            slow(&r),
            vec![Sample::valid(5 * SEC, 2.0), Sample::valid(9 * SEC, 4.0)]
        );
    }

    #[test]
    fn slow_tier_clock_back_goes_to_fast_only() {
        let mut r = TieredRing::new(100_000_000);
        r.push(Sample::valid(5 * SEC, 2.0));
        r.push(Sample::valid(4 * SEC, 50.0));
        r.push(Sample::valid(6 * SEC, 1.0));
        assert_eq!(r.fast.len(), 3);
        assert_eq!(slow(&r), vec![Sample::valid(5 * SEC, 2.0)]);
    }

    #[test]
    fn slow_tier_mark_gap_both() {
        let mut r = TieredRing::new(100_000_000);
        r.mark_gap();
        r.push(Sample::valid(5 * SEC, 1.0));
        r.push(Sample::valid(5 * SEC + 100_000_000, 2.0));
        r.push(Sample::valid(6 * SEC, 3.0));
        r.push(Sample::valid(7 * SEC, 4.0));
        let fast: Vec<bool> = r
            .fast
            .iter()
            .map(|x| x.flags.contains(SampleFlags::GAP_BEFORE))
            .collect();
        assert_eq!(fast, vec![true, false, false, false]);
        let slow: Vec<bool> = r
            .slow
            .iter()
            .map(|x| x.flags.contains(SampleFlags::GAP_BEFORE))
            .collect();
        assert_eq!(slow, vec![true, false]);
    }

    #[test]
    fn slow_tier_mark_gap_mid_bucket() {
        let mut r = TieredRing::new(100_000_000);
        r.push(Sample::valid(5 * SEC, 1.0));
        r.push(Sample::valid(5 * SEC + 100_000_000, 2.0));
        r.mark_gap();
        assert_eq!(slow(&r), vec![Sample::valid(5 * SEC, 2.0)]);
        // A late sample from the closed second does not emit second 5 again.
        r.push(Sample::valid(5 * SEC + 900_000_000, 9.0));
        r.push(Sample::valid(7 * SEC, 3.0));
        r.push(Sample::valid(7 * SEC + 100_000_000, 4.0));
        r.push(Sample::valid(8 * SEC, 5.0));
        let mut gap7 = Sample::valid(7 * SEC, 4.0);
        gap7.flags |= SampleFlags::GAP_BEFORE;
        assert_eq!(slow(&r), vec![Sample::valid(5 * SEC, 2.0), gap7]);
        let fast_gap: Vec<bool> = r
            .fast
            .iter()
            .map(|x| x.flags.contains(SampleFlags::GAP_BEFORE))
            .collect();
        assert_eq!(fast_gap, vec![false, false, true, false, false, false]);
    }
}
