//! `ChartView`: the entity behind one chart (ADR-009, UI-PRF-001).

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{Context, Entity, IntoElement, Render, SharedString, Subscription, Window};
use smallvec::SmallVec;
use tl_model::ids::{ProcessSeriesId, SeriesId};
use tl_model::series::SeriesView;
use tl_model::snapshot::{ComponentMask, ProcessSeriesSet, SeriesSet, Snapshot};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use super::element::ChartElement;
use super::{ChartCache, ChartLabels, ChartSpec, ChartState, geometry, hover, labels, scale};
use crate::hub::{SnapshotChanged, SnapshotHub};

pub struct ChartView {
    pub spec: ChartSpec,
    pub labels: ChartLabels,
    /// Owner-set validity. `derived_state` returns it when it is Failed, Restricted or
    /// Unsupported and derives the rest from the data.
    pub state: ChartState,
    hub: Entity<SnapshotHub>,
    _hub_sub: Subscription,
    pub(crate) cache: Rc<RefCell<ChartCache>>,
    /// Buffers reused by every geometry rebuild.
    pub(crate) scratch: geometry::Scratch,
    /// Right edge the cached geometry was built for. A stale series draws its tail up to it, so
    /// the element rebuilds when it moves while a series is stale.
    pub(crate) built_right_edge_ns: u64,
    /// Y scale memory across rebuilds (E19-004, R-262).
    pub(crate) scale_state: scale::ScaleState,
    /// Reused axis max text buffer for `scale::compute`.
    pub(crate) axis_text: String,
    /// Filled by E19-009; empty struct until then.
    #[allow(dead_code, reason = "read from E19-009 and E19-010 on")]
    pub(crate) text: labels::LabelCache,
    /// Resolved from the newest snapshot, one per `spec.series` entry.
    pub(crate) series: SmallVec<[Option<Arc<SeriesView>>; 2]>,
    /// True when the range wants the 1 Hz tier but a series resolved to its fast view because
    /// the snapshot has no 1 Hz view for it (P1 engine). The chart then draws the last 60 s.
    pub tier_fallback: bool,
    /// The value label shows the sum of every series' newest value (network download plus
    /// upload), not series 0 alone. `set_value_sum`.
    value_sum: bool,
    /// Owner-formatted value text shown in place of the formatted value while the value is
    /// Valid or Stale, such as "2.80 GiB / 15.92 GiB". `set_value_text`.
    pub(crate) value_text: Option<SharedString>,
    /// Reused formatter buffer.
    #[allow(dead_code, reason = "read from E19-009 and E19-010 on")]
    pub(crate) last_value_text: String,
    /// Hover pin and the shaped value box lines (E19-016).
    pub(crate) hover: hover::HoverState,
    /// The scale ceiling shows small in the plot's top-left corner (`set_compact_scale`).
    pub(crate) compact_scale: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    /// `geometry::build` calls.
    #[cfg(any(test, feature = "test-hooks"))]
    pub build_count: std::cell::Cell<u32>,
    /// `paint_path` calls, over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub path_paints: std::cell::Cell<u32>,
    /// Colours handed to `paint_path` by the last paint, in paint order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub path_colors: RefCell<SmallVec<[gpui_kit::Hsla; 8]>>,
    /// Right edge of the last paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_right_edge_ns: std::cell::Cell<u64>,
    /// Grid quads of the last paint (vertical and horizontal hairlines).
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_grid_quads: std::cell::Cell<u32>,
    /// Dotted baseline spans of the last paint, chart-local physical px before the paint shift.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_gap_spans: RefCell<SmallVec<[std::ops::Range<f32>; 4]>>,
}

