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
//! CPU, a GPU, a fan or a battery metric. Phase 3 adds the keys the device pages show
//! (ADR-019 "Metric metadata added in phase 3"), each with its menu label and the value text a
//! page container shows ([`MetricMeta::menu_label`], [`MetricMeta::page_value`]). The denominators and scales that depend on the device read the snapshot
//! ([`DeviceFact`]): the GPU record, the memory totals, an adapter's link speed, the CPU
//! inventory. This module does no drawing and keeps no state.

use tl_model::ids::{DeviceIdx, SeriesId};
use tl_model::snapshot::Snapshot;
use tl_model::system::{GpuTotals, Metric};
use tl_model::validity::Validity;
use tl_ui_settings::layout::{
    FormSet, METRIC_CLOCK_GFX, METRIC_CLOCK_MEM, METRIC_POWER, METRIC_TEMP, METRIC_VRAM, MetricKey,
};
use tl_ui_settings::layout_boxes::{
    DeviceKind, METRIC_CORE_CLOCKS, METRIC_CPU_KERNEL, METRIC_CPU_TOTAL, METRIC_DISK_ACTIVE,
    METRIC_DISK_IO, METRIC_DISK_READ, METRIC_DISK_WRITE, METRIC_FREQ, METRIC_LOAD,
    METRIC_MEM_AVAILABLE, METRIC_MEM_CACHED, METRIC_MEM_COMMIT, METRIC_MEM_COMPOSITION,
    METRIC_MEM_SWAP, METRIC_MEM_USED, METRIC_NET_RX, METRIC_NET_TRAFFIC, METRIC_NET_TX,
    METRIC_POWER_CORES, METRIC_POWER_DRAW, METRIC_PRESSURE,
};
use tl_ui_settings::layout_pages::{
    METRIC_CLOCK, METRIC_COMMIT_PCT, METRIC_CORES_OVERLAID, METRIC_CORES_STACKED, METRIC_DIRTY,
    METRIC_FAN, METRIC_FREQ_MAX, METRIC_GPU_CLOCKS, METRIC_GPU_MEMORY, METRIC_PERCENT,
    METRIC_PHYSICAL, METRIC_PWM, METRIC_RESPONSE, METRIC_RPM, METRIC_SIGNAL, METRIC_SPEED,
    METRIC_TOTAL_READ, METRIC_TOTAL_RX, METRIC_TOTAL_TX, METRIC_TOTAL_WRITTEN, METRIC_UTIL,
    METRIC_VRAM_SHARED,
};
use tl_ui_settings::settings::OverviewRange;

use crate::chart::labels::{NetFormat, format_value};
use crate::chart::{ValueFormat, YScale};
use crate::pages::performance::core::core_clocks::ClockScale;
use crate::pages::performance::core::gpu::gpu_totals;
use crate::pages::performance::core::scales::throughput_scale;
use crate::pages::performance::core::sensors::cpu_hot_c;
use crate::pages::performance::overview::range::{Figures, figures, figures_minus};

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
    Rpm,
    Milliseconds,
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
    /// `YScale::Commit` of the device fact, so the axis passes 100% of the limit when commit
    /// does; `YScale::StickyUp` with no maximum when the device does not report it.
    Commit(DeviceFact),
    /// `YScale::Stacked` of the present logical CPUs (UI-PRF-023 "All cores stacked").
    StackedLogical,
    /// `YScale::Symmetric`: battery power, charge above the zero line and discharge below.
    Symmetric,
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
    /// The value in the metric's format: "42.0%", "1755 MHz".
    Plain,
    /// A whole percent, cut down to the 1% step the core rows and the load bars show: "41%"
    /// for 41.7. The CPU, memory pressure and disk active time values (ADR-019 phase 2).
    WholePercent,
    /// The value and its total, "3.1 / 16.0 GiB" (the UI-PRF-017 VRAM row). The total is the
    /// bar's denominator, else the scale's total. Just the value when the device does not
    /// report the total.
    OfTotal,
    /// The value alone, "30.00 GiB", as the Memory box's rows show it; the total is still
    /// known (the "No swap" text reads it) but not written (tasklens-6swdr.12).
    Part,
    /// Read and write of a pair, "R 1.2 MB/s  W 340 KB/s".
    ReadWrite,
    /// Download and upload of a pair, "↓ 1.4 Mbps  ↑ 220 kbps".
    DownUp,
    /// The sum of a pair's two values, "12.3 MiB/s": a disk's read + write and an adapter's
    /// download + upload on a page (UI-PRF-029, UI-PRF-031).
    Sum,
    /// The value as a percent of its total, one decimal: commit of the commit limit, "69.0%".
    PercentOfTotal,
    /// A percent and the second value in RPM, "0% (0 RPM)": the GPU fan.
    PercentRpm,
    /// Milliseconds with two decimals, "0.42 ms", as the disk page's Details row.
    Milliseconds,
    /// The Wi-Fi signal, "-52 dBm (78%)", or "78%" when the adapter gives no dBm (the second
    /// value), as the adapter page's Details row.
    Signal,
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
    /// One line per present logical CPU, at alpha max(1/n, 0.08) and no fill, or stacked
    /// filled bands (UI-PRF-023 "All cores overlaid" and "All cores stacked").
    PerLogical { stacked: bool },
    /// The Physical memory chart (UI-PRF-026): in use filled, committed dashed, modified as a
    /// 1 px line at 0.6.
    Physical,
    /// The Fan speed chart (UI-PRF-034): RPM filled, PWM dashed on its own 0 to 100 axis.
    RpmPwm,
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
    /// The `en-US.ftl` key of the Source and Add Widget item text when it differs from the
    /// name, so two keys never share an item text (ADR-019 phase 3).
    pub menu_label: Option<&'static str>,
    /// The value text in a page container, when it differs from [`Self::value_text`] (the
    /// page format of ADR-019 phase 3 "Page style").
    pub page_value: Option<ValueText>,
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

