//! The Overview at the picked time (UI-PRF-057). While the timeline holds the page at a time,
//! every box reads [`box_snapshot`] instead of the live snapshot, and [`box_mask`] tells it
//! which snapshot changes count.
//!
//! The snapshot at the picked time is the live one with its values replaced by the device
//! metric history's point at that time: system totals, the series (the points up to 60 s
//! before the picked time, so a newest sample or a 60 s peak reads that time) and no processes,
//! since the device history keeps none. The device catalog, names and capacities (memory
//! total, VRAM total, link speed) stay from the live snapshot: they say which devices exist,
//! not what they did. A value the history does not keep at that time is `Missing` ("-"); until
//! the history answers the paused request every value is `WarmingUp`. A value the device does
//! not report at all stays `Unsupported`, so a box lists the same rows as when live.

use std::sync::Arc;

use gpui_kit::{App, Entity};
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{
    ComponentMask, DeviceHistory, HistPoint, ProcessSnapshot, SeriesSet, Snapshot, SystemSnapshot,
};
use tl_model::system::{
    CpuTotals, DiskTotals, GpuEngine, GpuTotals, MemoryTotals, Metric, NetTotals, SensorKind,
    SensorReading,
};
use tl_model::validity::Validity;

use super::timeline;
use crate::hub::SnapshotHub;

/// How far back the series of the snapshot at the picked time reach.
const SERIES_WINDOW_NS: i64 = 60_000_000_000;

/// What an Overview box reads: the live snapshot, or while the page is paused or previewing,
/// the snapshot at the picked time.
pub fn box_snapshot(hub: &Entity<SnapshotHub>, cx: &App) -> Arc<Snapshot> {
    let live = hub.read(cx).current();
    match timeline::paused(cx) {
        None => Arc::clone(live),
        Some(p) => Arc::new(picked_snapshot(live, p.at_realtime_ns)),
    }
}

/// The parts of a snapshot change that matter to a box's values. While paused, live system
/// totals, series and processes count for nothing, and a new device history stands for all
/// three.
pub fn box_mask(mask: ComponentMask, cx: &App) -> ComponentMask {
    if timeline::paused(cx).is_none() {
        return mask;
    }
    let values = ComponentMask::SYSTEM | ComponentMask::SERIES | ComponentMask::PROCESSES;
    let kept = ComponentMask(mask.0 & !values.0);
    if mask.contains(ComponentMask::DEVICE_HISTORY) {
        kept | values
    } else {
        kept
    }
}

/// The mean of the point of `points` whose bucket of `step_ns` holds `at`, `None` in a gap or
/// outside the points.
pub fn point_at(points: &[HistPoint], at: i64, step_ns: u64) -> Option<f32> {
    let i = points.partition_point(|p| p.t_realtime_ns <= at);
    let p = points.get(i.checked_sub(1)?)?;
    let end = p
        .t_realtime_ns
        .saturating_add(i64::try_from(step_ns).unwrap_or(i64::MAX));
    (at < end && !p.is_gap()).then_some(p.mean)
}

/// Reads the history's values at the picked time.
struct Picker<'a> {
    hist: &'a DeviceHistory,
    at: i64,
    /// The history answers a paused request whose range holds `at`.
    answered: bool,
}

impl<'a> Picker<'a> {
    fn new(hist: &'a DeviceHistory, at: i64) -> Self {
        let answered = hist.request.is_some_and(|r| {
            r.end_realtime_ns.is_some_and(|end| {
                let span = i64::try_from(r.range.span_ns()).unwrap_or(i64::MAX);
                (end.saturating_sub(span)..=end).contains(&at)
            })
        });
        Self { hist, at, answered }
    }