impl ChartView {
    /// Subscribes to `hub` and resolves the spec against its current snapshot.
    pub fn new(
        spec: ChartSpec,
        labels: ChartLabels,
        hub: &Entity<SnapshotHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _hub_sub = cx.subscribe(hub, Self::on_snapshot);
        let mut this = Self {
            spec,
            labels,
            state: ChartState::WarmingUp,
            hub: hub.clone(),
            _hub_sub,
            cache: Rc::default(),
            scratch: geometry::Scratch::default(),
            built_right_edge_ns: 0,
            scale_state: scale::ScaleState::default(),
            axis_text: String::new(),
            text: labels::LabelCache::default(),
            series: SmallVec::new(),
            tier_fallback: false,
            value_sum: false,
            compact_scale: false,
            value_text: None,
            last_value_text: String::new(),
            hover: hover::HoverState::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            build_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            path_paints: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            path_colors: RefCell::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            last_right_edge_ns: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            last_grid_quads: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            last_gap_spans: RefCell::default(),
        };
        this.resolve(hub.read(cx).current());
        this
    }

    /// Notifies only when one of this chart's series changed, or when a device change may have
    /// brought an absent series in (ADR-009: views notify only for what they draw).
    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let range_ns = self.spec.range_ns;
        let own_changed = ev.mask.contains(ComponentMask::SERIES) && {
            let changed = ChangedRuns::new(&ev.changed_series);
            self.spec
                .series
                .iter()
                .filter(|s| s.process.is_none())
                .any(|s| changed.tier_changed(s.id, range_ns))
        };
        let device_appeared = ev.mask.contains(ComponentMask::DEVICES)
            && (self.series.len() != self.spec.series.len()
                || self.series.iter().any(Option::is_none));
        // `changed_series` does not list process series, so a new process series set
        // re-resolves and notifies only when one of this chart's views is a new `Arc`.
        let process_changed = ev.mask.contains(ComponentMask::PROCESS_SERIES)
            && self.spec.series.iter().any(|s| s.process.is_some());
        if own_changed || device_appeared {
            self.resolve(hub.read(cx).current());
            cx.notify();
        } else if process_changed {
            let before = self.series.clone();
            self.resolve(hub.read(cx).current());
            let same = before.len() == self.series.len()
                && before.iter().zip(&self.series).all(|(a, b)| match (a, b) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                });
            if !same {
                cx.notify();
            }
        }
    }

    /// Looks up every spec series in `snap` by binary search and keeps an `Arc` clone of each,
    /// `None` when absent. Ranges over 60 s read the 1 Hz tier (`find_tiered`). A series with
    /// `process` set reads `snap.process_series` the same way (`find_process_tiered`). Reuses
    /// the inline buffer, so it does not allocate for up to 2 series.
    pub fn resolve(&mut self, snap: &Snapshot) {
        let set = &*snap.series;
        let range_ns = self.spec.range_ns;
        let mut fallback = false;
        self.series.clear();
        self.series.extend(self.spec.series.iter().map(|s| {
            let (view, fell_back) = match &s.process {
                Some(id) => find_process_tiered(&snap.process_series, id, range_ns),
                None => find_tiered(set, s.id, range_ns),
            };
            fallback |= fell_back;
            view.cloned()
        }));
        self.tier_fallback = fallback;
    }

    /// Validity for this paint (UI-PRF-001). Owner-set Failed, Restricted and Unsupported win.
    /// Otherwise WarmingUp without samples, Stale when the newest sample is too old, else Live.
    pub fn derived_state(&self, now_boot_ns: u64) -> ChartState {
        if matches!(
            self.state,
            ChartState::Failed(_) | ChartState::Restricted | ChartState::Unsupported
        ) {
            return self.state.clone();
        }
        let mut newest_ns = None;
        let mut period_ns = u64::MAX;
        for s in self.series.iter().flatten() {
            period_ns = period_ns.min(s.period_ns);
            if let Some(last) = s.samples.last() {
                newest_ns = Some(newest_ns.unwrap_or(0).max(last.t_ns));
            }
        }
        let Some(newest_ns) = newest_ns else {
            return ChartState::WarmingUp;
        };
        if geometry::is_stale(newest_ns, now_boot_ns, period_ns) {
            ChartState::Stale {
                age_ns: geometry::stale_age_ns(newest_ns, now_boot_ns),
            }
        } else {
            ChartState::Live
        }
    }

    /// The value label's input (E19-009; owners such as the sidebar card read it too): the newest
    /// valid value of series 0 and its validity at the current boot time. `None` when the spec
    /// has no series.
    pub fn value_now(&self) -> Option<(f32, Validity)> {
        self.value_at(now_boot_ns())
    }

    /// `value_now` at `now_boot_ns`. Owner-set Failed, Restricted and Unsupported map to the
    /// same `Validity`. Otherwise `Stale` when the chart is stale, `Valid` when the newest sample
    /// of series 0 is valid, and `WarmingUp` when series 0 has no valid newest sample yet. The
    /// value is the newest valid sample of series 0, or 0.0 when there is none; only `Valid` and
    /// `Stale` values are meant to be shown.
    ///
    /// With `value_sum` the value is the sum of every series' newest valid sample, and a series
    /// without a valid newest sample gives the whole value its own state ([`sum_state`]), so a
    /// partial sum never shows as the total.
    pub fn value_at(&self, now_boot_ns: u64) -> Option<(f32, Validity)> {
        if self.spec.series.is_empty() {
            return None;
        }
        let summed = if self.value_sum {
            &self.series[..]
        } else {
            &self.series[..self.series.len().min(1)]
        };
        let value = newest_valid_sum(summed);
        let validity = match self.derived_state(now_boot_ns) {
            ChartState::Failed(_) => Validity::Failed,
            ChartState::Restricted => Validity::Restricted,
            ChartState::Unsupported => Validity::Unsupported,
            ChartState::Stale { .. } if self.value_sum && value.is_none() => sum_state(summed),
            ChartState::Stale { .. } => Validity::Stale,
            ChartState::WarmingUp => Validity::WarmingUp,
            ChartState::Live if self.value_sum => sum_state(summed),
            ChartState::Live if newest_all_valid(summed) => Validity::Valid,
            ChartState::Live => Validity::WarmingUp,
        };
        Some((value.unwrap_or(0.0), validity))
    }

    /// Makes the value label the sum of every series' newest value (see `value_at`).
    pub fn set_value_sum(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.value_sum != on {
            self.value_sum = on;
            cx.notify();
        }
    }

    /// Shows `text` as the value while the value is Valid or Stale; `None` goes back to the
    /// formatted value. The owner calls it only when the shown numbers change, never per frame.
    pub fn set_value_text(&mut self, text: Option<SharedString>, cx: &mut Context<Self>) {
        if self.value_text != text {
            self.value_text = text;
            cx.notify();
        }
    }

    /// Shows the scale ceiling ("100%", "16.00 GiB", "8 MiB/s") as one small line in the
    /// plot's top-left corner, on plots from 32 px tall (Overview boxes, TMOG). It takes the
    /// place of the axis max label.
    pub fn set_compact_scale(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.compact_scale != on {
            self.compact_scale = on;
            cx.notify();
        }
    }

    /// The shaped labels of the last paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn label_cache(&self) -> &labels::LabelCache {
        &self.text
    }

    /// The value label's text as last painted.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn painted_value_text(&self) -> Option<&str> {
        self.text.value_text()
    }

    /// The owner value text from `set_value_text`.
    pub fn value_text(&self) -> Option<&SharedString> {
        self.value_text.as_ref()
    }

    /// The cached stroke paths' smallest vertex y, chart-local physical px (0 is the plot top).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn stroke_top_y(&self) -> Option<f32> {
        self.cache
            .borrow()
            .strokes
            .iter()
            .flat_map(|s| s.path.vertices.iter())
            .map(|v| f32::from(v.xy_position.y))
            .reduce(f32::min)
    }

    /// The cached stroke paths' largest vertex x, chart-local physical px.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn stroke_right_x(&self) -> Option<f32> {
        self.cache
            .borrow()
            .strokes
            .iter()
            .flat_map(|s| s.path.vertices.iter())
            .map(|v| f32::from(v.xy_position.x))
            .reduce(f32::max)
    }

    /// Fill and stroke sets in the geometry cache, stale sets excluded.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cached_fill_stroke_counts(&self) -> (usize, usize) {
        let c = self.cache.borrow();
        (c.fills.len(), c.strokes.len())
    }

    /// Replaces the spec. When it changed, resolves it against the hub's current snapshot and
    /// notifies.
    pub fn set_spec(&mut self, spec: ChartSpec, cx: &mut Context<Self>) {
        if self.spec != spec {
            self.spec = spec;
            let hub = self.hub.clone();
            self.resolve(hub.read(cx).current());
            cx.notify();
        }
    }

    pub fn set_labels(&mut self, labels: ChartLabels, cx: &mut Context<Self>) {
        if self.labels != labels {
            self.labels = labels;
            cx.notify();
        }
    }

    /// Owner-supplied validity (Failed, Restricted, Unsupported). The view derives WarmingUp and
    /// Stale itself (E19-008).
    pub fn set_state(&mut self, state: ChartState, cx: &mut Context<Self>) {
        if self.state != state {
            self.state = state;
            cx.notify();
        }
    }

    pub fn series_ids(&self) -> impl Iterator<Item = SeriesId> + '_ {
        self.spec.series.iter().map(|s| s.id)
    }

    /// Sets the resolved series directly, one per `spec.series` entry, bypassing the hub.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_series_for_test(
        &mut self,
        series: SmallVec<[Option<Arc<SeriesView>>; 2]>,
        cx: &mut Context<Self>,
    ) {
        self.series = series;
        cx.notify();
    }

    /// The resolved series, one per `spec.series` entry.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn resolved(&self) -> &[Option<Arc<SeriesView>>] {
        &self.series
    }

    /// Number of built paths in the geometry cache (fills, strokes and their stale sets).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cached_path_count(&self) -> usize {
        let c = self.cache.borrow();
        c.fills.len() + c.strokes.len() + c.stale_fills.len() + c.stale_strokes.len()
    }

    /// True while the geometry cache still holds its default key.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cache_key_is_default(&self) -> bool {
        self.cache.borrow().key == super::ChartGeometryKey::default()
    }
}