fn gpu_shared_total(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    gpu(snap, idx, |t| shown(t.shared_total).map(|v| v as f32))
}

/// The summed dedicated memory of every GPU, the Aggregate graphics page's Memory total.
fn gpu_mem_sum(snap: &Snapshot, _: DeviceIdx) -> Option<f32> {
    let totals = snap
        .system
        .gpus
        .iter()
        .filter_map(|(_, t)| shown(t.mem_total));
    let mut sum = None;
    for v in totals {
        *sum.get_or_insert(0.0) += v as f32;
    }
    sum
}

/// A fan's `fan<n>_max` in RPM, when the chip reports it.
fn fan_max(snap: &Snapshot, idx: DeviceIdx) -> Option<f32> {
    snap.system
        .sensors
        .iter()
        .find(|(i, _)| *i == idx)
        .and_then(|(_, r)| r.fan_max_rpm)
        .map(|v| v as f32)
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
    menu_label: None,
    page_value: None,
};

/// Every GPU engine: `engine:<key>` for each key of `GpuEngine::key`. One decimal, as the GPU
/// box's spark rows and the GPU page show it.
pub const GPU_ENGINE: MetricMeta = MetricMeta {
    page_value: Some(ValueText::Plain),
    ..PERCENT
};

/// A percent gauge shown as a whole percent ("42%"): CPU `pressure` and memory `pressure`.
pub const WHOLE_PERCENT: MetricMeta = MetricMeta {
    value_text: ValueText::WholePercent,
    ..PERCENT
};

/// CPU `total` and `kernel`, disk `active`: a whole percent in a box, one decimal on a page
/// ("18.6%", UI-PRF-024, UI-PRF-029).
pub const PAGE_PERCENT: MetricMeta = MetricMeta {
    page_value: Some(ValueText::Plain),
    ..WHOLE_PERCENT
};

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

