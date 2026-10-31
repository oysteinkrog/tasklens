//! Metric metadata (ADR-019 "Metric metadata", D-041): one [`MetricMeta`] per metric key a
//! widget can show.
//!
//! An entry gives the metric's unit, its [`ValueFormat`], the chart's [`YScale`] rule, whether
//! it is a gauge or a counter, the bar's denominator ([`BarMax`]) and how min, average and peak
//! are worked out over the range ([`StatsRule`]). The forms a metric offers follow from it: a
//! bar only with a denominator, stats only with a stats rule. A counter offers neither.
//!
//! Phase 1 has the GPU metrics only: every engine, VRAM, the two clocks, power and temperature.
//! The denominators and scales that depend on the card read its [`GpuTotals`] record, the same
//! facts the GPU page charts use. This module does no drawing and keeps no state.

use tl_model::ids::SeriesId;
use tl_model::layout::{
    FormSet, METRIC_CLOCK_GFX, METRIC_CLOCK_MEM, METRIC_POWER, METRIC_TEMP, METRIC_VRAM, MetricKey,
};
use tl_model::settings::OverviewRange;
use tl_model::snapshot::Snapshot;
use tl_model::system::{GpuTotals, Metric};
use tl_model::validity::Validity;

use crate::chart::labels::{NetFormat, format_value};
use crate::chart::{ValueFormat, YScale};
use crate::pages::performance::overview::range::{Figures, figures};

/// The unit a metric's values are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// 0 to 100.
    Percent,
    Bytes,
    Megahertz,
    Watts,
    Celsius,
}

/// A gauge is a reading at each sample (busy percent, VRAM in use). A counter only grows
/// (energy used); its value is the running total, and it has no bar and no stats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricKind {
    Gauge,
    Counter,
}

/// A fact of the device record, `None` when the device does not report it.
pub type DeviceFact = fn(&GpuTotals) -> Option<f32>;

/// The value a full bar stands for.
#[derive(Clone, Copy, Debug)]
pub enum BarMax {
    /// Always this value, for example 100 for a percent.
    Fixed(f32),
    /// A value from the device record. A device that does not report it shows no bar.
    Total(DeviceFact),
    /// The metric has no bar.
    None,
}

/// How the chart's Y axis is built (UI-PRF-006).
#[derive(Clone, Copy, Debug)]
pub enum ScaleRule {
    /// `YScale::Fixed`.
    Fixed { min: f32, max: f32 },
    /// `YScale::FixedTotal` of the device fact; `YScale::StickyUp` with no hardware maximum
    /// when the device does not report it, as the GPU page's Memory chart does.
    FixedTotal(DeviceFact),
    /// `YScale::StickyUp` with the device fact as the hardware maximum.
    StickyUp(DeviceFact),
    /// `YScale::StickyTemp`.
    StickyTemp,
}

/// How the stats form works out its figures over the box range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsRule {
    /// Min, average and peak over the range, as the Overview box titles (UI-PRF-055).
    MinAvgPeak,
    /// The metric has no stats.
    None,
}

/// How the value form writes the current value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueText {
    /// The value in the metric's format: "42%", "1755 MHz".
    Plain,
    /// The value and the bar's denominator, "3.1 / 16.0 GiB" (the UI-PRF-017 VRAM row). Just
    /// the value when the device does not report the denominator.
    OfTotal,
}

/// What a metric is and how a widget shows it.
#[derive(Clone, Copy, Debug)]
pub struct MetricMeta {
    pub unit: Unit,
    pub format: ValueFormat,
    pub value_text: ValueText,
    pub scale: ScaleRule,
    pub kind: MetricKind,
    pub bar: BarMax,
    pub stats: StatsRule,
}

/// A device fact when it is shown: valid or stale.
fn shown<T: Copy + Default>(m: Metric<T>) -> Option<T> {
    matches!(m.validity, Validity::Valid | Validity::Stale).then_some(m.value)
}

fn mem_total(t: &GpuTotals) -> Option<f32> {
    shown(t.mem_total).map(|v| v as f32)
}

fn clock_gfx_max(t: &GpuTotals) -> Option<f32> {
    t.info.clock_gfx_max_mhz.map(|v| v as f32)
}