    fn points(&self, id: SeriesId) -> Option<&'a [HistPoint]> {
        if !self.answered {
            return None;
        }
        let series = &self.hist.series;
        let i = series.binary_search_by(|(s, _)| s.cmp(&id)).ok()?;
        Some(&series[i].1)
    }

    fn value(&self, device: DeviceIdx, metric: MetricId) -> Option<f32> {
        point_at(
            self.points(SeriesId { device, metric })?,
            self.at,
            self.hist.step_ns,
        )
    }

    /// The validity of a value the history does not hold.
    fn none(&self) -> Validity {
        if self.answered {
            Validity::Missing
        } else {
            Validity::WarmingUp
        }
    }

    /// No value at the picked time: `Unsupported` stays, anything else is [`Self::none`].
    fn blank<T: Copy + Default>(&self, live: Metric<T>) -> Metric<T> {
        if live.validity == Validity::Unsupported {
            live
        } else {
            Metric::with(self.none())
        }
    }

    /// `metric` of `device` at the picked time in place of `live`.
    fn metric<T: Copy + Default>(
        &self,
        live: Metric<T>,
        device: DeviceIdx,
        metric: MetricId,
        from: impl FnOnce(f32) -> T,
    ) -> Metric<T> {
        if live.validity == Validity::Unsupported {
            return live;
        }
        match self.value(device, metric) {
            Some(v) => Metric::valid(from(v)),
            None => Metric::with(self.none()),
        }
    }

    fn f32(&self, live: Metric<f32>, device: DeviceIdx, metric: MetricId) -> Metric<f32> {
        self.metric(live, device, metric, |v| v)
    }

    fn f64(&self, live: Metric<f64>, device: DeviceIdx, metric: MetricId) -> Metric<f64> {
        self.metric(live, device, metric, f64::from)
    }

    fn u32(&self, live: Metric<u32>, device: DeviceIdx, metric: MetricId) -> Metric<u32> {
        self.metric(live, device, metric, |v| v.max(0.) as u32)
    }

    fn u64(&self, live: Metric<u64>, device: DeviceIdx, metric: MetricId) -> Metric<u64> {
        self.metric(live, device, metric, |v| v.max(0.) as u64)
    }

    fn cpu(&self, live: &CpuTotals, d: DeviceIdx) -> CpuTotals {
        CpuTotals {
            util_pct: self.f32(live.util_pct, d, MetricId::CPU_TOTAL),
            kernel_pct: self.f32(live.kernel_pct, d, MetricId::CPU_KERNEL),
            iowait_pct: self.blank(live.iowait_pct),
            freq_mhz: self.u32(live.freq_mhz, d, MetricId::CPU_FREQ),
            freq_fastest_mhz: self.blank(live.freq_fastest_mhz),
            per_cpu_mhz: live.per_cpu_mhz.iter().map(|m| self.blank(*m)).collect(),
            temp_c: self.f32(live.temp_c, d, MetricId::CPU_TEMP),
            package_power_w: self.f32(live.package_power_w, d, MetricId::CPU_POWER_PKG),
            core_power_w: self.f32(live.core_power_w, d, MetricId::CPU_POWER_CORE),
            processes: self.blank(live.processes),
            threads: self.blank(live.threads),
            handles: self.blank(live.handles),
            load_avg: self.blank(live.load_avg),
            psi_some_pct: self.f32(live.psi_some_pct, d, MetricId::PSI_CPU_SOME),
            throttling: self.blank(live.throttling),
            ..live.clone()
        }
    }

    fn memory(&self, live: &MemoryTotals, d: DeviceIdx) -> MemoryTotals {
        let bytes = |metric| self.value(d, metric).map(|v| v.max(0.) as u64);
        let Some(used) = bytes(MetricId::MEM_USED) else {
            return MemoryTotals {
                validity: self.none(),
                total: live.total,
                commit_limit: live.commit_limit,
                swap_total: live.swap_total,
                swap_free: live.swap_total,
                huge_pages_total: live.huge_pages_total,
                huge_page_size: live.huge_page_size,
                zram: self.blank(live.zram),
                psi_some_pct: self.blank(live.psi_some_pct),
                psi_full_pct: self.blank(live.psi_full_pct),
                swap_in_pps: self.blank(live.swap_in_pps),
                ..MemoryTotals::default()
            };
        };
        let swap_used = bytes(MetricId::SWAP_USED).unwrap_or(0);
        MemoryTotals {
            validity: Validity::Valid,
            total: live.total,
            available: bytes(MetricId::MEM_AVAILABLE).unwrap_or(0),
            used,
            cached: bytes(MetricId::MEM_CACHED).unwrap_or(0),
            dirty: bytes(MetricId::MEM_DIRTY).unwrap_or(0),
            committed: bytes(MetricId::MEM_COMMIT).unwrap_or(0),
            commit_limit: live.commit_limit,
            swap_total: live.swap_total,
            swap_free: live.swap_total.saturating_sub(swap_used),
            huge_pages_total: live.huge_pages_total,
            huge_page_size: live.huge_page_size,
            zram: self.blank(live.zram),
            psi_some_pct: self.f32(live.psi_some_pct, d, MetricId::PSI_MEM_SOME),
            psi_full_pct: self.f32(live.psi_full_pct, d, MetricId::PSI_MEM_FULL),
            swap_in_pps: self.f32(live.swap_in_pps, d, MetricId::SWAP_IN),
            ..MemoryTotals::default()
        }
    }

    fn disk(&self, d: DeviceIdx, live: &DiskTotals) -> DiskTotals {
        DiskTotals {
            active_pct: self.f32(live.active_pct, d, MetricId::DISK_ACTIVE),
            read_bps: self.f64(live.read_bps, d, MetricId::DISK_READ),
            write_bps: self.f64(live.write_bps, d, MetricId::DISK_WRITE),
            response_ms: self.f32(live.response_ms, d, MetricId::DISK_LATENCY),
            read_total: 0,
            written_total: 0,
            temp_c: self.blank(live.temp_c),
            ..*live
        }
    }

    fn net(&self, d: DeviceIdx, live: &NetTotals) -> NetTotals {
        NetTotals {
            rx_bps: self.f64(live.rx_bps, d, MetricId::NET_RX),
            tx_bps: self.f64(live.tx_bps, d, MetricId::NET_TX),
            rx_total: 0,
            tx_total: 0,
            ..*live
        }
    }

    fn gpu(&self, d: DeviceIdx, live: &GpuTotals) -> GpuTotals {
        let mut engines = live.engines;
        for e in GpuEngine::ALL {
            engines.set(e, self.f32(live.engines.get(e), d, e.metric()));
        }
        GpuTotals {
            util_pct: self.f32(live.util_pct, d, MetricId::GPU_UTIL),
            mem_util_pct: self.blank(live.mem_util_pct),
            mem_used: self.u64(live.mem_used, d, MetricId::GPU_MEM),
            shared_used: self.u64(live.shared_used, d, MetricId::GPU_MEM_SHARED),
            mem_reserved: self.blank(live.mem_reserved),
            enc_pct: self.f32(live.enc_pct, d, MetricId::GPU_ENC),
            dec_pct: self.f32(live.dec_pct, d, MetricId::GPU_DEC),
            enc_sessions: self.blank(live.enc_sessions),
            temp_c: self.f32(live.temp_c, d, MetricId::GPU_TEMP),
            temp_edge_c: self.blank(live.temp_edge_c),
            temp_junction_c: self.blank(live.temp_junction_c),
            temp_mem_c: self.blank(live.temp_mem_c),
            power_w: self.f32(live.power_w, d, MetricId::GPU_POWER),
            energy_since_boot: self.blank(live.energy_since_boot),
            clock_gfx_mhz: self.u32(live.clock_gfx_mhz, d, MetricId::GPU_CLOCK),
            clock_mem_mhz: self.u32(live.clock_mem_mhz, d, MetricId::GPU_MEM_CLOCK),
            clock_video_mhz: self.blank(live.clock_video_mhz),
            pstate: self.blank(live.pstate),
            clock_event_reasons: self.blank(live.clock_event_reasons),
            throttling: self.blank(live.throttling),
            fan_pct: self.f32(live.fan_pct, d, MetricId::GPU_FAN),
            fan_rpm: self.blank(live.fan_rpm),
            pcie_rx_bps: self.f32(live.pcie_rx_bps, d, MetricId::GPU_PCIE_RX),
            pcie_tx_bps: self.f32(live.pcie_tx_bps, d, MetricId::GPU_PCIE_TX),
            pcie_gen: self.blank(live.pcie_gen),
            pcie_width: self.blank(live.pcie_width),
            engines,
            ..live.clone()
        }
    }

    fn sensor(&self, d: DeviceIdx, live: &SensorReading) -> SensorReading {
        let metric = match live.kind {
            SensorKind::Temperature => MetricId::SENSOR_TEMP,
            SensorKind::Fan => MetricId::FAN_RPM,
        };
        SensorReading {
            value: self.f32(live.value, d, metric),
            pwm_pct: self.f32(live.pwm_pct, d, MetricId::FAN_PWM),
            ..*live
        }
    }

    /// Every history series as a view of its valid points in the 60 s up to the picked time.
    /// The samples' `t_ns` is CLOCK_REALTIME: only their order and spacing are read.
    fn series(&self, live: &SeriesSet) -> SeriesSet {
        let views = if self.answered {
            self.hist
                .series
                .iter()
                .map(|(id, points)| {
                    let from = self.at.saturating_sub(SERIES_WINDOW_NS);
                    let samples: Arc<[Sample]> = points
                        .iter()
                        .filter(|p| {
                            !p.is_gap() && p.t_realtime_ns > from && p.t_realtime_ns <= self.at
                        })
                        .map(|p| Sample::valid(p.t_realtime_ns.max(0) as u64, p.mean))
                        .collect();
                    Arc::new(SeriesView {
                        id: *id,
                        version: self.hist.header.version,
                        period_ns: self.hist.step_ns,
                        samples,
                    })
                })
                .collect()
        } else {
            Arc::from([])
        };
        SeriesSet {
            header: live.header,
            series: views,
            slow: Arc::from([]),
        }
    }
}