impl Render for ChartView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        ChartElement::new(cx.entity())
    }
}

/// The sum of each series' newest valid sample. `None` when `series` is empty, or a series is
/// absent or has no valid sample.
fn newest_valid_sum(series: &[Option<Arc<SeriesView>>]) -> Option<f32> {
    if series.is_empty() {
        return None;
    }
    series.iter().try_fold(0.0, |sum, s| {
        let newest = s.as_deref()?.samples.iter().rev().find(|s| s.is_valid())?;
        Some(sum + newest.value)
    })
}

/// The state of a summed value: `Missing` ("-") when a series is absent, or its newest sample
/// is a gap after valid data (a disconnected interface); else `WarmingUp` ("…") when a series
/// has no valid sample yet; else `Valid`. Samples carry no failure reason, so `Failed` ("!")
/// comes only from the owner-set chart state, as for series 0.
fn sum_state(series: &[Option<Arc<SeriesView>>]) -> Validity {
    if series.is_empty() {
        return Validity::WarmingUp;
    }
    let mut state = Validity::Valid;
    for s in series {
        let Some(view) = s.as_deref() else {
            return Validity::Missing;
        };
        match view.samples.last() {
            Some(newest) if newest.is_valid() => {}
            _ if view.samples.iter().any(|s| s.is_valid()) => return Validity::Missing,
            _ => state = Validity::WarmingUp,
        }
    }
    state
}

