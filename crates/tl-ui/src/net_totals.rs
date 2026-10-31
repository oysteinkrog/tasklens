//! The window's network total (ADR-018, tasklens-q4x76.22).
//!
//! The engine counts every adapter kind: `aggregate:net` is the total of all kinds, and
//! `aggregate:net:<kind>` the total of each kind. The window shows the total minus the per-kind
//! totals of the kinds its `performance.show_network_*` settings hide, so two windows on one
//! engine each see their own total (tested with two sessions on the real engine in
//! `tl-app/tests/net_kinds_two_sessions.rs`). History written before the per-kind totals has none
//! of them, so there the total reads as it was stored.

use std::sync::Arc;

use smallvec::SmallVec;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::SeriesView;
use tl_model::settings::PerformanceSettings;
use tl_model::snapshot::{DeviceTimeline, HistPoint, Snapshot, TimelineBand};

use crate::chart::view::derived_view;

/// The devices of the window's network total: the engine's total of every kind, and the
/// per-kind totals of the hidden kinds, which the window takes away.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetTotal {
    /// `aggregate:net`; `None` before the catalog has it.
    pub total: Option<DeviceIdx>,
    /// `aggregate:net:<kind>` of each hidden kind the catalog has, in `NetKind::ALL` order.
    pub hidden: SmallVec<[DeviceIdx; 5]>,
}

impl NetTotal {
    /// The total for a window with settings `p`, from `catalog`.
    pub fn of(catalog: &DeviceCatalog, p: &PerformanceSettings) -> Self {
        let idx = |kind| {
            catalog
                .find(&DeviceIdentity::Aggregate(kind))
                .map(|e| e.idx)
        };
        Self {
            total: idx(AggregateKind::Nets),
            hidden: hidden_kinds(p)
                .filter_map(|kind| idx(AggregateKind::NetsOf(kind)))
                .collect(),
        }
    }

    /// The total's series of `metric`, `None` before the catalog has the total.
    pub fn id(&self, metric: MetricId) -> Option<SeriesId> {
        self.total.map(|device| SeriesId { device, metric })
    }

    /// The series to take away from the total's `metric`, for `ChartSeries::minus`.
    pub fn minus(&self, metric: MetricId) -> Arc<[SeriesId]> {
        self.hidden
            .iter()
            .map(|&device| SeriesId { device, metric })
            .collect()
    }

    /// The shown total of `metric` as a live view of the fast tier: the engine's view itself
    /// when no kind is hidden, else a new view with the hidden kinds taken away
    /// ([`derived_view`]).
    pub fn view(&self, snap: &Snapshot, metric: MetricId) -> Option<Arc<SeriesView>> {
        let a = snap.series.get(self.id(metric)?)?;
        if self.hidden.is_empty() {
            return Some(Arc::clone(a));
        }
        let minus = self
            .hidden
            .iter()
            .map(|&device| snap.series.get(SeriesId { device, metric }).map(|v| &**v));
        Some(Arc::new(derived_view(a, minus)))
    }

    /// The newest valid value of the shown total of `metric`.
    pub fn newest(&self, snap: &Snapshot, metric: MetricId) -> Option<f32> {
        self.view(snap, metric)?
            .samples
            .iter()
            .rev()
            .find(|s| s.is_valid())
            .map(|s| s.value)
    }
}

/// The kinds settings `p` hide.
pub fn hidden_kinds(p: &PerformanceSettings) -> impl Iterator<Item = NetKind> + '_ {
    NetKind::ALL
        .into_iter()
        .filter(|&kind| !p.shows_net_kind(Some(kind)))
}

/// `points` minus the points of each `minus` series at the same time, each of mean, min and max
/// minus the other point's mean, at least 0. A gap stays a gap; a `minus` series with no point
/// or a gap at that time counts as 0. Both are sorted by time.
pub fn hist_minus(points: &[HistPoint], minus: &[&[HistPoint]]) -> Vec<HistPoint> {
    let mut out = points.to_vec();
    for b in minus {
        let mut j = 0;
        for p in out.iter_mut().filter(|p| !p.is_gap()) {
            while j < b.len() && b[j].t_realtime_ns < p.t_realtime_ns {
                j += 1;
            }
            if let Some(q) = b.get(j)
                && q.t_realtime_ns == p.t_realtime_ns
                && !q.is_gap()
            {
                p.mean = (p.mean - q.mean).max(0.);
                p.min = (p.min - q.mean).max(0.);
                p.max = (p.max - q.mean).max(0.);
            }
        }
    }
    out
}

/// The timeline's band `band` for a window with settings `p`: the Network band without the
/// kinds `p` hides; every other band as the engine sent it.
pub fn timeline_band(
    tl: &DeviceTimeline,
    band: usize,
    p: &PerformanceSettings,
) -> Arc<[HistPoint]> {
    let points = &tl.bands[band];
    if TimelineBand::ALL.get(band) != Some(&TimelineBand::Network) {
        return Arc::clone(points);
    }
    let minus: SmallVec<[&[HistPoint]; 5]> = hidden_kinds(p)
        .filter_map(|kind| {
            tl.net_kinds
                .iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, pts)| &pts[..])
        })
        .collect();
    if minus.is_empty() {
        return Arc::clone(points);
    }
    hist_minus(points, &minus).into()
}

