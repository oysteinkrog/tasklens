//! Metric metadata (ADR-019 "Metric metadata", D-041): one [`MetricMeta`] per metric key a
//! widget can show.
//!
//! An entry gives the metric's unit, its [`ValueFormat`], the chart's [`YScale`] rule, whether
//! it is a gauge or a counter, the bar's denominator ([`BarMax`]), how min, average and peak
//! are worked out over the range ([`StatsRule`]), how many lines its chart draws ([`Lines`])
//! and whether its value is smoothed. The forms a metric offers follow from it: a chart only
//! with a series, a bar only with a denominator, stats only with a stats rule and one series.
//! A counter offers no bar and no stats.
//!
//! A key is looked up with its device's kind ([`DeviceKind`]), so `temp` and `power` can mean a
//! CPU or a GPU metric. The denominators and scales that depend on the device read the snapshot
//! ([`DeviceFact`]): the GPU record, the memory totals, an adapter's link speed, the CPU
//! inventory. This module does no drawing and keeps no state.

use tl_model::ids::{DeviceIdx, SeriesId};
use tl_model::layout::{
    FormSet, METRIC_CLOCK_GFX, METRIC_CLOCK_MEM, METRIC_POWER, METRIC_TEMP, METRIC_VRAM, MetricKey,
};
use tl_model::layout_boxes::{
    DeviceKind, METRIC_CORE_CLOCKS, METRIC_CPU_KERNEL, METRIC_CPU_TOTAL, METRIC_DISK_ACTIVE,
    METRIC_DISK_IO, METRIC_DISK_READ, METRIC_DISK_WRITE, METRIC_FREQ, METRIC_LOAD,
    METRIC_MEM_AVAILABLE, METRIC_MEM_CACHED, METRIC_MEM_COMMIT, METRIC_MEM_COMPOSITION,
    METRIC_MEM_SWAP, METRIC_MEM_USED, METRIC_NET_RX, METRIC_NET_TRAFFIC, METRIC_NET_TX,
    METRIC_POWER_CORES, METRIC_POWER_DRAW, METRIC_PRESSURE,
};
use tl_model::settings::OverviewRange;
use tl_model::snapshot::Snapshot;
use tl_model::system::{GpuTotals, Metric};
use tl_model::validity::Validity;

use crate::chart::labels::{NetFormat, format_value};
use crate::chart::{ValueFormat, YScale};
use crate::pages::performance::core_clocks::ClockScale;
use crate::pages::performance::gpu::gpu_totals;
use crate::pages::performance::net::throughput_scale;
use crate::pages::performance::overview::range::{Figures, figures, figures_minus};
use crate::pages::performance::status_bar::rules::cpu_hot_c;

/// The unit a metric's values are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// 0 to 100.
    Percent,
    Bytes,
    /// Bytes per second.
    BytesPerSecond,
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

/// A fact of the device `DeviceIdx` in the snapshot, `None` when the device does not report it.
pub type DeviceFact = fn(&Snapshot, DeviceIdx) -> Option<f32>;

/// The device a metric is read on: the snapshot and the device's index. `None` where no device
/// is known yet.
pub type On<'a> = Option<(&'a Snapshot, DeviceIdx)>;

/// The value a full bar stands for.
#[derive(Clone, Copy, Debug)]
pub enum BarMax {
    /// Always this value, for example 100 for a percent.
    Fixed(f32),
    /// A value from the device. A device that does not report it shows no bar.
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
    /// `YScale::StickyUp` with no hardware maximum.
    StickyUpOpen,
    /// `YScale::StickyTemp`.
    StickyTemp,
    /// The CPU page's Core clocks chart: 0 to the highest core maximum (`ClockScale`).
    CoreClocks,
    /// The Network page's Throughput rule: the adapter's link speed (bits per second, the
    /// device fact) unless `network_dynamic_scale` is on, else auto (`throughput_scale`).
    Network(DeviceFact),
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
    /// The value and its total, "3.1 / 16.0 GiB" (the UI-PRF-017 VRAM row). The total is the
    /// bar's denominator, else the scale's total. Just the value when the device does not
    /// report the total.
    OfTotal,
    /// Read and write of a pair, "R 1.2 MB/s  W 340 KB/s".
    ReadWrite,
    /// Download and upload of a pair, "↓ 1.4 Mbps  ↑ 220 kbps".
    DownUp,
    /// The metric has no value form (Core clocks).
    None,
}

/// The lines a metric's chart draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lines {
    /// One series (phase 1).
    One,
    /// Two lines in one chart; the second is dashed when `second_dashed`.
    Pair { second_dashed: bool },
    /// Several series stacked (memory composition).
    Stacked,
    /// One line per physical core (Core clocks).
    PerCore,
    /// No series: the value comes from the device record only, so there is no chart and there
    /// are no stats (disk temperature).
    NoSeries,
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
    pub lines: Lines,
    /// The value and the bar use the core smoothing of UI-PRF-018 (`overview_core_smoothing`)
    /// and the chart plots 1-second means, as the core rows and their sparklines do. Only the
    /// logical CPU's `load` is smoothed.
    pub smoothed: bool,
}

/// A value when it is shown: valid or stale.
fn shown<T: Copy + Default>(m: Metric<T>) -> Option<T> {
    matches!(m.validity, Validity::Valid | Validity::Stale).then_some(m.value)
}

/// A fact of the GPU record of `idx`.
fn gpu(snap: &Snapshot, idx: DeviceIdx, f: impl FnOnce(&GpuTotals) -> Option<f32>) -> Option<f32> {
    gpu_totals(snap, idx).and_then(f)
}