/// True when `series` is not empty and every series is present with a valid newest sample.
fn newest_all_valid(series: &[Option<Arc<SeriesView>>]) -> bool {
    !series.is_empty()
        && series.iter().all(|s| {
            s.as_deref()
                .and_then(|s| s.samples.last())
                .is_some_and(|s| s.is_valid())
        })
}

/// Ranges longer than this read the 1 Hz tier (PERF-017, R-016).
pub const TIER_1HZ_MIN_RANGE_NS: u64 = 60_000_000_000;

/// `id` in `set` for a chart of `range_ns`. Over 60 s it looks up the 1 Hz view first and falls
/// back to the fast view when the snapshot has none; the bool is true when that fallback found
/// the fast view.
pub(crate) fn find_tiered(
    set: &SeriesSet,
    id: SeriesId,
    range_ns: u64,
) -> (Option<&Arc<SeriesView>>, bool) {
    if range_ns <= TIER_1HZ_MIN_RANGE_NS || id.metric.is_tier_1hz() {
        return (find_series(set, id), false);
    }
    let slow = SeriesId {
        metric: id.metric.with_tier_1hz(),
        ..id
    };
    match find_series(set, slow) {
        Some(v) => (Some(v), false),
        None => {
            let fast = find_series(set, id);
            (fast, fast.is_some())
        }
    }
}