/// `live` with its values replaced by the device history's at `at`, CLOCK_REALTIME.
pub fn picked_snapshot(live: &Snapshot, at: i64) -> Snapshot {
    let p = Picker::new(&live.device_history, at);
    let idx = |identity| {
        live.devices
            .find(&identity)
            .map_or(DeviceIdx(u16::MAX), |e| e.idx)
    };
    let sys = &live.system;
    let system = SystemSnapshot {
        cpu: p.cpu(&sys.cpu, idx(DeviceIdentity::CpuPackage)),
        memory: p.memory(&sys.memory, idx(DeviceIdentity::Memory)),
        disks: sys.disks.iter().map(|(d, t)| (*d, p.disk(*d, t))).collect(),
        nets: sys.nets.iter().map(|(d, t)| (*d, p.net(*d, t))).collect(),
        gpus: sys.gpus.iter().map(|(d, t)| (*d, p.gpu(*d, t))).collect(),
        sensors: sys
            .sensors
            .iter()
            .map(|(d, t)| (*d, p.sensor(*d, t)))
            .collect(),
        ..(**sys).clone()
    };
    Snapshot {
        system: Arc::new(system),
        series: Arc::new(p.series(&live.series)),
        processes: Arc::new(ProcessSnapshot::default()),
        ..live.clone()
    }
}