fn gpu_mem_total(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    gpu(snap, idx, |t| shown(t.mem_total).map(|v| v as f32))
}

fn gpu_clock_gfx_max(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    gpu(snap, idx, |t| t.info.clock_gfx_max_mhz.map(|v| v as f32))
}

fn gpu_clock_mem_max(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    gpu(snap, idx, |t| t.info.clock_mem_max_mhz.map(|v| v as f32))
}

/// The enforced power limit, else the highest limit the card accepts (the GPU page's Power
/// chart uses the same).
fn gpu_power_max(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    gpu(snap, idx, |t| {
        shown(t.power_limit_w).or(t.info.power_max_limit_w)
    })
}

fn gpu_slowdown_temp(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    gpu(snap, idx, |t| t.info.slowdown_temp_c)
}

/// The processor's maximum clock from the inventory, as the CPU page's Clock graph uses it.
fn cpu_max_mhz(snap: &Snapshot, _: DeviceIdx) -> Option<f32> {
    let max = snap.inventory.processor.max_mhz;
    (max.validity == Validity::Valid).then_some(max.value as f32)
}

/// The CPU temperature sensor's `hot_c`.
fn cpu_hot(snap: &Snapshot, _: DeviceIdx) -> Option<f32> {
    cpu_hot_c(&snap.devices)
}

/// A memory total, while the memory record is shown.
fn mem_fact(
    snap: &Snapshot,
    f: impl FnOnce(&tl_model::system::MemoryTotals) -> u64,
) -> Option<f32> {
    let m = &snap.system.memory;
    matches!(m.validity, Validity::Valid | Validity::Stale).then(|| f(m) as f32)
}

fn mem_total(snap: &Snapshot, _: DeviceIdx) -> Option<f32> {
    mem_fact(snap, |m| m.total)
}

fn swap_total(snap: &Snapshot, _: DeviceIdx) -> Option<f32> {
    mem_fact(snap, |m| m.swap_total)
}

fn commit_limit(snap: &Snapshot, _: DeviceIdx) -> Option<f32> {
    mem_fact(snap, |m| m.commit_limit)
}

/// Bits per megabit.
const BITS_PER_MBIT: f32 = 1_000_000.0;

/// An adapter's link speed in bits per second, when sysfs reports one above 0. `None` for the
/// network aggregate, which has no record.
fn link_speed_bits(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    snap.system
        .nets
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, n)| n.link_speed_mbps)
        .filter(|m| m.validity == Validity::Valid && m.value > 0)
        .map(|m| m.value as f32 * BITS_PER_MBIT)
}

/// An adapter's link speed in bytes per second, the unit of its rates.
fn link_speed_bytes(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    link_speed_bits(snap, idx).map(|bits| bits / 8.0)
}

/// 0 to 100.
const PERCENT_SCALE: ScaleRule = ScaleRule::Fixed {
    min: 0.0,
    max: 100.0,
};

/// A percent gauge: every GPU engine (`engine:<key>`), CPU `total`, `kernel` and `pressure`,
/// memory `pressure`, disk `active`.
pub const PERCENT: MetricMeta = MetricMeta {
    unit: Unit::Percent,
    format: ValueFormat::Percent,
    value_text: ValueText::Plain,
    scale: PERCENT_SCALE,
    kind: MetricKind::Gauge,
    bar: BarMax::Fixed(100.0),
    stats: StatsRule::MinAvgPeak,
    lines: Lines::One,
    smoothed: false,
};

/// Every GPU engine: `engine:<key>` for each key of `GpuEngine::key`.
pub const GPU_ENGINE: MetricMeta = PERCENT;

/// `vram`: VRAM in use, of the card's total.
pub const GPU_VRAM: MetricMeta = MetricMeta {
    unit: Unit::Bytes,
    format: ValueFormat::Bytes,
    value_text: ValueText::OfTotal,
    scale: ScaleRule::FixedTotal(gpu_mem_total),
    bar: BarMax::Total(gpu_mem_total),
    ..PERCENT
};

/// `clock_gfx`: the graphics clock.
pub const GPU_CLOCK_GFX: MetricMeta = MetricMeta {
    unit: Unit::Megahertz,
    format: ValueFormat::Mhz,
    scale: ScaleRule::StickyUp(gpu_clock_gfx_max),
    bar: BarMax::Total(gpu_clock_gfx_max),
    ..PERCENT
};

/// `clock_mem`: the memory clock.
pub const GPU_CLOCK_MEM: MetricMeta = MetricMeta {
    unit: Unit::Megahertz,
    format: ValueFormat::Mhz,
    scale: ScaleRule::StickyUp(gpu_clock_mem_max),
    bar: BarMax::Total(gpu_clock_mem_max),
    ..PERCENT
};

/// `power` on a GPU: the power draw.
pub const GPU_POWER: MetricMeta = MetricMeta {
    unit: Unit::Watts,
    format: ValueFormat::Watts,
    scale: ScaleRule::StickyUp(gpu_power_max),
    bar: BarMax::Total(gpu_power_max),
    ..PERCENT
};

/// `temp` on a GPU: the GPU temperature.
pub const GPU_TEMP: MetricMeta = MetricMeta {
    unit: Unit::Celsius,
    format: ValueFormat::Celsius,
    scale: ScaleRule::StickyTemp,
    bar: BarMax::Total(gpu_slowdown_temp),
    ..PERCENT
};

/// `load` of a logical CPU: its percent, smoothed as the core rows are.
pub const CORE_LOAD: MetricMeta = MetricMeta {
    smoothed: true,
    ..PERCENT
};