/// The process series `id` for a chart of `range_ns`: over 60 s the 1 Hz view, falling back to
/// the fast view when the set has no 1 Hz view (the bool is then true), else the fast view.
pub(crate) fn find_process_tiered<'a>(
    set: &'a ProcessSeriesSet,
    id: &ProcessSeriesId,
    range_ns: u64,
) -> (Option<&'a Arc<SeriesView>>, bool) {
    if range_ns <= TIER_1HZ_MIN_RANGE_NS {
        return (set.get(id), false);
    }
    match set.get_slow(id) {
        Some(v) => (Some(v), false),
        None => {
            let fast = set.get(id);
            (fast, fast.is_some())
        }
    }
}

/// A `SnapshotChanged::changed_series` list split into its two sorted runs.
/// `Snapshot::changed_since` lists the fast ids, then the 1 Hz ids, so the whole list is not
/// sorted but each run is.
#[derive(Clone, Copy, Debug)]
pub struct ChangedRuns<'a> {
    pub fast: &'a [SeriesId],
    pub slow: &'a [SeriesId],
}

impl<'a> ChangedRuns<'a> {
    /// Finds the split with one binary search on the tier bit.
    pub fn new(changed: &'a [SeriesId]) -> Self {
        let split = changed.partition_point(|id| !id.metric.is_tier_1hz());
        let (fast, slow) = changed.split_at(split);
        Self { fast, slow }
    }

    /// The run that holds ids of `id`'s tier.
    pub fn run_for(&self, id: SeriesId) -> &'a [SeriesId] {
        if id.metric.is_tier_1hz() {
            self.slow
        } else {
            self.fast
        }
    }

    /// Binary search in `id`'s run: at most ⌊log₂ n⌋ + 1 comparisons.
    pub fn contains(&self, id: SeriesId) -> bool {
        search_series(self.run_for(id), |c| c.cmp(&id)).is_some()
    }

    /// True when `id` changed, or its 1 Hz view when `range_ns` reads that tier.
    pub fn tier_changed(&self, id: SeriesId, range_ns: u64) -> bool {
        self.contains(id)
            || (range_ns > TIER_1HZ_MIN_RANGE_NS
                && self.contains(SeriesId {
                    metric: id.metric.with_tier_1hz(),
                    ..id
                }))
    }
}

/// `id` in `set`: the 1 Hz tier list when the id carries the tier bit, else the fast list.
pub(crate) fn find_series(set: &SeriesSet, id: SeriesId) -> Option<&Arc<SeriesView>> {
    let views = if id.metric.is_tier_1hz() {
        &set.slow
    } else {
        &set.series
    };
    search_series(views, |v| v.id.cmp(&id))
}