/// `power` on a GPU: the power draw. On a page with its limit, "31.9 / 300 W" (UI-PRF-032).
pub const GPU_POWER: MetricMeta = MetricMeta {
    unit: Unit::Watts,
    format: ValueFormat::Watts,
    scale: ScaleRule::StickyUp(gpu_power_max),
    bar: BarMax::Total(gpu_power_max),
    page_value: Some(ValueText::OfTotal),
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

/// `load` of a logical CPU: its percent, smoothed and shown as the core rows are ("41%").
pub const CORE_LOAD: MetricMeta = MetricMeta {
    smoothed: true,
    ..WHOLE_PERCENT
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

/// `used`, `cached` and `available`: a part of memory, of the total. The value is the part
/// alone ("30.00 GiB"), as the Memory box's rows show it.
pub const MEM_PART: MetricMeta = MetricMeta {
    unit: Unit::Bytes,
    format: ValueFormat::Bytes,
    value_text: ValueText::Part,
    scale: ScaleRule::FixedTotal(mem_total),
    bar: BarMax::Total(mem_total),
    ..PERCENT
};

/// `swap`: swap in use, of the swap total. "No swap" without swap. A page shows the bytes in
/// use, "1.20 GiB" (UI-PRF-026).
pub const MEM_SWAP: MetricMeta = MetricMeta {
    scale: ScaleRule::FixedTotal(swap_total),
    bar: BarMax::Total(swap_total),
    page_value: Some(ValueText::Part),
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
    value_text: ValueText::OfTotal,
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

/// `io` of a disk or of every disk: read filled and write dashed, two lines on one scale, as
/// the `disk_charts` cells.
pub const DISK_IO: MetricMeta = MetricMeta {
    value_text: ValueText::ReadWrite,
    page_value: Some(ValueText::Sum),
    stats: StatsRule::None,
    lines: Lines::Pair {
        second_dashed: true,
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

/// `traffic` of an adapter or of every adapter: download filled and upload as a lighter solid
/// line, one scale, as the `combined` chart.
pub const NET_TRAFFIC: MetricMeta = MetricMeta {
    value_text: ValueText::DownUp,
    page_value: Some(ValueText::Sum),
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::Pair {
        second_dashed: false,
    },
    ..NET_RATE
};

// ---- Phase 3: the keys the device pages add (ADR-019 "Metric metadata added in phase 3") ----

/// `cores_overlaid` of the CPU: every logical CPU's load, one line each. Chart and name only.
pub const CORES_OVERLAID: MetricMeta = MetricMeta {
    value_text: ValueText::None,
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::PerLogical { stacked: false },
    ..PERCENT
};

/// `cores_stacked` of the CPU: the same as stacked bands, on a `Stacked(n)` scale.
pub const CORES_STACKED: MetricMeta = MetricMeta {
    scale: ScaleRule::StackedLogical,
    lines: Lines::PerLogical { stacked: true },
    ..CORES_OVERLAID
};

/// `freq` and `freq_max` of the CPU: the speed and the fastest core's clock, "4631 MHz" on a
/// page too (UI-PRF-025).
pub const CPU_FREQ_PAGE: MetricMeta = MetricMeta {
    page_value: Some(ValueText::Plain),
    ..CPU_FREQ
};

/// `clock` of the CPU: the speed filled and the fastest core dashed (UI-PRF-025). The value is
/// the speed.
pub const CPU_CLOCK: MetricMeta = MetricMeta {
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::Pair {
        second_dashed: true,
    },
    ..CPU_FREQ_PAGE
};

/// `physical` of memory: in use, committed and modified in one chart (UI-PRF-026). The value
/// is in use alone, "34.68 GiB".
pub const MEM_PHYSICAL: MetricMeta = MetricMeta {
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::Physical,
    ..MEM_PART
};

/// `commit_pct` of memory: committed memory as a percent of the commit limit, "69.0%". The
/// axis passes 100% when commit does; the bar stops at full.
pub const MEM_COMMIT_PCT: MetricMeta = MetricMeta {
    value_text: ValueText::PercentOfTotal,
    scale: ScaleRule::Commit(commit_limit),
    menu_label: Some("perf-widget-menu-commit-pct"),
    ..MEM_COMMIT
};

/// `dirty` of memory: modified memory, "120.00 MiB".
pub const MEM_DIRTY: MetricMeta = MetricMeta {
    value_text: ValueText::Plain,
    scale: ScaleRule::StickyUpOpen,
    bar: BarMax::None,
    ..MEM_PART
};

/// `memory` of a GPU: dedicated memory filled and shared memory dashed, "2.80 GiB / 15.92 GiB".
pub const GPU_MEMORY: MetricMeta = MetricMeta {
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::Pair {
        second_dashed: true,
    },
    menu_label: Some("perf-widget-menu-gpu-memory"),
    ..GPU_VRAM
};

/// `vram_shared` of a GPU: shared memory in use, of the shared total.
pub const GPU_VRAM_SHARED: MetricMeta = MetricMeta {
    scale: ScaleRule::FixedTotal(gpu_shared_total),
    bar: BarMax::Total(gpu_shared_total),
    ..GPU_VRAM
};

/// `clocks` of a GPU: graphics clock filled and memory clock dashed, as the 1.3.0 Clocks chart
/// (UI-PRF-032). The value is the graphics clock of its hardware maximum, "1905 / 3105 MHz", as
/// the UI-PRF-033 Clock speed row; the memory clock is the dashed line only.
pub const GPU_CLOCKS: MetricMeta = MetricMeta {
    value_text: ValueText::OfTotal,
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::Pair {
        second_dashed: true,
    },
    ..GPU_CLOCK_GFX
};

/// `fan` of a GPU: its fan speed from the record, "0% (0 RPM)". Bar, value and name.
pub const GPU_FAN: MetricMeta = MetricMeta {
    value_text: ValueText::PercentRpm,
    stats: StatsRule::None,
    lines: Lines::NoSeries,
    ..PERCENT
};

/// `response` of a disk: average response time from the record, "0.42 ms". Value and name.
pub const DISK_RESPONSE: MetricMeta = MetricMeta {
    unit: Unit::Milliseconds,
    format: ValueFormat::Count,
    value_text: ValueText::Milliseconds,
    scale: ScaleRule::StickyUpOpen,
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::NoSeries,
    ..PERCENT
};

/// `total_read`, `total_written`, `total_rx` and `total_tx`: a running total from the record,
/// "812.40 GiB". A counter: value and name.
pub const RUNNING_TOTAL: MetricMeta = MetricMeta {
    unit: Unit::Bytes,
    format: ValueFormat::Bytes,
    scale: ScaleRule::StickyUpOpen,
    kind: MetricKind::Counter,
    bar: BarMax::None,
    stats: StatsRule::None,
    lines: Lines::NoSeries,
    ..PERCENT
};

/// `signal` of an adapter: the Wi-Fi signal, "-52 dBm (78%)". The value is the percent, so
/// the bar is out of 100. Bar, value and name.
pub const NET_SIGNAL: MetricMeta = MetricMeta {
    value_text: ValueText::Signal,
    stats: StatsRule::None,
    lines: Lines::NoSeries,
    ..PERCENT
};

/// `rpm` of a fan, "1200 RPM", on a scale up to `fan<n>_max` when the chip reports it.
pub const FAN_RPM: MetricMeta = MetricMeta {
    unit: Unit::Rpm,
    format: ValueFormat::Rpm,
    scale: ScaleRule::StickyUp(fan_max),
    bar: BarMax::Total(fan_max),
    ..PERCENT
};

/// `speed` of a fan: RPM filled and PWM dashed (UI-PRF-034). The value is the RPM.
pub const FAN_SPEED: MetricMeta = MetricMeta {
    stats: StatsRule::None,
    lines: Lines::RpmPwm,
    ..FAN_RPM
};

/// `pwm` of a fan, "45%".
pub const FAN_PWM: MetricMeta = WHOLE_PERCENT;

/// `temp` of a fan: its paired sensor, "45 °C". Without a paired sensor the widget shows "-".
pub const FAN_TEMP: MetricMeta = MetricMeta {
    unit: Unit::Celsius,
    format: ValueFormat::Celsius,
    scale: ScaleRule::StickyTemp,
    bar: BarMax::None,
    ..PERCENT
};

/// `power` of a battery, "-12.4 W": charge above the zero line, discharge below.
pub const BATTERY_POWER: MetricMeta = MetricMeta {
    unit: Unit::Watts,
    format: ValueFormat::Watts,
    scale: ScaleRule::Symmetric,
    bar: BarMax::None,
    ..PERCENT
};

/// `util` of the GPU aggregate, `percent` of a battery: a percent with one decimal.
pub const PAGE_PLAIN_PERCENT: MetricMeta = GPU_ENGINE;

/// `memory` of the GPU aggregate: the summed memory, of the summed total.
pub const GPU_MEMORY_SUM: MetricMeta = MetricMeta {
    scale: ScaleRule::FixedTotal(gpu_mem_sum),
    bar: BarMax::Total(gpu_mem_sum),
    ..GPU_VRAM
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
                    METRIC_GPU_MEMORY => &GPU_MEMORY,
                    METRIC_VRAM_SHARED => &GPU_VRAM_SHARED,
                    METRIC_GPU_CLOCKS => &GPU_CLOCKS,
                    METRIC_FAN => &GPU_FAN,
                    _ => return None,
                }
            }
            DeviceKind::Cpu => match k {
                METRIC_CPU_TOTAL | METRIC_CPU_KERNEL => &PAGE_PERCENT,
                METRIC_PRESSURE => &WHOLE_PERCENT,
                METRIC_FREQ => &CPU_FREQ_PAGE,
                METRIC_CORES_OVERLAID => &CORES_OVERLAID,
                METRIC_CORES_STACKED => &CORES_STACKED,
                METRIC_CLOCK => &CPU_CLOCK,
                METRIC_FREQ_MAX => &CPU_FREQ_PAGE,
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
                METRIC_PRESSURE => &WHOLE_PERCENT,
                METRIC_MEM_COMPOSITION => &MEM_COMPOSITION,
                METRIC_PHYSICAL => &MEM_PHYSICAL,
                METRIC_COMMIT_PCT => &MEM_COMMIT_PCT,
                METRIC_DIRTY => &MEM_DIRTY,
                _ => return None,
            },
            DeviceKind::Disk | DeviceKind::Disks => match k {
                METRIC_DISK_ACTIVE => &PAGE_PERCENT,
                METRIC_DISK_READ | METRIC_DISK_WRITE => &DISK_RATE,
                METRIC_DISK_IO => &DISK_IO,
                METRIC_TEMP if kind == DeviceKind::Disk => &DISK_TEMP,
                METRIC_RESPONSE if kind == DeviceKind::Disk => &DISK_RESPONSE,
                METRIC_TOTAL_READ | METRIC_TOTAL_WRITTEN if kind == DeviceKind::Disk => {
                    &RUNNING_TOTAL
                }
                _ => return None,
            },
            DeviceKind::Net | DeviceKind::Nets => match k {
                METRIC_NET_RX | METRIC_NET_TX => &NET_RATE,
                METRIC_NET_TRAFFIC => &NET_TRAFFIC,
                METRIC_TOTAL_RX | METRIC_TOTAL_TX if kind == DeviceKind::Net => &RUNNING_TOTAL,
                METRIC_SIGNAL if kind == DeviceKind::Net => &NET_SIGNAL,
                _ => return None,
            },
            DeviceKind::Fan => match k {
                METRIC_SPEED => &FAN_SPEED,
                METRIC_RPM => &FAN_RPM,
                METRIC_PWM => &FAN_PWM,
                METRIC_TEMP => &FAN_TEMP,
                _ => return None,
            },
            DeviceKind::Battery => match k {
                METRIC_POWER => &BATTERY_POWER,
                METRIC_PERCENT => &PAGE_PLAIN_PERCENT,
                _ => return None,
            },
            DeviceKind::AggregateGpu => match k {
                METRIC_UTIL => &PAGE_PLAIN_PERCENT,
                METRIC_GPU_MEMORY => &GPU_MEMORY_SUM,
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
            Lines::Pair { .. }
                | Lines::Stacked
                | Lines::PerCore
                | Lines::PerLogical { .. }
                | Lines::Physical
                | Lines::RpmPwm
        )
    }

    /// The value text in a page container ([`Self::page_value`]) or in a box.
    pub fn value_text_in(&self, page: bool) -> ValueText {
        match self.page_value {
            Some(text) if page => text,
            _ => self.value_text,
        }
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

    /// The total the [`ValueText::OfTotal`] and [`ValueText::PercentOfTotal`] texts name, in
    /// a box or on a page: the bar's denominator, else the scale's total or hardware maximum.
    /// `None` for the other value texts.
    pub fn value_total(&self, on: On<'_>) -> Option<f32> {
        let names_total = |t: ValueText| {
            matches!(
                t,
                ValueText::OfTotal | ValueText::Part | ValueText::PercentOfTotal
            )
        };
        if !names_total(self.value_text) && !self.page_value.is_some_and(names_total) {
            return None;
        }
        match (self.bar, self.scale) {
            (BarMax::Total(f), _)
            | (_, ScaleRule::FixedTotal(f) | ScaleRule::StickyUp(f) | ScaleRule::Commit(f)) => {
                fact(f, on)
            }
            (BarMax::Fixed(max), _) => usable(max),
            _ => None,
        }
    }

    /// The format of the stats figures: a percent of `total` for
    /// [`ValueText::PercentOfTotal`] (commit as a percent of the limit), else [`Self::format`].
    pub fn stats_format(&self, total: Option<f32>) -> ValueFormat {
        match total {
            Some(total) if self.value_text == ValueText::PercentOfTotal => ValueFormat::PercentOf {
                total: total as u64,
            },
            _ => self.format,
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
            ScaleRule::Commit(f) => match fact(f, on) {
                Some(limit) => YScale::Commit(limit),
                None => YScale::StickyUp { hw_max: None },
            },
            ScaleRule::StackedLogical => {
                let n = on.map_or(0, |(snap, _)| logical_count(snap));
                YScale::Stacked(n.max(1))
            }
            ScaleRule::Symmetric => YScale::Symmetric,
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
        self.write_value_as(self.value_text, value, second, total, net, out);
    }

    /// [`Self::write_value`] in the value text `text` ([`Self::value_text_in`]).
    pub fn write_value_as(
        &self,
        text: ValueText,
        value: f32,
        second: Option<f32>,
        total: Option<f32>,
        net: NetFormat,
        out: &mut String,
    ) {
        use std::fmt::Write as _;
        let one = |v: f32, out: &mut String| format_value(self.format, v, net, out);
        match text {
            ValueText::Plain | ValueText::Part => one(value, out),
            ValueText::WholePercent => {
                let _ = write!(out, "{}%", crate::components::progress::bucket(value));
            }
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
            ValueText::Sum => one(value + second.unwrap_or(0.0), out),
            ValueText::PercentOfTotal => match total {
                Some(total) => {
                    let pct = (value / total * 100.0).max(0.0);
                    format_value(ValueFormat::Percent, pct, net, out);
                }
                None => one(value, out),
            },
            ValueText::PercentRpm => {
                let _ = write!(out, "{}%", value.round().max(0.0) as u32);
                if let Some(rpm) = second {
                    let _ = write!(out, " ({} RPM)", rpm.round().max(0.0) as u32);
                }
            }
            ValueText::Milliseconds => tl_model::units::response_ms(out, f64::from(value)),
            ValueText::Signal => {
                let pct = value.round().clamp(0.0, 100.0) as u32;
                match second {
                    Some(dbm) => {
                        let _ = write!(out, "{} dBm ({pct}%)", dbm.round() as i32);
                    }
                    None => {
                        let _ = write!(out, "{pct}%");
                    }
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

/// The number of present logical CPUs in the catalog.
fn logical_count(snap: &Snapshot) -> u16 {
    let n = snap
        .devices
        .entries
        .iter()
        .filter(|e| e.present)
        .filter(|e| {
            matches!(
                e.identity,
                tl_model::devices::DeviceIdentity::CpuLogical { .. }
            )
        })
        .count();
    u16::try_from(n).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
    use tl_model::ids::{DeviceIdx, MetricId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::snapshot::SystemSnapshot;
    use tl_model::snapshot::{DeviceHistory, HistPoint, SeriesSet};
    use tl_model::system::{GpuEngine, GpuStaticInfo};
    use tl_model::system::{MemoryTotals, NetTotals};
    use tl_model::table::Total;
    use tl_model::time::BootId;
    use tl_model::units;
    use tl_ui_settings::layout_boxes::{BOX_METRICS, metric_series};

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
        assert!(gpu_meta(&MetricKey::new("nope")).is_none());
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
        // The value alone, as the Memory box's Commit row (tasklens-6swdr.12).
        assert_eq!(total, Some(40.0 * gib));
        let mut expected = String::new();
        units::bytes(&mut expected, 50 << 30);
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

    // ---- Phase 3: the device pages (ADR-019 "Phase 3") ----

    #[test]
    fn every_phase_3_key_has_metadata() {
        use tl_ui_settings::layout_pages::PAGE_METRICS;
        for (kind, key) in PAGE_METRICS {
            let key = MetricKey::new(key);
            let meta = MetricMeta::of(kind, &key)
                .unwrap_or_else(|| panic!("{kind:?} {key:?} has metadata"));
            let series = metric_series(kind, &key).expect("series");
            assert_eq!(meta.is_multi(), series.is_multi(), "{kind:?} {key:?}");
            assert_eq!(
                meta.lines == Lines::NoSeries,
                series.first().is_none(),
                "{kind:?} {key:?}"
            );
            assert!(meta.forms().contains(FormSet::LABEL), "{kind:?} {key:?}");
            assert!(!meta.smoothed, "{kind:?} {key:?}");
        }
        // The phase 1 and phase 2 keys keep their metadata on the new kinds' neighbours.
        assert!(MetricMeta::of(DeviceKind::Fan, &MetricKey::new(METRIC_VRAM)).is_none());
        assert!(MetricMeta::of(DeviceKind::Disks, &MetricKey::new(METRIC_RESPONSE)).is_none());
        assert!(MetricMeta::of(DeviceKind::Nets, &MetricKey::new(METRIC_SIGNAL)).is_none());
        assert!(MetricMeta::of(DeviceKind::Gpu, &MetricKey::new(METRIC_UTIL)).is_none());
    }

    #[test]
    fn phase_3_forms_match_the_adr() {
        let at = |kind, key: &str| MetricMeta::of(kind, &MetricKey::new(key)).expect("known");
        let all = FormSet::all();
        let cl = FormSet::CHART | FormSet::LABEL;
        let cvl = cl | FormSet::VALUE;
        let vl = FormSet::VALUE | FormSet::LABEL;
        let bvl = vl | FormSet::BAR;
        let cases = [
            (DeviceKind::Cpu, METRIC_CORES_OVERLAID, cl),
            (DeviceKind::Cpu, METRIC_CORES_STACKED, cl),
            (DeviceKind::Cpu, METRIC_CLOCK, cvl),
            (DeviceKind::Cpu, METRIC_FREQ_MAX, all),
            (DeviceKind::Memory, METRIC_PHYSICAL, cvl),
            (DeviceKind::Memory, METRIC_COMMIT_PCT, all),
            (DeviceKind::Memory, METRIC_DIRTY, all - FormSet::BAR),
            (DeviceKind::Gpu, METRIC_GPU_MEMORY, cvl),
            (DeviceKind::Gpu, METRIC_VRAM_SHARED, all),
            (DeviceKind::Gpu, METRIC_GPU_CLOCKS, cvl),
            (DeviceKind::Gpu, METRIC_FAN, bvl),
            (DeviceKind::Disk, METRIC_RESPONSE, vl),
            (DeviceKind::Disk, METRIC_TOTAL_READ, vl),
            (DeviceKind::Disk, METRIC_TOTAL_WRITTEN, vl),
            (DeviceKind::Net, METRIC_TOTAL_RX, vl),
            (DeviceKind::Net, METRIC_TOTAL_TX, vl),
            (DeviceKind::Net, METRIC_SIGNAL, bvl),
            (DeviceKind::Fan, METRIC_SPEED, cvl),
            (DeviceKind::Fan, METRIC_RPM, all),
            (DeviceKind::Fan, METRIC_PWM, all),
            (DeviceKind::Fan, METRIC_TEMP, all - FormSet::BAR),
            (DeviceKind::Battery, METRIC_POWER, all - FormSet::BAR),
            (DeviceKind::Battery, METRIC_PERCENT, all),
            (DeviceKind::AggregateGpu, METRIC_UTIL, all),
            (DeviceKind::AggregateGpu, METRIC_GPU_MEMORY, all),
        ];
        for (kind, key, forms) in cases {
            assert_eq!(at(kind, key).forms(), forms, "{kind:?} {key}");
        }
        // Totals are counters; the others are gauges.
        for key in [METRIC_TOTAL_READ, METRIC_TOTAL_WRITTEN] {
            assert_eq!(at(DeviceKind::Disk, key).kind, MetricKind::Counter);
        }
        assert_eq!(at(DeviceKind::Fan, METRIC_RPM).kind, MetricKind::Gauge);
        // Menu labels where two keys would share an item text.
        assert_eq!(
            at(DeviceKind::Memory, METRIC_COMMIT_PCT).menu_label,
            Some("perf-widget-menu-commit-pct")
        );
        assert_eq!(
            at(DeviceKind::Gpu, METRIC_GPU_MEMORY).menu_label,
            Some("perf-widget-menu-gpu-memory")
        );
        assert_eq!(at(DeviceKind::Gpu, METRIC_VRAM).menu_label, None);
        // Scales.
        assert_eq!(
            at(DeviceKind::Battery, METRIC_POWER).y_scale(None, net()),
            YScale::Symmetric
        );
        assert_eq!(
            at(DeviceKind::Cpu, METRIC_CORES_STACKED).y_scale(None, net()),
            YScale::Stacked(1)
        );
    }

    #[test]
    fn page_value_text() {
        let at = |kind, key: &str| MetricMeta::of(kind, &MetricKey::new(key)).expect("known");
        let text = |meta: &MetricMeta, page: bool, v: f32, second: Option<f32>, total| {
            let mut out = String::new();
            let t = meta.value_text_in(page);
            meta.write_value_as(t, v, second, total, NetFormat::default(), &mut out);
            out
        };
        let fmt = |format, v: f32| {
            let mut out = String::new();
            format_value(format, v, NetFormat::default(), &mut out);
            out
        };
        // One decimal on a page, a whole percent in a box.
        let total = at(DeviceKind::Cpu, METRIC_CPU_TOTAL);
        assert_eq!(text(total, false, 18.6, None, None), "18%");
        assert_eq!(
            text(total, true, 18.6, None, None),
            fmt(ValueFormat::Percent, 18.6)
        );
        let active = at(DeviceKind::Disk, METRIC_DISK_ACTIVE);
        assert_eq!(
            text(active, true, 3.4, None, None),
            fmt(ValueFormat::Percent, 3.4)
        );
        // Read + write and download + upload on a page.
        let io = at(DeviceKind::Disk, METRIC_DISK_IO);
        assert_eq!(
            text(io, true, 1_000_000.0, Some(500_000.0), None),
            fmt(ValueFormat::RateIec, 1_500_000.0)
        );
        assert!(text(io, false, 1.0, Some(2.0), None).starts_with("R "));
        let traffic = at(DeviceKind::Net, METRIC_NET_TRAFFIC);
        assert_eq!(
            text(traffic, true, 150_000.0, Some(37_500.0), None),
            fmt(ValueFormat::RateNetwork, 187_500.0)
        );
        // GPU power with its limit on a page.
        let snap = with_gpu(full_gpu());
        let gpu = Some((&snap, DEV));
        let power = at(DeviceKind::Gpu, METRIC_POWER);
        let limit = power.value_total(gpu);
        assert_eq!(limit, Some(320.0));
        assert_eq!(
            text(power, true, 31.9, None, limit),
            format!(
                "{} / {}",
                fmt(ValueFormat::Watts, 31.9),
                fmt(ValueFormat::Watts, 320.0)
            )
        );
        assert_eq!(
            text(power, false, 31.9, None, limit),
            fmt(ValueFormat::Watts, 31.9)
        );
        // GPU clocks: the graphics clock of the graphics maximum, as the UI-PRF-033 Clock
        // speed row "1905 / 3105 MHz". The memory clock is not in the text.
        let clocks = at(DeviceKind::Gpu, METRIC_GPU_CLOCKS);
        let clocks_max = clocks.value_total(gpu);
        assert_eq!(clocks_max, Some(2520.0));
        let clocks_text = format!(
            "{} / {}",
            fmt(ValueFormat::Mhz, 1905.0),
            fmt(ValueFormat::Mhz, 2520.0)
        );
        assert_eq!(text(clocks, false, 1905.0, None, clocks_max), clocks_text);
        assert_eq!(text(clocks, true, 1905.0, None, clocks_max), clocks_text);
        // Commit as a percent of the limit; its stats too.
        let commit = at(DeviceKind::Memory, METRIC_COMMIT_PCT);
        assert_eq!(text(commit, false, 69.0, None, Some(100.0)), "69.0%");
        assert_eq!(
            commit.stats_format(Some(40.0)),
            ValueFormat::PercentOf { total: 40 }
        );
        assert_eq!(
            at(DeviceKind::Memory, METRIC_DIRTY).stats_format(Some(1.0)),
            ValueFormat::Bytes
        );
        // Record values.
        let fan = at(DeviceKind::Gpu, METRIC_FAN);
        assert_eq!(text(fan, false, 0.0, Some(0.0), None), "0% (0 RPM)");
        assert_eq!(text(fan, false, 41.6, None, None), "42%");
        let signal = at(DeviceKind::Net, METRIC_SIGNAL);
        assert_eq!(text(signal, true, 78.0, Some(-52.0), None), "-52 dBm (78%)");
        assert_eq!(text(signal, true, 78.0, None, None), "78%");
        let response = at(DeviceKind::Disk, METRIC_RESPONSE);
        let mut ms = String::new();
        units::response_ms(&mut ms, 0.42);
        assert_eq!(text(response, true, 0.42, None, None), ms);
        assert_eq!(
            text(at(DeviceKind::Fan, METRIC_PWM), true, 45.4, None, None),
            "45%"
        );
    }

    #[test]
    fn phase_3_facts() {
        let gib = 1024.0 * 1024.0 * 1024.0;
        let mut a = full_gpu();
        a.shared_total = Metric::valid(8 << 30);
        let mut b = full_gpu();
        b.mem_total = Metric::valid(8 << 30);
        let snap = snap_with(|s| s.gpus = Arc::from([(DEV, a), (DeviceIdx(2), b)]));
        let on = Some((&snap, DEV));
        let shared =
            MetricMeta::of(DeviceKind::Gpu, &MetricKey::new(METRIC_VRAM_SHARED)).expect("shared");
        assert_eq!(shared.bar_max(on), Some(8.0 * gib));
        let sum = MetricMeta::of(DeviceKind::AggregateGpu, &MetricKey::new(METRIC_GPU_MEMORY))
            .expect("sum");
        assert_eq!(sum.bar_max(Some((&snap, DeviceIdx(9)))), Some(24.0 * gib));
        assert_eq!(sum.y_scale(on, net()), YScale::FixedTotal(24.0 * gib));
        // Commit's axis is the commit limit.
        let mem = snap_with(|s| {
            s.memory = MemoryTotals {
                validity: Validity::Valid,
                commit_limit: 40 << 30,
                ..MemoryTotals::default()
            };
        });
        let commit =
            MetricMeta::of(DeviceKind::Memory, &MetricKey::new(METRIC_COMMIT_PCT)).expect("commit");
        assert_eq!(
            commit.y_scale(Some((&mem, DEV)), net()),
            YScale::Commit(40.0 * gib)
        );
        assert_eq!(commit.value_total(Some((&mem, DEV))), Some(40.0 * gib));
    }
}