#[cfg(test)]
mod tests {
    use tl_model::devices::DeviceEntry;
    use tl_model::series::Sample;
    use tl_model::snapshot::SeriesSet;
    use tl_model::time::BootId;

    use super::*;

    const S: u64 = 1_000_000_000;

    fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("n"),
            present: true,
            parent: None,
            persistent_key: Arc::from("k"),
            net_kind: None,
            hot_c: None,
        }
    }

    fn catalog() -> DeviceCatalog {
        DeviceCatalog {
            entries: Arc::from(vec![
                entry(0, DeviceIdentity::Aggregate(AggregateKind::Nets)),
                entry(
                    1,
                    DeviceIdentity::Aggregate(AggregateKind::NetsOf(NetKind::Wired)),
                ),
                entry(
                    2,
                    DeviceIdentity::Aggregate(AggregateKind::NetsOf(NetKind::Virtual)),
                ),
            ]),
            ..DeviceCatalog::default()
        }
    }

    fn view(device: u16, values: &[f32]) -> Arc<SeriesView> {
        let samples: Vec<Sample> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| Sample::valid((i as u64 + 1) * S, v))
            .collect();
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(device),
                metric: MetricId::NET_RX,
            },
            version: 1,
            period_ns: S,
            samples: samples.into(),
        })
    }

    fn snapshot() -> Snapshot {
        let mut snap = Snapshot::empty(BootId(1), 1);
        snap.devices = Arc::new(catalog());
        snap.series = Arc::new(SeriesSet {
            series: Arc::from(vec![
                view(0, &[1000., 1700.]),
                view(1, &[1000., 1000.]),
                view(2, &[0., 700.]),
            ]),
            ..SeriesSet::default()
        });
        snap
    }

    /// The default settings hide virtual adapters, so their total is taken away; a window that
    /// shows every kind reads the engine's total as it is.
    #[test]
    fn two_settings_read_their_own_total_from_one_snapshot() {
        let snap = snapshot();
        let default = PerformanceSettings::default();
        let all = PerformanceSettings {
            show_network_virtual: true,
            ..PerformanceSettings::default()
        };
        let a = NetTotal::of(&snap.devices, &default);
        let b = NetTotal::of(&snap.devices, &all);
        assert_eq!(a.hidden.as_slice(), &[DeviceIdx(2)]);
        assert!(b.hidden.is_empty());
        assert_eq!(a.newest(&snap, MetricId::NET_RX), Some(1000.));
        assert_eq!(b.newest(&snap, MetricId::NET_RX), Some(1700.));
        assert_eq!(
            &*a.minus(MetricId::NET_TX),
            &[SeriesId {
                device: DeviceIdx(2),
                metric: MetricId::NET_TX,
            }]
        );
    }

    /// A hidden kind the engine has not seen has no series, so nothing is taken away.
    #[test]
    fn hidden_kind_without_a_total_takes_nothing_away() {
        let snap = snapshot();
        let p = PerformanceSettings {
            show_network_vpn: false,
            show_network_virtual: true,
            ..PerformanceSettings::default()
        };
        let t = NetTotal::of(&snap.devices, &p);
        assert!(t.hidden.is_empty());
        assert_eq!(t.newest(&snap, MetricId::NET_RX), Some(1700.));
    }

    fn pt(t: i64, mean: f32) -> HistPoint {
        HistPoint {
            t_realtime_ns: t,
            mean,
            min: mean,
            max: mean,
        }
    }

    /// Old history has no per-kind points: the stored total reads as it is. Where a per-kind
    /// point exists at the same time, it is taken away; a gap stays a gap.
    #[test]
    fn hist_minus_keeps_old_points_and_gaps() {
        let total = [pt(10, 500.), pt(20, 900.), pt(30, f32::NAN), pt(40, 800.)];
        let virt = [pt(20, 300.), pt(40, f32::NAN)];
        let out = hist_minus(&total, &[&virt]);
        assert_eq!(out[0].mean, 500.);
        assert_eq!(out[1].mean, 600.);
        assert!(out[2].is_gap());
        assert_eq!(out[3].mean, 800.);
        assert_eq!(hist_minus(&total, &[])[1].mean, 900.);
    }

    #[test]
    fn timeline_network_band_drops_hidden_kinds() {
        let band = Arc::from(vec![pt(10, 1000.), pt(20, 1000.)]);
        let tl = DeviceTimeline {
            bands: [Arc::from([]), Arc::from([]), Arc::from([]), band],
            net_kinds: Arc::from(vec![(
                NetKind::Virtual,
                Arc::from(vec![pt(10, 250.), pt(20, 400.)]),
            )]),
            ..DeviceTimeline::default()
        };
        let p = PerformanceSettings::default();
        let shown = timeline_band(&tl, 3, &p);
        assert_eq!(
            shown.iter().map(|p| p.mean).collect::<Vec<_>>(),
            [750., 600.]
        );
        let all = PerformanceSettings {
            show_network_virtual: true,
            ..PerformanceSettings::default()
        };
        assert!(Arc::ptr_eq(&timeline_band(&tl, 3, &all), &tl.bands[3]));
        assert!(Arc::ptr_eq(&timeline_band(&tl, 0, &p), &tl.bands[0]));
    }
}
