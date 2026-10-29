//! The Overview time range and the range figures (UI-PRF-055, D-030).
//!
//! `performance.overview_range` sets the range of every Overview chart; the Overview ignores
//! `graph_range_s`. 1 min and 5 min draw the live series; 1 h, 24 h and 7 d draw the device
//! metric history (ADR-017), which the page asks for with `Demand.device_history_range`
//! ([`history_request`]). The chart view picks its source from its range
//! (`chart::view::reads_history`).
//!
//! While the timeline paused the Overview at a time (UI-PRF-057, [`super::timeline::Paused`]),
//! every range is asked for at that time from the history, and the switch disables the ranges
//! the finest tier at that time cannot serve.
//!
//! [`RangeText`] is what a device box shows for the range: the "min 6%  avg 42%  peak 100%" line
//! under its title ([`figures`]) and the start of its time axis ("1 hour ago", or "since 09:14"
//! while the history does not cover the range yet, [`axis_start`]). The page works both out and
//! hands each box its text, so a box notifies only when its own text changes.

use gpui_kit::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, div,
};
use tl_model::demand::DeviceHistoryRequest;
use tl_model::ids::SeriesId;
use tl_model::series::Sample;
use tl_model::settings::OverviewRange;
use tl_model::snapshot::{DeviceHistory, HistPoint, Snapshot};

use super::timeline::{self, Paused};
use crate::chart::view::find_tiered;
use crate::components::button::{TlButton, TlButtonKind};
use crate::i18n::tr;
use crate::pages::performance::details::Template;
use crate::settings_global::TlSettings;

/// The switch labels, in [`OverviewRange::ALL`] order.
pub const SWITCH_KEYS: [&str; 5] = [
    "perf-overview-range-1m",
    "perf-overview-range-5m",
    "perf-overview-range-1h",
    "perf-overview-range-24h",
    "perf-overview-range-7d",
];

/// The time axis start labels, in [`OverviewRange::ALL`] order.
pub const AGO_KEYS: [&str; 5] = [
    "perf-overview-ago-1m",
    "perf-overview-ago-5m",
    "perf-overview-ago-1h",
    "perf-overview-ago-24h",
    "perf-overview-ago-7d",
];

/// The debug selector of the switch segment for `range` in the switch `prefix`
/// (`perf-overview-range-1h`).
pub fn segment_id(prefix: &str, range: OverviewRange) -> String {
    format!("{prefix}-{}", range.key())
}

/// The prefix of the switch in the Overview header.
pub const HEADER_SWITCH: &str = "perf-overview-range";

/// The prefix of the switch at the right end of the status bar.
pub const STATUS_SWITCH: &str = "perf-status-range";

/// `performance.overview_range`, the default before settings load.
pub fn current_range(cx: &App) -> OverviewRange {
    if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).performance.overview_range
    } else {
        OverviewRange::default()
    }
}

/// Writes `performance.overview_range`.
pub fn set_range(range: OverviewRange, cx: &mut App) {
    if cx.has_global::<TlSettings>() && current_range(cx) != range {
        TlSettings::update(cx, |d| d.settings.performance.overview_range = range);
    }
}

/// What the page asks the device metric history for: the range ending now for 1 h, 24 h and
/// 7 d, nothing for the live ranges. While `paused`, every range ends at the picked time
/// centred in the range, never after `now_realtime_ns` (UI-PRF-057).
pub fn history_request(
    range: OverviewRange,
    paused: Option<Paused>,
    now_realtime_ns: i64,
) -> Option<DeviceHistoryRequest> {
    if let Some(p) = paused {
        return Some(DeviceHistoryRequest {
            range: range.device_range(),
            end_realtime_ns: Some(timeline::paused_end(
                p.at_realtime_ns,
                range,
                now_realtime_ns,
            )),
        });
    }
    range.from_history().then(|| DeviceHistoryRequest {
        range: range.device_range(),
        end_realtime_ns: None,
    })
}

/// `history.device_metrics`, on before settings load. 24 h and 7 d read only the stored history,
/// so with it off their segments are disabled (UI-PRF-060).
pub fn history_kept(cx: &App) -> bool {
    !cx.has_global::<TlSettings>() || TlSettings::settings(cx).history.device_metrics
}