#[cfg(test)]
mod tests {
    use tl_model::demand::{DeviceHistoryRequest, DeviceRange};

    use super::*;

    const S: i64 = 1_000_000_000;

    fn pt(t: i64, mean: f32) -> HistPoint {
        HistPoint {
            t_realtime_ns: t,
            mean,
            min: mean,
            max: mean,
        }
    }

    #[test]
    fn point_at_reads_the_bucket_that_holds_the_time() {
        let points = [pt(0, 1.), pt(10 * S, f32::NAN), pt(30 * S, 3.)];
        let step = 10 * S as u64;
        assert_eq!(point_at(&points, 5 * S, step), Some(1.));
        assert_eq!(point_at(&points, 15 * S, step), None, "a gap");
        assert_eq!(point_at(&points, 25 * S, step), None, "no point there");
        assert_eq!(point_at(&points, 30 * S, step), Some(3.));
        assert_eq!(point_at(&points, -S, step), None, "before the first");
        assert_eq!(point_at(&points, 40 * S, step), None, "after the last");
    }

    #[test]
    fn picker_is_answered_only_by_a_paused_request_that_holds_the_time() {
        let hist = |end| DeviceHistory {
            request: Some(DeviceHistoryRequest {
                range: DeviceRange::H1,
                end_realtime_ns: end,
            }),
            ..DeviceHistory::default()
        };
        let at = 1_000 * S;
        assert!(!Picker::new(&hist(None), at).answered, "a live answer");
        assert!(Picker::new(&hist(Some(at + 60 * S)), at).answered);
        assert!(
            !Picker::new(&hist(Some(at - 60 * S)), at).answered,
            "an answer for an earlier pick"
        );
        assert_eq!(Picker::new(&hist(None), at).none(), Validity::WarmingUp);
        assert_eq!(Picker::new(&hist(Some(at)), at).none(), Validity::Missing);
    }
}