/// Binary search over `views`, which is sorted by `SeriesId` (series views or plain ids). `cmp`
/// orders an item against the target. At most ⌊log₂ n⌋ + 1 comparisons (7 for 80 series); `slice::binary_search_by` always
/// takes one more. Public so a test can count the comparisons.
pub fn search_series<T>(views: &[T], mut cmp: impl FnMut(&T) -> Ordering) -> Option<&T> {
    let (mut lo, mut hi) = (0, views.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match cmp(&views[mid]) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Some(&views[mid]),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use tl_model::ids::{DeviceIdx, MetricId};
    use tl_model::series::Sample;

    use super::*;

    fn view(metric: MetricId, samples: Vec<Sample>) -> Option<Arc<SeriesView>> {
        Some(Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(1),
                metric,
            },
            version: 1,
            period_ns: 1_000_000_000,
            samples: samples.into(),
        }))
    }

    #[test]
    fn value_sum_of_newest_valid() {
        let rx = view(
            MetricId::NET_RX,
            vec![Sample::valid(1, 5.0), Sample::valid(2, 1.2)],
        );
        let tx = view(
            MetricId::NET_TX,
            vec![Sample::valid(1, 9.0), Sample::valid(2, 0.3)],
        );
        let both = [rx.clone(), tx];
        let sum = newest_valid_sum(&both).expect("sum");
        assert!((sum - 1.5).abs() < 1e-6, "sum {sum}");
        assert!(newest_all_valid(&both));
        assert_eq!(newest_valid_sum(&both[..1]), Some(1.2), "series 0 alone");
    }

    #[test]
    fn value_sum_state_per_series() {
        let rx = view(MetricId::NET_RX, vec![Sample::valid(1, 1.2)]);
        let ok = view(MetricId::NET_TX, vec![Sample::valid(1, 0.3)]);
        assert_eq!(sum_state(&[rx.clone(), ok]), Validity::Valid);
        // Absent: "-".
        assert_eq!(sum_state(&[rx.clone(), None]), Validity::Missing);
        // A gap after valid data (disconnected): "-".
        let gap = view(
            MetricId::NET_TX,
            vec![Sample::valid(1, 0.3), Sample::gap(2)],
        );
        assert_eq!(sum_state(&[rx.clone(), gap]), Validity::Missing);
        // No valid sample yet: "…".
        let new = view(MetricId::NET_TX, vec![Sample::gap(1)]);
        assert_eq!(sum_state(&[rx.clone(), new.clone()]), Validity::WarmingUp);
        assert_eq!(
            sum_state(&[rx, view(MetricId::NET_TX, Vec::new())]),
            Validity::WarmingUp
        );
        // Missing wins over warming up.
        assert_eq!(sum_state(&[new, None]), Validity::Missing);
    }

    #[test]
    fn value_sum_missing_series_is_not_partial() {
        let rx = view(MetricId::NET_RX, vec![Sample::valid(1, 1.2)]);
        // Absent series: no sum at all.
        let absent = [rx.clone(), None];
        assert_eq!(newest_valid_sum(&absent), None);
        assert!(!newest_all_valid(&absent));
        // A series whose newest sample is a gap: the sum keeps its last valid value, but the
        // newest values are not all valid.
        let tx = view(
            MetricId::NET_TX,
            vec![Sample::valid(1, 0.3), Sample::gap(2)],
        );
        let gapped = [rx.clone(), tx];
        assert!(!newest_all_valid(&gapped));
        // A series with no valid sample at all.
        let empty = [rx, view(MetricId::NET_TX, vec![Sample::gap(1)])];
        assert_eq!(newest_valid_sum(&empty), None);
        assert_eq!(newest_valid_sum(&[]), None);
        assert!(!newest_all_valid(&[]));
    }
}