fn clock_mem_max(t: &GpuTotals) -> Option<f32> {
    t.info.clock_mem_max_mhz.map(|v| v as f32)
}

/// The enforced power limit, else the highest limit the card accepts (the GPU page's Power
/// chart uses the same).
fn power_max(t: &GpuTotals) -> Option<f32> {
    shown(t.power_limit_w).or(t.info.power_max_limit_w)
}

fn slowdown_temp(t: &GpuTotals) -> Option<f32> {
    t.info.slowdown_temp_c
}

/// Every GPU engine: `engine:<key>` for each key of `GpuEngine::key`.
pub const GPU_ENGINE: MetricMeta = MetricMeta {
    unit: Unit::Percent,
    format: ValueFormat::Percent,
    value_text: ValueText::Plain,
    scale: ScaleRule::Fixed {
        min: 0.0,
        max: 100.0,
    },
    kind: MetricKind::Gauge,
    bar: BarMax::Fixed(100.0),
    stats: StatsRule::MinAvgPeak,
};

/// `vram`: VRAM in use, of the card's total.
pub const GPU_VRAM: MetricMeta = MetricMeta {
    unit: Unit::Bytes,
    format: ValueFormat::Bytes,
    value_text: ValueText::OfTotal,
    scale: ScaleRule::FixedTotal(mem_total),
    kind: MetricKind::Gauge,
    bar: BarMax::Total(mem_total),
    stats: StatsRule::MinAvgPeak,
};

/// `clock_gfx`: the graphics clock.
pub const GPU_CLOCK_GFX: MetricMeta = MetricMeta {
    unit: Unit::Megahertz,
    format: ValueFormat::Mhz,
    value_text: ValueText::Plain,
    scale: ScaleRule::StickyUp(clock_gfx_max),
    kind: MetricKind::Gauge,
    bar: BarMax::Total(clock_gfx_max),
    stats: StatsRule::MinAvgPeak,
};

/// `clock_mem`: the memory clock.
pub const GPU_CLOCK_MEM: MetricMeta = MetricMeta {
    unit: Unit::Megahertz,
    format: ValueFormat::Mhz,
    value_text: ValueText::Plain,
    scale: ScaleRule::StickyUp(clock_mem_max),
    kind: MetricKind::Gauge,
    bar: BarMax::Total(clock_mem_max),
    stats: StatsRule::MinAvgPeak,
};

/// `power`: the power draw.
pub const GPU_POWER: MetricMeta = MetricMeta {
    unit: Unit::Watts,
    format: ValueFormat::Watts,
    value_text: ValueText::Plain,
    scale: ScaleRule::StickyUp(power_max),
    kind: MetricKind::Gauge,
    bar: BarMax::Total(power_max),
    stats: StatsRule::MinAvgPeak,
};

/// `temp`: the GPU temperature.
pub const GPU_TEMP: MetricMeta = MetricMeta {
    unit: Unit::Celsius,
    format: ValueFormat::Celsius,
    value_text: ValueText::Plain,
    scale: ScaleRule::StickyTemp,
    kind: MetricKind::Gauge,
    bar: BarMax::Total(slowdown_temp),
    stats: StatsRule::MinAvgPeak,
};

/// A denominator that can fill a bar: finite and above 0.
fn usable(max: f32) -> Option<f32> {
    (max.is_finite() && max > 0.0).then_some(max)
}

