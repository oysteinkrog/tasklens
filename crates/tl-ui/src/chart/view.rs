//! `ChartView`: the entity behind one chart (ADR-009, UI-PRF-001).

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{Context, Entity, IntoElement, Render, SharedString, Subscription, Window};
use smallvec::SmallVec;
use tl_model::ids::{ProcessSeriesId, SeriesId};
use tl_model::series::{Sample, SampleFlags, SeriesView};
use tl_model::snapshot::{
    ComponentMask, DeviceHistory, HistPoint, ProcessSeriesSet, SeriesSet, Snapshot,
};
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
    /// Glow behind the newest 10% and a dot on each line's newest point (UI-PRF-058).
    /// `with_newest_marks`.
    pub(crate) newest_marks: bool,
    /// The chart keeps its resolved series and stops (`freeze`): no new snapshot is read, the
    /// right edge stays at the newest valid sample and the chart is never Stale.
    pub(crate) frozen: bool,
    /// The chart reads the device metric history for ranges over 10 minutes
    /// (`with_device_history`, the Overview charts).
    device_history: bool,
    /// Set while the chart reads the device metric history ([`reads_history`]): the series are
    /// on CLOCK_REALTIME and the chart is drawn against that clock.
    pub(crate) history: Option<HistoryClock>,
    /// While `history` is set: the CLOCK_REALTIME start of every boot inside the served range,
    /// drawn as boot markers (UI-PRF-055). Empty otherwise.
    pub(crate) boots: Arc<[i64]>,
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
    /// Boot markers of the last paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_boot_marks: std::cell::Cell<u32>,
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
            newest_marks: false,
            frozen: false,
            device_history: false,
            history: None,
            boots: Arc::from([]),
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
            #[cfg(any(test, feature = "test-hooks"))]
            last_boot_marks: std::cell::Cell::new(0),
        };
        this.resolve(hub.read(cx).current());
        this
    }

    /// Turns on the newest-point glow and dot (UI-PRF-058): Overview boxes, device pages and
    /// sidebar cards. Charts under `grid::MARKS_MIN_HEIGHT` tall still draw none.
    #[must_use]
    pub fn with_newest_marks(mut self) -> Self {
        self.newest_marks = true;
        self
    }

    /// Makes ranges over 10 minutes read `Snapshot.device_history` (ADR-017, UI-PRF-055): the
    /// Overview charts. Other charts keep the live series at every range.
    #[must_use]
    pub fn with_device_history(mut self, snap: &Snapshot) -> Self {
        self.device_history = true;
        self.resolve(snap);
        self
    }

    /// Whether this chart reads the device metric history now.
    fn reads_device_history(&self) -> bool {
        self.device_history && reads_history(self.spec.range_ns)
    }

    /// Notifies only when one of this chart's series changed, or when a device change may have
    /// brought an absent series in (ADR-009: views notify only for what they draw). A frozen
    /// chart reads no snapshot.
    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        if self.frozen {
            return;
        }
        let range_ns = self.spec.range_ns;
        if self.reads_device_history() {
            // History series change only with the device history component.
            if ev.mask.contains(ComponentMask::DEVICE_HISTORY) {
                self.resolve(hub.read(cx).current());
                cx.notify();
            }
            return;
        }
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
    ///
    /// With `with_device_history`, ranges over 10 minutes read `snap.device_history` instead
    /// ([`reads_history`]): each series becomes a view on CLOCK_REALTIME, and process series are
    /// absent.
    pub fn resolve(&mut self, snap: &Snapshot) {
        let set = &*snap.series;
        let range_ns = self.spec.range_ns;
        if self.reads_device_history() {
            let hist = &*snap.device_history;
            self.history = Some(HistoryClock::of(hist));
            self.boots = Arc::clone(&hist.boots);
            self.tier_fallback = false;
            let stack = self.spec.stack;
            self.series.clear();
            self.series.extend(self.spec.series.iter().map(|s| {
                let points = hist.get(s.id).filter(|_| s.process.is_none())?;
                Some(Arc::new(history_view(s.id, points, hist, stack)))
            }));
            return;
        }
        self.history = None;
        if !self.boots.is_empty() {
            self.boots = Arc::from([]);
        }
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
    /// A frozen chart is never Stale.
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
        // History points are not live samples: their age says nothing about the source.
        if !self.frozen
            && self.history.is_none()
            && geometry::is_stale(newest_ns, now_boot_ns, period_ns)
        {
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

    /// The cached stroke paths' smallest vertex x, chart-local physical px.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn stroke_left_x(&self) -> Option<f32> {
        self.cache
            .borrow()
            .strokes
            .iter()
            .flat_map(|s| s.path.vertices.iter())
            .map(|v| f32::from(v.xy_position.x))
            .reduce(f32::min)
    }

    /// Fill and stroke sets in the geometry cache, stale sets excluded.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cached_fill_stroke_counts(&self) -> (usize, usize) {
        let c = self.cache.borrow();
        (c.fills.len(), c.strokes.len())
    }

    /// Replaces the spec. When it changed, resolves it against the hub's current snapshot and
    /// notifies. A frozen chart keeps its series.
    pub fn set_spec(&mut self, spec: ChartSpec, cx: &mut Context<Self>) {
        if self.spec != spec {
            self.spec = spec;
            if !self.frozen {
                let hub = self.hub.clone();
                self.resolve(hub.read(cx).current());
            }
            cx.notify();
        }
    }

    /// Stops the chart at its last samples for good (PERF-086: an ended process card). It keeps
    /// the series it has, reads no new snapshot, pins its right edge at the newest valid sample
    /// and is never Stale, so it neither slides nor fades. The owner may then drop the series
    /// from demand.
    pub fn freeze(&mut self, cx: &mut Context<Self>) {
        if !self.frozen {
            self.frozen = true;
            cx.notify();
        }
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen
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

/// Ranges longer than this read the device metric history (ADR-017, UI-PRF-055): the live
/// series hold at most 10 minutes (ADR-003). Only the Overview asks for such ranges.
pub const HISTORY_MIN_RANGE_NS: u64 = 600_000_000_000;

/// Whether a chart of `range_ns` made `with_device_history` reads `Snapshot.device_history`.
pub const fn reads_history(range_ns: u64) -> bool {
    range_ns > HISTORY_MIN_RANGE_NS
}

/// The clock of a chart that reads the device metric history: its series are on
/// CLOCK_REALTIME, so the paint turns the boot clock into that clock with `offset_ns`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryClock {
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME of the current boot.
    pub offset_ns: i64,
    /// The right edge while the history request is paused at a time (UI-PRF-057); `None` is
    /// now.
    pub end_ns: Option<u64>,
}