/// The segmented switch: one segment per range, the current one on. A click saves the range.
/// While paused, a range the history cannot serve at the picked time is disabled with "Not kept
/// at this resolution" (UI-PRF-057). With `history.device_metrics` off, 24 h and 7 d are
/// disabled with "Device history is off" (UI-PRF-060).
pub fn range_switch(prefix: &'static str, cx: &App) -> AnyElement {
    let current = current_range(cx);
    let paused = timeline::paused(cx);
    let now = timeline::now_realtime_ns();
    let kept = history_kept(cx);
    let mut row = div().flex().flex_row().flex_none();
    for (range, key) in OverviewRange::ALL.into_iter().zip(SWITCH_KEYS) {
        let id = segment_id(prefix, range);
        let stored = kept || !matches!(range, OverviewRange::H24 | OverviewRange::D7);
        let servable = paused.is_none_or(|p| timeline::servable(range, now - p.at_realtime_ns));
        let mut button = TlButton::new(
            ElementId::Name(segment_id(prefix, range).into()),
            TlButtonKind::Segment {
                on: range == current,
            },
        )
        .label(tr(cx, key))
        .disabled(!stored || !servable)
        .on_click(move |_, _, cx| set_range(range, cx));
        if !stored {
            button = button.tooltip(tr(cx, "perf-overview-range-history-off"));
        } else if !servable {
            button = button.tooltip(tr(cx, "perf-overview-range-not-kept"));
        }
        row = row.child(div().debug_selector(move || id.clone()).child(button));
    }
    row.into_any_element()
}

/// What a device box shows for the range.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RangeText {
    /// "min 6%  avg 42%  peak 100%", empty while there is no valid sample in the range.
    pub figures: SharedString,
    /// The start label of the time axis, empty before the page set it.
    pub axis_start: SharedString,
}

/// The minimum, mean and peak of a box's main metric over the range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Figures {
    pub min: f32,
    pub avg: f32,
    pub peak: f32,
}

/// Running minimum, sum and maximum.
#[derive(Default)]
struct Acc {
    min: f32,
    max: f32,
    sum: f64,
    n: u32,
}

impl Acc {
    fn push(&mut self, min: f32, mean: f32, max: f32) {
        if self.n == 0 {
            (self.min, self.max) = (min, max);
        } else {
            self.min = self.min.min(min);
            self.max = self.max.max(max);
        }
        self.sum += f64::from(mean);
        self.n += 1;
    }

    fn done(&self) -> Option<Figures> {
        (self.n > 0).then(|| Figures {
            min: self.min,
            avg: (self.sum / f64::from(self.n)) as f32,
            peak: self.max,
        })
    }
}

/// The figures of the sum of `ids` (one or two series of one device) over `range`, `None`
/// without a valid sample. Live ranges take the min, mean and max of the valid samples from the
/// newest sample back over the range. History ranges take the min of the stored mins, the mean
/// of the means and the max of the maxes of the served points (UI-PRF-055); a history that
/// answers another range gives `None`. Two series are summed where both have a valid value at
/// the same time. In the history the stored min and max of two series need not fall at the same
/// moment, so their sums are only bounds of the total; there the min, avg and peak of the sum
/// are taken from the sum of the two means of each point. Gaps are left out.
pub fn figures(snap: &Snapshot, ids: &[SeriesId], range: OverviewRange) -> Option<Figures> {
    let mut acc = Acc::default();
    let paused = snap
        .device_history
        .request
        .is_some_and(|r| r.end_realtime_ns.is_some());
    if range.from_history() || paused {
        history_figures(&snap.device_history, ids, range, &mut acc)?;
    } else {
        let span = range.span_ns();
        let mut views = ids.iter().map(|&id| find_tiered(&snap.series, id, span).0);
        let a = views.next()??;
        let b = match views.next() {
            Some(v) => Some(v?),
            None => None,
        };
        let newest = [Some(a), b]
            .iter()
            .flatten()
            .filter_map(|v| v.samples.last())
            .map(|s| s.t_ns)
            .max()?;
        let from = newest.saturating_sub(span);
        let a = tail(&a.samples, from);
        match b {
            None => a
                .iter()
                .filter(|s| s.is_valid())
                .for_each(|s| acc.push(s.value, s.value, s.value)),
            Some(b) => merge(
                a,
                tail(&b.samples, from),
                |s| s.t_ns as i64,
                |x, y| {
                    if x.is_valid() && y.is_valid() {
                        let v = x.value + y.value;
                        acc.push(v, v, v);
                    }
                },
            ),
        }
    }
    acc.done()
}

/// The samples of `s` from time `from` on.
fn tail(s: &[Sample], from: u64) -> &[Sample] {
    &s[s.partition_point(|x| x.t_ns < from)..]
}

/// Adds the points of `ids` in `hist` to `acc`. `None` when the history answers another range
/// or the first series is absent.
fn history_figures(
    hist: &DeviceHistory,
    ids: &[SeriesId],
    range: OverviewRange,
    acc: &mut Acc,
) -> Option<()> {
    if hist.request.map(|r| r.range) != Some(range.device_range()) {
        return None;
    }
    let a = hist.get(*ids.first()?)?;
    match ids.get(1) {
        None => a
            .iter()
            .filter(|p| !p.is_gap())
            .for_each(|p| acc.push(p.min, p.mean, p.max)),
        Some(&id) => {
            let b = hist.get(id)?;
            merge(
                a,
                b,
                |p: &HistPoint| p.t_realtime_ns,
                |x, y| {
                    if !x.is_gap() && !y.is_gap() {
                        let v = x.mean + y.mean;
                        acc.push(v, v, v);
                    }
                },
            );
        }
    }
    Some(())
}