impl MetricMeta {
    /// The entry for `key`; `None` for a key this build does not know, whose widget shows its
    /// unavailable state.
    pub fn of(key: &MetricKey) -> Option<&'static MetricMeta> {
        if key.gpu_engine().is_some() {
            return Some(&GPU_ENGINE);
        }
        match key.as_str() {
            METRIC_VRAM => Some(&GPU_VRAM),
            METRIC_CLOCK_GFX => Some(&GPU_CLOCK_GFX),
            METRIC_CLOCK_MEM => Some(&GPU_CLOCK_MEM),
            METRIC_POWER => Some(&GPU_POWER),
            METRIC_TEMP => Some(&GPU_TEMP),
            _ => None,
        }
    }

    /// Whether the metric can have a bar on some device. A counter never has one.
    fn has_bar(&self) -> bool {
        self.kind == MetricKind::Gauge && !matches!(self.bar, BarMax::None)
    }

    /// Whether the metric has stats. A counter never has them.
    fn has_stats(&self) -> bool {
        self.kind == MetricKind::Gauge && self.stats == StatsRule::MinAvgPeak
    }

    /// The forms the metric can offer on some device: chart, value and label always, a bar
    /// with a denominator, stats with a stats rule.
    pub fn forms(&self) -> FormSet {
        let mut forms = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        forms.set(FormSet::BAR, self.has_bar());
        forms.set(FormSet::STATS, self.has_stats());
        forms
    }

    /// The forms the metric offers on the device with record `gpu`: [`Self::forms`] without
    /// the bar when the device does not report the bar's denominator. A form the metric does
    /// not offer is not listed in the widget menu.
    pub fn offered_forms(&self, gpu: Option<&GpuTotals>) -> FormSet {
        let mut forms = self.forms();
        if self.bar_max(gpu).is_none() {
            forms.remove(FormSet::BAR);
        }
        forms
    }

    /// The value a full bar stands for on the device with record `gpu`, `None` when the
    /// metric or the device has no bar.
    pub fn bar_max(&self, gpu: Option<&GpuTotals>) -> Option<f32> {
        if !self.has_bar() {
            return None;
        }
        match self.bar {
            BarMax::Fixed(max) => usable(max),
            BarMax::Total(fact) => gpu.and_then(fact).and_then(usable),
            BarMax::None => None,
        }
    }

    /// How full the bar is for `value`, 0 to 1; `None` without a bar.
    pub fn bar_fraction(&self, value: f32, gpu: Option<&GpuTotals>) -> Option<f32> {
        let max = self.bar_max(gpu)?;
        Some(if value.is_finite() {
            (value / max).clamp(0.0, 1.0)
        } else {
            0.0
        })
    }

    /// The chart's Y axis on the device with record `gpu`.
    pub fn y_scale(&self, gpu: Option<&GpuTotals>) -> YScale {
        match self.scale {
            ScaleRule::Fixed { min, max } => YScale::Fixed { min, max },
            ScaleRule::FixedTotal(fact) => match gpu.and_then(fact).and_then(usable) {
                Some(total) => YScale::FixedTotal(total),
                None => YScale::StickyUp { hw_max: None },
            },
            ScaleRule::StickyUp(fact) => YScale::StickyUp {
                hw_max: gpu.and_then(fact).and_then(usable),
            },
            ScaleRule::StickyTemp => YScale::StickyTemp,
        }
    }

    /// Appends the value form's text for `value`: the value in the metric's format, and for
    /// [`ValueText::OfTotal`] " / " and the denominator when the device reports it.
    pub fn write_value(&self, value: f32, gpu: Option<&GpuTotals>, out: &mut String) {
        match (self.value_text, self.bar_max(gpu)) {
            (ValueText::OfTotal, Some(total)) => {
                format_value(self.format, value, NetFormat::default(), out);
                out.push_str(" / ");
                format_value(self.format, total, NetFormat::default(), out);
            }
            _ => format_value(self.format, value, NetFormat::default(), out),
        }
    }

    /// The stats of series `id` over `range` by the metric's stats rule. `MinAvgPeak` takes
    /// the figures the box titles use (UI-PRF-055): the valid live samples for 1 min and 5 min,
    /// the stored points (min of the mins, mean of the means, max of the maxes) for 1 h, 24 h
    /// and 7 d. `None` without a valid sample in the range, and always for a metric without
    /// stats, so the stats text stays empty.
    pub fn stats(&self, snap: &Snapshot, id: SeriesId, range: OverviewRange) -> Option<Figures> {
        if !self.has_stats() {
            return None;
        }
        figures(snap, &[id], range)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
    use tl_model::ids::{DeviceIdx, MetricId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::snapshot::{DeviceHistory, HistPoint, SeriesSet};
    use tl_model::system::{GpuEngine, GpuStaticInfo};
    use tl_model::time::BootId;
    use tl_model::units;

    use super::*;

    const S: u64 = 1_000_000_000;

    /// Every metric key a phase 1 GPU widget can show.
    fn gpu_keys() -> Vec<MetricKey> {
        let mut keys: Vec<MetricKey> = GpuEngine::ALL.into_iter().map(MetricKey::engine).collect();
        for k in [
            METRIC_VRAM,
            METRIC_CLOCK_GFX,
            METRIC_CLOCK_MEM,
            METRIC_POWER,
            METRIC_TEMP,
        ] {
            keys.push(MetricKey::new(k));
        }
        keys
    }

    /// A GPU record with no value and no static facts.
    fn bare_gpu() -> GpuTotals {
        GpuTotals {
            device: DeviceIdx(1),
            util_pct: Metric::default(),
            mem_util_pct: Metric::default(),
            mem_used: Metric::default(),
            mem_total: Metric::with(Validity::Unsupported),
            shared_used: Metric::default(),
            shared_total: Metric::default(),
            enc_pct: Metric::default(),
            dec_pct: Metric::default(),
            mem_reserved: Metric::default(),
            enc_sessions: Metric::default(),
            enc_dec_shared: false,
            temp_c: Metric::default(),
            temp_edge_c: Metric::default(),
            temp_junction_c: Metric::default(),
            temp_mem_c: Metric::default(),
            power_w: Metric::default(),
            power_limit_w: Metric::with(Validity::Unsupported),
            clock_gfx_mhz: Metric::default(),
            clock_mem_mhz: Metric::default(),
            fan_pct: Metric::default(),
            pcie_rx_bps: Metric::default(),
            pcie_tx_bps: Metric::default(),
            engines: Default::default(),
            throttling: Default::default(),
            fdinfo_partial: false,
            energy_since_boot: Metric::default(),
            clock_video_mhz: Metric::default(),
            pstate: Metric::default(),
            clock_event_reasons: Metric::default(),
            fan_rpm: Metric::default(),
            pcie_gen: Metric::default(),
            pcie_width: Metric::default(),
            info: Arc::default(),
        }
    }

    /// A GPU record with every fact a denominator or scale reads.
    fn full_gpu() -> GpuTotals {
        let mut t = bare_gpu();
        t.mem_total = Metric::valid(16 << 30);
        t.power_limit_w = Metric::valid(320.0);
        t.info = Arc::new(GpuStaticInfo {
            clock_gfx_max_mhz: Some(2520),
            clock_mem_max_mhz: Some(10501),
            power_max_limit_w: Some(450.0),
            slowdown_temp_c: Some(90.0),
            ..GpuStaticInfo::default()
        });
        t
    }

    #[test]
    fn every_gpu_metric_has_an_entry() {
        for key in gpu_keys() {
            assert!(key.metric().is_some(), "{key:?} names a series");
            let meta = MetricMeta::of(&key).unwrap_or_else(|| panic!("{key:?} has metadata"));
            assert_eq!(meta.kind, MetricKind::Gauge, "{key:?}");
            assert_eq!(meta.stats, StatsRule::MinAvgPeak, "{key:?}");
            let forms = meta.offered_forms(Some(&full_gpu()));
            assert_eq!(
                forms,
                FormSet::all(),
                "{key:?} offers every form on a full record"
            );
        }
        assert!(MetricMeta::of(&MetricKey::new("engine:nope")).is_none());
        assert!(MetricMeta::of(&MetricKey::new("fan")).is_none());
    }

    #[test]
    fn table_matches_the_adr() {
        let engine = MetricMeta::of(&MetricKey::engine(GpuEngine::Enc)).expect("engine");
        assert_eq!(engine.unit, Unit::Percent);
        assert_eq!(engine.format, ValueFormat::Percent);
        assert_eq!(
            engine.y_scale(None),
            YScale::Fixed {
                min: 0.0,
                max: 100.0
            }
        );
        assert_eq!(engine.bar_max(None), Some(100.0));

        let gpu = full_gpu();
        let at = |key: &str| MetricMeta::of(&MetricKey::new(key)).expect("known key");
        let vram = at(METRIC_VRAM);
        assert_eq!(vram.format, ValueFormat::Bytes);
        assert_eq!(
            vram.y_scale(Some(&gpu)),
            YScale::FixedTotal(16.0 * 1024.0 * 1024.0 * 1024.0)
        );
        assert_eq!(
            vram.bar_max(Some(&gpu)),
            Some(16.0 * 1024.0 * 1024.0 * 1024.0)
        );
        assert_eq!(
            at(METRIC_CLOCK_GFX).y_scale(Some(&gpu)),
            YScale::StickyUp {
                hw_max: Some(2520.0)
            }
        );
        assert_eq!(at(METRIC_CLOCK_GFX).bar_max(Some(&gpu)), Some(2520.0));
        assert_eq!(at(METRIC_CLOCK_MEM).bar_max(Some(&gpu)), Some(10501.0));
        assert_eq!(at(METRIC_CLOCK_MEM).format, ValueFormat::Mhz);
        assert_eq!(at(METRIC_POWER).format, ValueFormat::Watts);
        assert_eq!(at(METRIC_POWER).bar_max(Some(&gpu)), Some(320.0));
        assert_eq!(
            at(METRIC_POWER).y_scale(Some(&gpu)),
            YScale::StickyUp {
                hw_max: Some(320.0)
            }
        );
        assert_eq!(at(METRIC_TEMP).format, ValueFormat::Celsius);
        assert_eq!(at(METRIC_TEMP).y_scale(Some(&gpu)), YScale::StickyTemp);
        assert_eq!(at(METRIC_TEMP).bar_max(Some(&gpu)), Some(90.0));
    }

    #[test]
    fn power_bar_falls_back_to_the_highest_limit() {
        let mut gpu = full_gpu();
        gpu.power_limit_w = Metric::with(Validity::Unsupported);
        assert_eq!(
            MetricMeta::of(&MetricKey::new(METRIC_POWER))
                .expect("power")
                .bar_max(Some(&gpu)),
            Some(450.0)
        );
    }

    #[test]
    fn no_denominator_offers_no_bar() {
        let gpu = bare_gpu();
        for key in [
            METRIC_VRAM,
            METRIC_CLOCK_GFX,
            METRIC_CLOCK_MEM,
            METRIC_POWER,
            METRIC_TEMP,
        ] {
            let meta = MetricMeta::of(&MetricKey::new(key)).expect("known key");
            assert!(meta.forms().contains(FormSet::BAR), "{key} can have a bar");
            for record in [Some(&gpu), None] {
                assert_eq!(meta.bar_max(record), None, "{key}");
                assert_eq!(meta.bar_fraction(50.0, record), None, "{key}");
                let forms = meta.offered_forms(record);
                assert!(!forms.contains(FormSet::BAR), "{key} offers no bar");
                assert!(forms.contains(FormSet::STATS | FormSet::CHART | FormSet::VALUE));
            }
        }
        // VRAM without a total keeps a chart, on a scale that grows with the values.
        let vram = MetricMeta::of(&MetricKey::new(METRIC_VRAM)).expect("vram");
        assert_eq!(vram.y_scale(Some(&gpu)), YScale::StickyUp { hw_max: None });
        // An engine's bar needs no device fact.
        let engine = MetricMeta::of(&MetricKey::engine(GpuEngine::Gpu)).expect("engine");
        assert!(engine.offered_forms(None).contains(FormSet::BAR));
    }

    #[test]
    fn bar_none_and_counters_offer_no_bar_and_no_stats() {
        let no_bar = MetricMeta {
            bar: BarMax::None,
            stats: StatsRule::None,
            ..GPU_ENGINE
        };
        assert_eq!(
            no_bar.forms(),
            FormSet::CHART | FormSet::VALUE | FormSet::LABEL
        );
        assert_eq!(no_bar.bar_max(None), None);
        let counter = MetricMeta {
            kind: MetricKind::Counter,
            ..GPU_ENGINE
        };
        assert_eq!(
            counter.offered_forms(None),
            FormSet::CHART | FormSet::VALUE | FormSet::LABEL
        );
        assert_eq!(counter.bar_max(None), None);
        let snap = Snapshot::empty(BootId(0), 0);
        let id = SeriesId {
            device: DeviceIdx(1),
            metric: MetricId::GPU_UTIL,
        };
        assert_eq!(counter.stats(&snap, id, OverviewRange::M1), None);
    }

    #[test]
    fn bar_fraction_clamps() {
        let engine = &GPU_ENGINE;
        assert_eq!(engine.bar_fraction(42.0, None), Some(0.42));
        assert_eq!(engine.bar_fraction(130.0, None), Some(1.0));
        assert_eq!(engine.bar_fraction(-3.0, None), Some(0.0));
        assert_eq!(engine.bar_fraction(f32::NAN, None), Some(0.0));
    }

    #[test]
    fn value_text() {
        let gpu = full_gpu();
        let text = |meta: &MetricMeta, v: f32, record: Option<&GpuTotals>| {
            let mut out = String::new();
            meta.write_value(v, record, &mut out);
            out
        };
        let vram = MetricMeta::of(&MetricKey::new(METRIC_VRAM)).expect("vram");
        let used = 3.1 * 1024.0 * 1024.0 * 1024.0;
        let mut expected = String::new();
        units::bytes_of(&mut expected, used as u64, 16 << 30);
        assert_eq!(text(vram, used, Some(&gpu)), expected);
        assert!(expected.contains(" / "), "{expected}");
        let mut alone = String::new();
        units::bytes(&mut alone, used as u64);
        assert_eq!(text(vram, used, None), alone, "no total: the value alone");

        let mut pct = String::new();
        format_value(ValueFormat::Percent, 42.0, NetFormat::default(), &mut pct);
        assert_eq!(text(&GPU_ENGINE, 42.0, None), pct);
        let mut mhz = String::new();
        format_value(ValueFormat::Mhz, 1755.0, NetFormat::default(), &mut mhz);
        let clock = MetricMeta::of(&MetricKey::new(METRIC_CLOCK_GFX)).expect("clock");
        assert_eq!(
            text(clock, 1755.0, Some(&gpu)),
            mhz,
            "a clock shows no total"
        );
    }

    fn series(metric: MetricId) -> SeriesId {
        SeriesId {
            device: DeviceIdx(1),
            metric,
        }
    }

    #[test]
    fn live_stats_over_a_known_series() {
        let mut snap = Snapshot::empty(BootId(0), 0);
        let id = series(MetricId::GPU_UTIL);
        let samples = vec![
            Sample::valid(S, 99.0), // more than 60 s before the newest sample
            Sample::valid(100 * S, 10.0),
            Sample::gap(101 * S),
            Sample::valid(102 * S, 30.0),
            Sample::valid(103 * S, 20.0),
        ];
        snap.series = Arc::new(SeriesSet {
            series: Arc::from([Arc::new(SeriesView {
                id,
                version: 1,
                period_ns: S,
                samples: samples.into(),
            })]),
            ..SeriesSet::default()
        });
        let meta = MetricMeta::of(&MetricKey::engine(GpuEngine::Gpu)).expect("engine");
        let f = meta.stats(&snap, id, OverviewRange::M1).expect("stats");
        assert_eq!((f.min, f.avg, f.peak), (10.0, 20.0, 30.0));
        // 5 min reaches the old sample too.
        let f = meta.stats(&snap, id, OverviewRange::M5).expect("stats");
        assert_eq!((f.min, f.avg, f.peak), (10.0, 39.75, 99.0));
        // No series, no stats.
        assert_eq!(
            meta.stats(&snap, series(MetricId::GPU_TEMP), OverviewRange::M1),
            None
        );
    }

    #[test]
    fn history_stats_over_known_points() {
        let mut snap = Snapshot::empty(BootId(0), 0);
        let id = series(MetricId::GPU_POWER);
        let point = |t: i64, min: f32, mean: f32, max: f32| HistPoint {
            t_realtime_ns: t,
            mean,
            min,
            max,
        };
        let nan = f32::NAN;
        snap.device_history = Arc::new(DeviceHistory {
            request: Some(DeviceHistoryRequest {
                range: DeviceRange::H1,
                end_realtime_ns: None,
            }),
            series: Arc::from([(
                id,
                Arc::from([
                    point(10, 40.0, 80.0, 150.0),
                    point(20, nan, nan, nan),
                    point(30, 30.0, 120.0, 300.0),
                ]),
            )]),
            ..DeviceHistory::default()
        });
        let meta = MetricMeta::of(&MetricKey::new(METRIC_POWER)).expect("power");
        let f = meta.stats(&snap, id, OverviewRange::H1).expect("stats");
        assert_eq!((f.min, f.avg, f.peak), (30.0, 100.0, 300.0));
        // A history for another range gives no stats.
        assert_eq!(meta.stats(&snap, id, OverviewRange::H24), None);
    }
}