/// `freq` of the CPU or a logical CPU: the clock, on the CPU page's Clock graph scale.
pub const CPU_FREQ: MetricMeta = MetricMeta {
    unit: Unit::Megahertz,
    format: ValueFormat::Mhz,
    scale: ScaleRule::StickyUp(cpu_max_mhz),
    bar: BarMax::Total(cpu_max_mhz),
    ..PERCENT
};

/// `temp` of the CPU.
pub const CPU_TEMP: MetricMeta = MetricMeta {
    unit: Unit::Celsius,
    format: ValueFormat::Celsius,
    scale: ScaleRule::StickyTemp,
    bar: BarMax::Total(cpu_hot),
    ..PERCENT
};

/// `power` and `power_cores` of the CPU: RAPL package and core power. No bar.
pub const CPU_POWER: MetricMeta = MetricMeta {
    unit: Unit::Watts,
    format: ValueFormat::Watts,
    scale: ScaleRule::StickyUpOpen,
    bar: BarMax::None,
    ..PERCENT
};

/// `power_draw` of the CPU: package power filled and core power dashed in one chart (the
/// UI-PRF-019 strip). The value is the package power.
pub const CPU_POWER_DRAW: MetricMeta = MetricMeta {
    stats: StatsRule::None,
    lines: Lines::Pair {
        second_dashed: true,
    },
    ..CPU_POWER
};

/// `core_clocks` of the CPU: one line per physical core, as the CPU page's Core clocks chart.
/// It offers a chart and its name only.
pub const CORE_CLOCKS: MetricMeta = MetricMeta {
    unit: Unit::Megahertz,
    format: ValueFormat::Mhz,
    value_text: ValueText::None,
    scale: ScaleRule::CoreClocks,
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::PerCore,
    ..PERCENT
};

/// `used`, `cached` and `available`: a part of memory, of the total.
pub const MEM_PART: MetricMeta = MetricMeta {
    unit: Unit::Bytes,
    format: ValueFormat::Bytes,
    value_text: ValueText::OfTotal,
    scale: ScaleRule::FixedTotal(mem_total),
    bar: BarMax::Total(mem_total),
    ..PERCENT
};

/// `swap`: swap in use, of the swap total. "No swap" without swap.
pub const MEM_SWAP: MetricMeta = MetricMeta {
    scale: ScaleRule::FixedTotal(swap_total),
    bar: BarMax::Total(swap_total),
    ..MEM_PART
};

/// `commit`: committed memory, of the commit limit. The bar stops at full; the value can pass
/// the limit.
pub const MEM_COMMIT: MetricMeta = MetricMeta {
    scale: ScaleRule::StickyUp(commit_limit),
    bar: BarMax::Total(commit_limit),
    ..MEM_PART
};

/// `composition`: in use, cached and available stacked. The value is in use, of the total.
pub const MEM_COMPOSITION: MetricMeta = MetricMeta {
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::Stacked,
    ..MEM_PART
};

/// `read` and `write` of a disk or of every disk.
pub const DISK_RATE: MetricMeta = MetricMeta {
    unit: Unit::BytesPerSecond,
    format: ValueFormat::RateIec,
    scale: ScaleRule::StickyUpOpen,
    bar: BarMax::None,
    ..PERCENT
};

/// `io` of a disk or of every disk: read and write, two lines on one scale.
pub const DISK_IO: MetricMeta = MetricMeta {
    value_text: ValueText::ReadWrite,
    stats: StatsRule::None,
    lines: Lines::Pair {
        second_dashed: false,
    },
    ..DISK_RATE
};

/// `temp` of a disk: from its record (hwmon, else SMART). Value and name only.
pub const DISK_TEMP: MetricMeta = MetricMeta {
    unit: Unit::Celsius,
    format: ValueFormat::Celsius,
    scale: ScaleRule::StickyTemp,
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::NoSeries,
    ..PERCENT
};

/// `rx` and `tx` of an adapter or of every adapter. The bar is the adapter's link speed; the
/// aggregate has none.
pub const NET_RATE: MetricMeta = MetricMeta {
    unit: Unit::BytesPerSecond,
    format: ValueFormat::RateNetwork,
    scale: ScaleRule::Network(link_speed_bits),
    bar: BarMax::Total(link_speed_bytes),
    ..PERCENT
};

/// `traffic` of an adapter or of every adapter: download filled and upload dashed, one scale.
pub const NET_TRAFFIC: MetricMeta = MetricMeta {
    value_text: ValueText::DownUp,
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::Pair {
        second_dashed: true,
    },
    ..NET_RATE
};

/// A denominator that can fill a bar: finite and above 0.
fn usable(max: f32) -> Option<f32> {
    (max.is_finite() && max > 0.0).then_some(max)
}

/// The fact of `on`, when it is usable.
fn fact(f: DeviceFact, on: On<'_>) -> Option<f32> {
    on.and_then(|(snap, idx)| f(snap, idx)).and_then(usable)
}

/// How the network value and scale are shown: the unit setting and whether the scale follows
/// the traffic instead of the link speed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NetView {
    pub format: NetFormat,
    pub dynamic_scale: bool,
}