/// Calls `f` for each pair of items of `a` and `b` (both sorted by `t`) with the same time.
fn merge<T>(a: &[T], b: &[T], t: impl Fn(&T) -> i64, mut f: impl FnMut(&T, &T)) {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (ta, tb) = (t(&a[i]), t(&b[j]));
        match ta.cmp(&tb) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                f(&a[i], &b[j]);
                i += 1;
                j += 1;
            }
        }
    }
}

/// The figures of the busiest of `ids` (one series per device): the one with the highest mean.
pub fn busiest(snap: &Snapshot, ids: &[SeriesId], range: OverviewRange) -> Option<Figures> {
    ids.iter()
        .filter_map(|&id| figures(snap, &[id], range))
        .max_by(|a, b| a.avg.total_cmp(&b.avg))
}

/// The figures line from `template` ("min { $min }  avg { $avg }  peak { $peak }"), each value
/// written by `fmt`; empty without figures.
pub fn figures_text(
    template: &Template,
    figures: Option<Figures>,
    mut fmt: impl FnMut(f32, &mut String),
) -> SharedString {
    let Some(f) = figures else {
        return SharedString::default();
    };
    let mut values = [String::new(), String::new(), String::new()];
    for (v, out) in [f.min, f.avg, f.peak].into_iter().zip(&mut values) {
        fmt(v, out);
    }
    let mut out = String::new();
    let _ = template.write(&mut out, &[&values[0], &values[1], &values[2]]);
    out.into()
}

/// A whole percent, "42%".
pub fn whole_percent(v: f32, out: &mut String) {
    use std::fmt::Write as _;
    let _ = write!(out, "{:.0}%", v.clamp(0., 100.));
}

/// The time axis start label: "1 minute ago" and so on, or `since` ("since 09:14") for a history
/// range whose served history starts later than the range does. `now_realtime_ns` is the
/// current CLOCK_REALTIME time.
pub fn axis_start(
    range: OverviewRange,
    hist: &DeviceHistory,
    now_realtime_ns: i64,
    ago: &[SharedString; 5],
    since: &Template,
) -> SharedString {
    let i = OverviewRange::ALL
        .iter()
        .position(|&r| r == range)
        .unwrap_or(0);
    let plain = ago[i].clone();
    if !range.from_history() || hist.request.map(|r| r.range) != Some(range.device_range()) {
        return plain;
    }
    let start = now_realtime_ns.saturating_sub(range.span_ns() as i64);
    match hist.since_realtime_ns {
        Some(oldest) if oldest > start => {
            let Ok(ts) = jiff::Timestamp::from_nanosecond(i128::from(oldest)) else {
                return plain;
            };
            let clock = ts
                .to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string();
            let mut out = String::new();
            let _ = since.write(&mut out, &[&clock]);
            out.into()
        }
        _ => plain,
    }
}