impl HistoryClock {
    fn of(hist: &DeviceHistory) -> Self {
        Self {
            offset_ns: hist.realtime_offset_ns,
            end_ns: hist
                .request
                .and_then(|r| r.end_realtime_ns)
                .map(|t| t.max(0) as u64),
        }
    }

    /// The time at the right edge, on CLOCK_REALTIME, when the boot clock reads `now_boot_ns`.
    pub fn right_edge_ns(&self, now_boot_ns: u64) -> u64 {
        self.end_ns
            .unwrap_or_else(|| now_boot_ns.saturating_add_signed(self.offset_ns))
    }
}

/// `points` as a series view on CLOCK_REALTIME (UI-PRF-055). A gap point is a gap sample, and a
/// point more than one and a half steps after the one before it starts a new run, so a stretch
/// without points (the app not running, the machine off or asleep) draws as a break, not as a
/// line across it. A valid point is its max a sixth of a step after the bucket start, its min a
/// third in (each only when it differs from the mean), then its mean at the bucket middle, so a
/// spike shorter than the step stays visible and the newest sample is still a mean. With `stack`
/// a valid point is its mean only: the stacked bands sum each series' samples per period slot,
/// so extrema would add a series to itself, and the maxima of different series need not happen
/// together (the bands could top the total). The version is the component's, so a new publish
/// rebuilds the geometry.
fn history_view(
    id: SeriesId,
    points: &[HistPoint],
    hist: &DeviceHistory,
    stack: bool,
) -> SeriesView {
    let step = hist.step_ns.max(1);
    let mut samples = Vec::with_capacity(points.len());
    let mut prev: Option<i64> = None;
    for p in points {
        let Ok(t) = u64::try_from(p.t_realtime_ns) else {
            continue;
        };
        let first = samples.len();
        if p.is_gap() {
            samples.push(Sample::gap(t));
        } else {
            if !stack && p.max > p.mean {
                samples.push(Sample::valid(t + step / 6, p.max));
            }
            if !stack && p.min < p.mean {
                samples.push(Sample::valid(t + step / 3, p.min));
            }
            samples.push(Sample::valid(t + step / 2, p.mean));
        }
        if prev.is_some_and(|q| p.t_realtime_ns.saturating_sub(q) as u64 > step + step / 2) {
            samples[first].flags |= SampleFlags::GAP_BEFORE;
        }
        prev = Some(p.t_realtime_ns);
    }
    SeriesView {
        id,
        version: hist.header.version,
        period_ns: step,
        samples: samples.into(),
    }
}

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
    use crate::chart::ChartSeries;

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

    /// A stacked history chart (the Memory composition) draws each point's mean only. With the
    /// max and min as well, a band added its own extrema into one period slot, so the bands
    /// above it rose and the composition could top the total.
    #[test]
    fn stacked_history_draws_means_only() {
        const STEP: u64 = 10_000_000_000;
        let hist = DeviceHistory {
            step_ns: STEP,
            ..DeviceHistory::default()
        };
        let t0 = 1_000 * STEP as i64;
        let points: Vec<HistPoint> = (0..6i64)
            .map(|i| HistPoint {
                t_realtime_ns: t0 + i * STEP as i64,
                mean: 20.0,
                min: 10.0,
                max: 30.0,
            })
            .collect();
        let views: Vec<SeriesView> = [
            MetricId::MEM_USED,
            MetricId::MEM_CACHED,
            MetricId::MEM_AVAILABLE,
        ]
        .map(|metric| {
            let id = SeriesId {
                device: DeviceIdx(1),
                metric,
            };
            history_view(id, &points, &hist, true)
        })
        .into();
        for v in &views {
            assert_eq!(v.samples.len(), points.len(), "one sample per point");
        }
        // Unstacked, the same points keep their max and min.
        let id = views[0].id;
        assert_eq!(history_view(id, &points, &hist, false).samples.len(), 18);

        let line = gpui_kit::hsla(0.0, 1.0, 0.5, 1.0);
        let series: Vec<(ChartSeries, Option<&SeriesView>)> = views
            .iter()
            .map(|v| {
                let spec = ChartSeries {
                    line,
                    ..ChartSeries::default()
                };
                (spec, Some(v))
            })
            .collect();
        let newest = views[0].samples.last().expect("samples").t_ns;
        let input = geometry::BuildInput {
            series: &series,
            map: geometry::TimeMap {
                anchor_ns: newest,
                right_edge_ns: newest,
                range_ns: 6 * STEP,
                plot_width: 600.0,
            },
            plot_height: 100.0,
            y_min: 0.0,
            y_max: 100.0,
            now_boot_ns: newest,
            stack: true,
            curved: false,
        };
        let mut cache = ChartCache::default();
        geometry::build(&input, &mut cache, &mut geometry::Scratch::default());
        // The three means sum to 60 of 100, so the top band's line sits at y 40.
        let top = cache
            .strokes
            .iter()
            .flat_map(|s| s.path.vertices.iter())
            .map(|v| f32::from(v.xy_position.y))
            .reduce(f32::min)
            .expect("strokes");
        assert!(
            (top - 40.0).abs() < 0.5,
            "the bands top out at y {top}, not 40"
        );
    }
}