impl MetricMeta {
    /// The entry for `key` on a device of `kind`; `None` for a key this build does not know
    /// there, whose widget shows its unavailable state.
    pub fn of(kind: DeviceKind, key: &MetricKey) -> Option<&'static MetricMeta> {
        let k = key.as_str();
        let meta = match kind {
            DeviceKind::Gpu => {
                if key.gpu_engine().is_some() {
                    return Some(&GPU_ENGINE);
                }
                match k {
                    METRIC_VRAM => &GPU_VRAM,
                    METRIC_CLOCK_GFX => &GPU_CLOCK_GFX,
                    METRIC_CLOCK_MEM => &GPU_CLOCK_MEM,
                    METRIC_POWER => &GPU_POWER,
                    METRIC_TEMP => &GPU_TEMP,
                    _ => return None,
                }
            }
            DeviceKind::Cpu => match k {
                METRIC_CPU_TOTAL | METRIC_CPU_KERNEL | METRIC_PRESSURE => &PERCENT,
                METRIC_FREQ => &CPU_FREQ,
                METRIC_TEMP => &CPU_TEMP,
                METRIC_POWER | METRIC_POWER_CORES => &CPU_POWER,
                METRIC_POWER_DRAW => &CPU_POWER_DRAW,
                METRIC_CORE_CLOCKS => &CORE_CLOCKS,
                _ => return None,
            },
            DeviceKind::CpuLogical => match k {
                METRIC_LOAD => &CORE_LOAD,
                METRIC_FREQ => &CPU_FREQ,
                _ => return None,
            },
            DeviceKind::Memory => match k {
                METRIC_MEM_USED | METRIC_MEM_CACHED | METRIC_MEM_AVAILABLE => &MEM_PART,
                METRIC_MEM_SWAP => &MEM_SWAP,
                METRIC_MEM_COMMIT => &MEM_COMMIT,
                METRIC_PRESSURE => &PERCENT,
                METRIC_MEM_COMPOSITION => &MEM_COMPOSITION,
                _ => return None,
            },
            DeviceKind::Disk | DeviceKind::Disks => match k {
                METRIC_DISK_ACTIVE => &PERCENT,
                METRIC_DISK_READ | METRIC_DISK_WRITE => &DISK_RATE,
                METRIC_DISK_IO => &DISK_IO,
                METRIC_TEMP if kind == DeviceKind::Disk => &DISK_TEMP,
                _ => return None,
            },
            DeviceKind::Net | DeviceKind::Nets => match k {
                METRIC_NET_RX | METRIC_NET_TX => &NET_RATE,
                METRIC_NET_TRAFFIC => &NET_TRAFFIC,
                _ => return None,
            },
            DeviceKind::Other => return None,
        };
        Some(meta)
    }

    /// Whether the chart draws two or more series.
    pub fn is_multi(&self) -> bool {
        matches!(
            self.lines,
            Lines::Pair { .. } | Lines::Stacked | Lines::PerCore
        )
    }

    /// Whether the metric has a series to chart and to take stats from.
    fn has_series(&self) -> bool {
        self.lines != Lines::NoSeries
    }

    /// Whether the metric can have a bar on some device. A counter and a multi-series metric
    /// never have one.
    fn has_bar(&self) -> bool {
        self.kind == MetricKind::Gauge && !self.is_multi() && !matches!(self.bar, BarMax::None)
    }

    /// Whether the metric has stats: a gauge of one series with a stats rule.
    fn has_stats(&self) -> bool {
        self.kind == MetricKind::Gauge
            && self.stats == StatsRule::MinAvgPeak
            && self.has_series()
            && !self.is_multi()
    }

    /// The forms the metric can offer on some device: the name always; a chart with a series;
    /// a value unless it has none; a bar with a denominator; stats with a stats rule.
    pub fn forms(&self) -> FormSet {
        let mut forms = FormSet::LABEL;
        forms.set(FormSet::CHART, self.has_series());
        forms.set(FormSet::VALUE, self.value_text != ValueText::None);
        forms.set(FormSet::BAR, self.has_bar());
        forms.set(FormSet::STATS, self.has_stats());
        forms
    }

    /// The forms the metric offers on the device `on`: [`Self::forms`] without the bar when
    /// the device does not report the bar's denominator. A form the metric does not offer is
    /// not listed in the widget menu.
    pub fn offered_forms(&self, on: On<'_>) -> FormSet {
        let mut forms = self.forms();
        if self.bar_max(on).is_none() {
            forms.remove(FormSet::BAR);
        }
        forms
    }

    /// The value a full bar stands for on the device `on`, `None` when the metric or the
    /// device has no bar.
    pub fn bar_max(&self, on: On<'_>) -> Option<f32> {
        if !self.has_bar() {
            return None;
        }
        match self.bar {
            BarMax::Fixed(max) => usable(max),
            BarMax::Total(f) => fact(f, on),
            BarMax::None => None,
        }
    }

    /// The total the [`ValueText::OfTotal`] text names: the bar's denominator, else the
    /// scale's total. `None` for the other value texts.
    pub fn value_total(&self, on: On<'_>) -> Option<f32> {
        if self.value_text != ValueText::OfTotal {
            return None;
        }
        match (self.bar, self.scale) {
            (BarMax::Total(f), _) | (_, ScaleRule::FixedTotal(f)) => fact(f, on),
            (BarMax::Fixed(max), _) => usable(max),
            _ => None,
        }
    }

    /// How full the bar is for `value`, 0 to 1; `None` without a bar.
    pub fn bar_fraction(&self, value: f32, on: On<'_>) -> Option<f32> {
        let max = self.bar_max(on)?;
        Some(if value.is_finite() {
            (value / max).clamp(0.0, 1.0)
        } else {
            0.0
        })
    }

    /// The chart's Y axis on the device `on`. `net` is the network view the Network rule
    /// follows.
    pub fn y_scale(&self, on: On<'_>, net: NetView) -> YScale {
        match self.scale {
            ScaleRule::Fixed { min, max } => YScale::Fixed { min, max },
            ScaleRule::FixedTotal(f) => match fact(f, on) {
                Some(total) => YScale::FixedTotal(total),
                None => YScale::StickyUp { hw_max: None },
            },
            ScaleRule::StickyUp(f) => YScale::StickyUp {
                hw_max: fact(f, on),
            },
            ScaleRule::StickyUpOpen => YScale::StickyUp { hw_max: None },
            ScaleRule::StickyTemp => YScale::StickyTemp,
            ScaleRule::CoreClocks => match on {
                Some((snap, _)) => ClockScale::read(snap).y(),
                None => YScale::StickyUp { hw_max: None },
            },
            ScaleRule::Network(f) => {
                let unit = net.format.unit;
                throughput_scale(net.dynamic_scale, fact(f, on), unit)
            }
        }
    }

    /// Appends the value form's text: `value` in the metric's format; for
    /// [`ValueText::OfTotal`] " / " and `total` when known; for a pair, both values with
    /// their marks ("R 1.2 MB/s  W 340 KB/s", "↓ 1.4 Mbps  ↑ 220 kbps"), `second` being the
    /// second line's value. Nothing for [`ValueText::None`].
    pub fn write_value(
        &self,
        value: f32,
        second: Option<f32>,
        total: Option<f32>,
        net: NetFormat,
        out: &mut String,
    ) {
        let one = |v: f32, out: &mut String| format_value(self.format, v, net, out);
        match self.value_text {
            ValueText::Plain => one(value, out),
            ValueText::OfTotal => {
                one(value, out);
                if let Some(total) = total {
                    out.push_str(" / ");
                    one(total, out);
                }
            }
            ValueText::ReadWrite | ValueText::DownUp => {
                let (a, b) = if self.value_text == ValueText::ReadWrite {
                    ("R ", "  W ")
                } else {
                    ("↓ ", "  ↑ ")
                };
                out.push_str(a);
                one(value, out);
                if let Some(second) = second {
                    out.push_str(b);
                    one(second, out);
                }
            }
            ValueText::None => {}
        }
    }

    /// The stats of series `id` over `range` by the metric's stats rule. `MinAvgPeak` takes
    /// the figures the box titles use (UI-PRF-055): the valid live samples for 1 min and 5 min,
    /// the stored points (min of the mins, mean of the means, max of the maxes) for 1 h, 24 h
    /// and 7 d. `None` without a valid sample in the range, and always for a metric without
    /// stats, so the stats text stays empty.
    pub fn stats(&self, snap: &Snapshot, id: SeriesId, range: OverviewRange) -> Option<Figures> {
        self.stats_minus(snap, id, &[], range)
    }

    /// [`Self::stats`] of series `id` minus the sum of `minus` at each time (the network
    /// aggregate without the hidden kinds, as the Network box title).
    pub fn stats_minus(
        &self,
        snap: &Snapshot,
        id: SeriesId,
        minus: &[SeriesId],
        range: OverviewRange,
    ) -> Option<Figures> {
        if !self.has_stats() {
            return None;
        }
        if minus.is_empty() {
            figures(snap, &[id], range)
        } else {
            figures_minus(snap, &[(id, minus)], range)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
    use tl_model::ids::{DeviceIdx, MetricId};
    use tl_model::layout_boxes::{BOX_METRICS, metric_series};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::snapshot::SystemSnapshot;
    use tl_model::snapshot::{DeviceHistory, HistPoint, SeriesSet};
    use tl_model::system::{GpuEngine, GpuStaticInfo};
    use tl_model::system::{MemoryTotals, NetTotals};
    use tl_model::table::Total;
    use tl_model::time::BootId;
    use tl_model::units;

    use super::*;

    const S: u64 = 1_000_000_000;

    /// The device the records below are on.
    const DEV: DeviceIdx = DeviceIdx(1);

    /// The metadata of a GPU metric key.
    fn gpu_meta(key: &MetricKey) -> Option<&'static MetricMeta> {
        MetricMeta::of(DeviceKind::Gpu, key)
    }

    /// A snapshot whose system record `edit` changes.
    fn snap_with(edit: impl FnOnce(&mut SystemSnapshot)) -> Snapshot {
        let mut snap = Snapshot::empty(BootId(0), 0);
        let mut sys = (*snap.system).clone();
        edit(&mut sys);
        snap.system = Arc::new(sys);
        snap
    }

    /// A snapshot with the GPU record `t` on [`DEV`].
    fn with_gpu(t: GpuTotals) -> Snapshot {
        snap_with(|s| s.gpus = Arc::from([(DEV, t)]))
    }

    fn net() -> NetView {
        NetView::default()
    }

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
            let meta = gpu_meta(&key).unwrap_or_else(|| panic!("{key:?} has metadata"));
            assert_eq!(meta.kind, MetricKind::Gauge, "{key:?}");
            assert_eq!(meta.stats, StatsRule::MinAvgPeak, "{key:?}");
            let full = with_gpu(full_gpu());
            let forms = meta.offered_forms(Some((&full, DEV)));
            assert_eq!(
                forms,
                FormSet::all(),
                "{key:?} offers every form on a full record"
            );
        }
        assert!(gpu_meta(&MetricKey::new("engine:nope")).is_none());
        assert!(gpu_meta(&MetricKey::new("fan")).is_none());
    }

    #[test]
    fn table_matches_the_adr() {
        let engine = gpu_meta(&MetricKey::engine(GpuEngine::Enc)).expect("engine");
        assert_eq!(engine.unit, Unit::Percent);
        assert_eq!(engine.format, ValueFormat::Percent);
        assert_eq!(
            engine.y_scale(None, net()),
            YScale::Fixed {
                min: 0.0,
                max: 100.0
            }
        );
        assert_eq!(engine.bar_max(None), Some(100.0));

        let snap = with_gpu(full_gpu());
        let gpu = Some((&snap, DEV));
        let at = |key: &str| gpu_meta(&MetricKey::new(key)).expect("known key");
        let vram = at(METRIC_VRAM);
        assert_eq!(vram.format, ValueFormat::Bytes);
        assert_eq!(
            vram.y_scale(gpu, net()),
            YScale::FixedTotal(16.0 * 1024.0 * 1024.0 * 1024.0)
        );
        assert_eq!(vram.bar_max(gpu), Some(16.0 * 1024.0 * 1024.0 * 1024.0));
        assert_eq!(
            at(METRIC_CLOCK_GFX).y_scale(gpu, net()),
            YScale::StickyUp {
                hw_max: Some(2520.0)
            }
        );
        assert_eq!(at(METRIC_CLOCK_GFX).bar_max(gpu), Some(2520.0));
        assert_eq!(at(METRIC_CLOCK_MEM).bar_max(gpu), Some(10501.0));
        assert_eq!(at(METRIC_CLOCK_MEM).format, ValueFormat::Mhz);
        assert_eq!(at(METRIC_POWER).format, ValueFormat::Watts);
        assert_eq!(at(METRIC_POWER).bar_max(gpu), Some(320.0));
        assert_eq!(
            at(METRIC_POWER).y_scale(gpu, net()),
            YScale::StickyUp {
                hw_max: Some(320.0)
            }
        );
        assert_eq!(at(METRIC_TEMP).format, ValueFormat::Celsius);
        assert_eq!(at(METRIC_TEMP).y_scale(gpu, net()), YScale::StickyTemp);
        assert_eq!(at(METRIC_TEMP).bar_max(gpu), Some(90.0));
    }

    #[test]
    fn power_bar_falls_back_to_the_highest_limit() {
        let mut t = full_gpu();
        t.power_limit_w = Metric::with(Validity::Unsupported);
        let snap = with_gpu(t);
        assert_eq!(
            gpu_meta(&MetricKey::new(METRIC_POWER))
                .expect("power")
                .bar_max(Some((&snap, DEV))),
            Some(450.0)
        );
    }

    #[test]
    fn no_denominator_offers_no_bar() {
        let snap = with_gpu(bare_gpu());
        let gpu = Some((&snap, DEV));
        for key in [
            METRIC_VRAM,
            METRIC_CLOCK_GFX,
            METRIC_CLOCK_MEM,
            METRIC_POWER,
            METRIC_TEMP,
        ] {
            let meta = gpu_meta(&MetricKey::new(key)).expect("known key");
            assert!(meta.forms().contains(FormSet::BAR), "{key} can have a bar");
            for record in [gpu, None] {
                assert_eq!(meta.bar_max(record), None, "{key}");
                assert_eq!(meta.bar_fraction(50.0, record), None, "{key}");
                let forms = meta.offered_forms(record);
                assert!(!forms.contains(FormSet::BAR), "{key} offers no bar");
                assert!(forms.contains(FormSet::STATS | FormSet::CHART | FormSet::VALUE));
            }
        }
        // VRAM without a total keeps a chart, on a scale that grows with the values.
        let vram = gpu_meta(&MetricKey::new(METRIC_VRAM)).expect("vram");
        assert_eq!(vram.y_scale(gpu, net()), YScale::StickyUp { hw_max: None });
        // An engine's bar needs no device fact.
        let engine = gpu_meta(&MetricKey::engine(GpuEngine::Gpu)).expect("engine");
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
        let snap = with_gpu(full_gpu());
        let gpu = Some((&snap, DEV));
        let text = |meta: &MetricMeta, v: f32, on: On<'_>| {
            let mut out = String::new();
            meta.write_value(
                v,
                None,
                meta.value_total(on),
                NetFormat::default(),
                &mut out,
            );
            out
        };
        let vram = gpu_meta(&MetricKey::new(METRIC_VRAM)).expect("vram");
        let used = 3.1 * 1024.0 * 1024.0 * 1024.0;
        let mut expected = String::new();
        units::bytes_of(&mut expected, used as u64, 16 << 30);
        assert_eq!(text(vram, used, gpu), expected);
        assert!(expected.contains(" / "), "{expected}");
        let mut alone = String::new();
        units::bytes(&mut alone, used as u64);
        assert_eq!(text(vram, used, None), alone, "no total: the value alone");

        let mut pct = String::new();
        format_value(ValueFormat::Percent, 42.0, NetFormat::default(), &mut pct);
        assert_eq!(text(&GPU_ENGINE, 42.0, None), pct);
        let mut mhz = String::new();
        format_value(ValueFormat::Mhz, 1755.0, NetFormat::default(), &mut mhz);
        let clock = gpu_meta(&MetricKey::new(METRIC_CLOCK_GFX)).expect("clock");
        assert_eq!(text(clock, 1755.0, gpu), mhz, "a clock shows no total");
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
        let meta = gpu_meta(&MetricKey::engine(GpuEngine::Gpu)).expect("engine");
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
        let meta = gpu_meta(&MetricKey::new(METRIC_POWER)).expect("power");
        let f = meta.stats(&snap, id, OverviewRange::H1).expect("stats");
        assert_eq!((f.min, f.avg, f.peak), (30.0, 100.0, 300.0));
        // A history for another range gives no stats.
        assert_eq!(meta.stats(&snap, id, OverviewRange::H24), None);
    }

    // ---- Phase 2: CPU, memory, disk and network (ADR-019 "Phase 2") ----

    #[test]
    fn every_phase_2_key_has_metadata() {
        for (kind, key) in BOX_METRICS {
            let key = MetricKey::new(key);
            let meta = MetricMeta::of(kind, &key)
                .unwrap_or_else(|| panic!("{kind:?} {key:?} has metadata"));
            let series = metric_series(kind, &key).expect("series");
            // The lines follow the series the key names.
            assert_eq!(meta.is_multi(), series.is_multi(), "{kind:?} {key:?}");
            assert_eq!(
                meta.lines == Lines::NoSeries,
                series.first().is_none(),
                "{kind:?} {key:?}"
            );
            let forms = meta.forms();
            assert!(forms.contains(FormSet::LABEL), "{kind:?} {key:?}");
            if meta.is_multi() {
                assert!(
                    !forms.intersects(FormSet::BAR | FormSet::STATS),
                    "{kind:?} {key:?}"
                );
                assert!(forms.contains(FormSet::CHART), "{kind:?} {key:?}");
            }
        }
        // Keys of another kind and unknown keys have none.
        assert!(MetricMeta::of(DeviceKind::Disks, &MetricKey::new(METRIC_TEMP)).is_none());
        assert!(MetricMeta::of(DeviceKind::Memory, &MetricKey::new(METRIC_LOAD)).is_none());
        assert!(MetricMeta::of(DeviceKind::Other, &MetricKey::new(METRIC_NET_RX)).is_none());
        assert!(MetricMeta::of(DeviceKind::Cpu, &MetricKey::new(METRIC_VRAM)).is_none());
        // `temp` and `power` mean the CPU or the GPU metric by the device kind.
        let temp = MetricKey::new(METRIC_TEMP);
        assert!(matches!(
            MetricMeta::of(DeviceKind::Cpu, &temp).map(|m| m.bar),
            Some(BarMax::Total(_))
        ));
        let power = MetricKey::new(METRIC_POWER);
        assert!(matches!(
            MetricMeta::of(DeviceKind::Cpu, &power).map(|m| m.bar),
            Some(BarMax::None)
        ));
        assert!(matches!(
            MetricMeta::of(DeviceKind::Gpu, &power).map(|m| m.bar),
            Some(BarMax::Total(_))
        ));
    }

    #[test]
    fn phase_2_forms() {
        let at = |kind, key: &str| MetricMeta::of(kind, &MetricKey::new(key)).expect("known");
        let all = FormSet::all();
        assert_eq!(at(DeviceKind::Cpu, METRIC_CPU_TOTAL).forms(), all);
        assert_eq!(at(DeviceKind::Disk, METRIC_DISK_ACTIVE).forms(), all);
        assert_eq!(at(DeviceKind::Memory, METRIC_MEM_SWAP).forms(), all);
        // Power has no bar; disk rates have none either.
        assert_eq!(
            at(DeviceKind::Cpu, METRIC_POWER).forms(),
            all - FormSet::BAR
        );
        assert_eq!(
            at(DeviceKind::Disks, METRIC_DISK_READ).forms(),
            all - FormSet::BAR
        );
        // A multi-series metric offers chart, value and name.
        let cvl = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        for (kind, key) in [
            (DeviceKind::Cpu, METRIC_POWER_DRAW),
            (DeviceKind::Memory, METRIC_MEM_COMPOSITION),
            (DeviceKind::Disk, METRIC_DISK_IO),
            (DeviceKind::Nets, METRIC_NET_TRAFFIC),
        ] {
            assert_eq!(at(kind, key).forms(), cvl, "{kind:?} {key}");
        }
        // Core clocks: chart and name. Disk temperature: value and name.
        assert_eq!(
            at(DeviceKind::Cpu, METRIC_CORE_CLOCKS).forms(),
            FormSet::CHART | FormSet::LABEL
        );
        assert_eq!(
            at(DeviceKind::Disk, METRIC_TEMP).forms(),
            FormSet::VALUE | FormSet::LABEL
        );
        // Only the logical CPU's load is smoothed.
        for (kind, key) in BOX_METRICS {
            let smoothed = at(kind, key).smoothed;
            assert_eq!(smoothed, key == METRIC_LOAD, "{kind:?} {key}");
        }
    }

    #[test]
    fn memory_facts() {
        let gib = 1024.0 * 1024.0 * 1024.0;
        let snap = snap_with(|s| {
            s.memory = MemoryTotals {
                validity: Validity::Valid,
                total: 32 << 30,
                swap_total: 8 << 30,
                commit_limit: 40 << 30,
                ..MemoryTotals::default()
            };
        });
        let on = Some((&snap, DeviceIdx(2)));
        let at =
            |key: &str| MetricMeta::of(DeviceKind::Memory, &MetricKey::new(key)).expect("known");
        let used = at(METRIC_MEM_USED);
        assert_eq!(used.bar_max(on), Some(32.0 * gib));
        assert_eq!(used.y_scale(on, net()), YScale::FixedTotal(32.0 * gib));
        assert_eq!(at(METRIC_MEM_SWAP).bar_max(on), Some(8.0 * gib));
        let commit = at(METRIC_MEM_COMMIT);
        assert_eq!(commit.bar_max(on), Some(40.0 * gib));
        assert_eq!(
            commit.y_scale(on, net()),
            YScale::StickyUp {
                hw_max: Some(40.0 * gib)
            }
        );
        // The bar stops at full; the value can pass the limit.
        assert_eq!(commit.bar_fraction(50.0 * gib, on), Some(1.0));
        let mut out = String::new();
        let total = commit.value_total(on);
        commit.write_value(50.0 * gib, None, total, NetFormat::default(), &mut out);
        let mut expected = String::new();
        units::bytes_of(&mut expected, 50 << 30, 40 << 30);
        assert_eq!(out, expected);
        // Composition has no bar, but its value names the total.
        let comp = at(METRIC_MEM_COMPOSITION);
        assert_eq!(comp.bar_max(on), None);
        assert_eq!(comp.value_total(on), Some(32.0 * gib));
        // No swap: no bar.
        let no_swap = snap_with(|s| {
            s.memory = MemoryTotals {
                validity: Validity::Valid,
                total: 32 << 30,
                ..MemoryTotals::default()
            };
        });
        let swap = at(METRIC_MEM_SWAP);
        assert_eq!(
            swap.offered_forms(Some((&no_swap, DeviceIdx(2)))),
            FormSet::all() - FormSet::BAR
        );
        // A memory record that is not shown gives no total.
        let missing = Snapshot::empty(BootId(0), 0);
        assert_eq!(used.bar_max(Some((&missing, DeviceIdx(2)))), None);
    }

    #[test]
    fn cpu_facts() {
        let mut snap = Snapshot::empty(BootId(0), 0);
        let mut inv = (*snap.inventory).clone();
        inv.processor.max_mhz = Total {
            value: 5200,
            validity: Validity::Valid,
        };
        snap.inventory = Arc::new(inv);
        let on = Some((&snap, DeviceIdx(0)));
        let freq =
            MetricMeta::of(DeviceKind::CpuLogical, &MetricKey::new(METRIC_FREQ)).expect("freq");
        assert_eq!(freq.bar_max(on), Some(5200.0));
        assert_eq!(
            freq.y_scale(on, net()),
            YScale::StickyUp {
                hw_max: Some(5200.0)
            }
        );
        // No hot sensor: the CPU temperature has no bar.
        let temp = MetricMeta::of(DeviceKind::Cpu, &MetricKey::new(METRIC_TEMP)).expect("temp");
        assert_eq!(temp.bar_max(on), None);
        assert_eq!(temp.y_scale(on, net()), YScale::StickyTemp);
        // Core clocks: 0 to the highest maximum.
        let clocks =
            MetricMeta::of(DeviceKind::Cpu, &MetricKey::new(METRIC_CORE_CLOCKS)).expect("clocks");
        assert_eq!(
            clocks.y_scale(on, net()),
            YScale::Fixed {
                min: 0.0,
                max: 5200.0
            }
        );
        let mut out = String::new();
        clocks.write_value(3000.0, None, None, NetFormat::default(), &mut out);
        assert!(out.is_empty(), "Core clocks has no value");
    }

    fn adapter(speed_mbps: Metric<u32>) -> NetTotals {
        NetTotals {
            device: DEV,
            rx_bps: Metric::valid(125_000.0),
            tx_bps: Metric::valid(0.0),
            rx_total: 0,
            tx_total: 0,
            link_speed_mbps: speed_mbps,
            state: tl_model::system::LinkState::Up,
        }
    }

    #[test]
    fn network_facts() {
        let snap = snap_with(|s| s.nets = Arc::from([(DEV, adapter(Metric::valid(1000)))]));
        let on = Some((&snap, DEV));
        let rx = MetricMeta::of(DeviceKind::Net, &MetricKey::new(METRIC_NET_RX)).expect("rx");
        // The link speed in bytes per second fills the bar; bits per second set the scale.
        assert_eq!(rx.bar_max(on), Some(125_000_000.0));
        assert_eq!(rx.bar_fraction(125_000.0, on), Some(0.001));
        assert_eq!(rx.y_scale(on, net()), YScale::LinkSpeed(1e9));
        let dynamic = NetView {
            dynamic_scale: true,
            ..NetView::default()
        };
        assert_eq!(rx.y_scale(on, dynamic), YScale::AutoRate { iec: false });
        // No link speed, or the aggregate (no record): no bar, an auto scale.
        let unknown = snap_with(|s| {
            s.nets = Arc::from([(DEV, adapter(Metric::with(Validity::Unsupported)))]);
        });
        assert_eq!(rx.bar_max(Some((&unknown, DEV))), None);
        assert_eq!(rx.bar_max(Some((&snap, DeviceIdx(9)))), None);
        let nets = MetricMeta::of(DeviceKind::Nets, &MetricKey::new(METRIC_NET_RX)).expect("rx");
        assert!(
            !nets
                .offered_forms(Some((&snap, DeviceIdx(9))))
                .contains(FormSet::BAR)
        );
    }

    #[test]
    fn pair_value_text() {
        let text = |kind, key: &str, a: f32, b: f32| {
            let meta = MetricMeta::of(kind, &MetricKey::new(key)).expect("known");
            let mut out = String::new();
            meta.write_value(a, Some(b), None, NetFormat::default(), &mut out);
            out
        };
        let rate = |format, v: f32| {
            let mut out = String::new();
            format_value(format, v, NetFormat::default(), &mut out);
            out
        };
        assert_eq!(
            text(DeviceKind::Disks, METRIC_DISK_IO, 1_200_000.0, 340_000.0),
            format!(
                "R {}  W {}",
                rate(ValueFormat::RateIec, 1_200_000.0),
                rate(ValueFormat::RateIec, 340_000.0)
            )
        );
        assert_eq!(
            text(DeviceKind::Nets, METRIC_NET_TRAFFIC, 175_000.0, 27_500.0),
            format!(
                "↓ {}  ↑ {}",
                rate(ValueFormat::RateNetwork, 175_000.0),
                rate(ValueFormat::RateNetwork, 27_500.0)
            )
        );
        // Power draw shows the package watts.
        assert_eq!(
            text(DeviceKind::Cpu, METRIC_POWER_DRAW, 35.2, 20.0),
            rate(ValueFormat::Watts, 35.2)
        );
    }
}