/// The translated time axis start labels, in [`OverviewRange::ALL`] order.
pub fn ago_words(cx: &App) -> [SharedString; 5] {
    AGO_KEYS.map(|k| tr(cx, k))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tl_model::demand::DeviceRange;
    use tl_model::ids::{DeviceIdx, MetricId};
    use tl_model::series::SeriesView;
    use tl_model::snapshot::SeriesSet;
    use tl_model::time::BootId;

    use super::*;

    const S: u64 = 1_000_000_000;

    fn id(metric: MetricId) -> SeriesId {
        SeriesId {
            device: DeviceIdx(0),
            metric,
        }
    }

    fn view(metric: MetricId, samples: Vec<Sample>) -> Arc<SeriesView> {
        Arc::new(SeriesView {
            id: id(metric),
            version: 1,
            period_ns: S,
            samples: samples.into(),
        })
    }

    fn point(t: i64, min: f32, mean: f32, max: f32) -> HistPoint {
        HistPoint {
            t_realtime_ns: t,
            mean,
            min,
            max,
        }
    }

    #[test]
    fn live_figures_skip_gaps_and_old_samples() {
        let mut snap = Snapshot::empty(BootId(0), 0);
        let samples = vec![
            Sample::valid(S, 99.0), // older than 60 s before the newest
            Sample::valid(100 * S, 10.0),
            Sample::gap(101 * S),
            Sample::valid(102 * S, 30.0),
            Sample::valid(103 * S, 20.0),
        ];
        snap.series = Arc::new(SeriesSet {
            series: Arc::from([view(MetricId::CPU_TOTAL, samples)]),
            ..SeriesSet::default()
        });
        let f = figures(&snap, &[id(MetricId::CPU_TOTAL)], OverviewRange::M1).expect("figures");
        assert_eq!(f.min, 10.0);
        assert_eq!(f.avg, 20.0);
        assert_eq!(f.peak, 30.0);
    }

    #[test]
    fn history_figures_use_stored_min_and_max() {
        let mut snap = Snapshot::empty(BootId(0), 0);
        let nan = f32::NAN;
        snap.device_history = Arc::new(DeviceHistory {
            request: Some(DeviceHistoryRequest {
                range: DeviceRange::H24,
                end_realtime_ns: None,
            }),
            series: Arc::from([(
                id(MetricId::CPU_TOTAL),
                Arc::from([
                    point(10, 5.0, 20.0, 60.0),
                    point(20, nan, nan, nan),
                    point(30, 2.0, 40.0, 100.0),
                ]),
            )]),
            ..DeviceHistory::default()
        });
        let ids = [id(MetricId::CPU_TOTAL)];
        let f = figures(&snap, &ids, OverviewRange::H24).expect("figures");
        assert_eq!((f.min, f.avg, f.peak), (2.0, 30.0, 100.0));
        // An answer for another range is not used.
        assert_eq!(figures(&snap, &ids, OverviewRange::H1), None);
    }

    #[test]
    fn two_series_sum_where_both_are_valid() {
        let mut snap = Snapshot::empty(BootId(0), 0);
        snap.series = Arc::new(SeriesSet {
            series: Arc::from([
                view(
                    MetricId::NET_RX,
                    vec![Sample::valid(S, 1.0), Sample::valid(2 * S, 3.0)],
                ),
                view(
                    MetricId::NET_TX,
                    vec![Sample::gap(S), Sample::valid(2 * S, 4.0)],
                ),
            ]),
            ..SeriesSet::default()
        });
        let ids = [id(MetricId::NET_RX), id(MetricId::NET_TX)];
        let f = figures(&snap, &ids, OverviewRange::M5).expect("figures");
        assert_eq!((f.min, f.avg, f.peak), (7.0, 7.0, 7.0));
    }

    #[test]
    fn history_two_series_sum_the_means_not_the_extrema() {
        // RX and TX swap between 100 and 0 inside each point: the total stays 100, so the sum
        // of the stored mins (0) and maxes (200) would report a min and peak that never
        // happened.
        let mut snap = Snapshot::empty(BootId(0), 0);
        snap.device_history = Arc::new(DeviceHistory {
            request: Some(DeviceHistoryRequest {
                range: DeviceRange::H1,
                end_realtime_ns: None,
            }),
            series: Arc::from([
                (
                    id(MetricId::NET_RX),
                    Arc::from([point(10, 0.0, 50.0, 100.0), point(20, 0.0, 60.0, 100.0)]),
                ),
                (
                    id(MetricId::NET_TX),
                    Arc::from([point(10, 0.0, 50.0, 100.0), point(20, 0.0, 40.0, 100.0)]),
                ),
            ]),
            ..DeviceHistory::default()
        });
        let ids = [id(MetricId::NET_RX), id(MetricId::NET_TX)];
        let f = figures(&snap, &ids, OverviewRange::H1).expect("figures");
        assert_eq!((f.min, f.avg, f.peak), (100.0, 100.0, 100.0));
    }

    #[test]
    fn history_request_only_for_long_ranges() {
        assert_eq!(history_request(OverviewRange::M1, None, 0), None);
        assert_eq!(history_request(OverviewRange::M5, None, 0), None);
        assert_eq!(
            history_request(OverviewRange::D7, None, 0),
            Some(DeviceHistoryRequest {
                range: DeviceRange::D7,
                end_realtime_ns: None
            })
        );
    }

    #[test]
    fn paused_request_centres_the_picked_time() {
        let now = 100 * 3_600 * S as i64;
        let paused = |at| {
            Some(Paused {
                at_realtime_ns: at,
                kind: timeline::PauseKind::Pause,
            })
        };
        // A live range is asked for from the history too, ending half a range after the pick.
        let at = now - 3_600 * S as i64;
        assert_eq!(
            history_request(OverviewRange::M1, paused(at), now),
            Some(DeviceHistoryRequest {
                range: DeviceRange::M1,
                end_realtime_ns: Some(at + 30 * S as i64),
            })
        );
        // Never after now.
        assert_eq!(
            history_request(OverviewRange::H24, paused(at), now).and_then(|r| r.end_realtime_ns),
            Some(now)
        );
    }
}
