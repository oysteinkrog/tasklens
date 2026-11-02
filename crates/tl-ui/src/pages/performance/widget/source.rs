//! The data of one widget source, shared by every widget that shows it (ADR-019 "Entities and
//! data flow").
//!
//! [`SourceData`] is one entity per source in use. It resolves its [`SourceSelector`] to a
//! device and a series from the device catalog (again only when the catalog changes), reads the
//! current value, the bar's denominator and the stats from each snapshot, formats the value and
//! stats text once for all its widgets, and notifies only when something a widget shows has
//! changed. It emits [`AvailabilityChanged`] when its device goes or comes back, or the device
//! stops or starts reporting the metric; that is the only source event a container listens to.
//!
//! [`SourceStore`] (one per Overview page) maps selectors to their `SourceData`. It keeps weak
//! handles only: a `SourceData` lives while a widget holds it, and is dropped with the last one.
//! The store also tells slot sources (`DeviceSelector::Slot`) which GPUs the box shows.
//!
//! A source reads the record its box shows the value from today (ADR-019 phase 2): the GPU
//! record, the CPU and memory totals, a disk's or an adapter's record, and for a logical CPU,
//! the disk aggregate and the network aggregate the newest sample of the series. The phase 3
//! keys read what their 1.3.0 page shows: a fan's sensor record, an adapter's Wi-Fi details,
//! and for a battery and the GPU aggregate the newest sample of the series. A metric with
//! two or more lines (`power_draw`, `core_clocks`, `composition`, `io`, `traffic`) is one source
//! with a list of series ([`SourceData::series_list`]). The source reads the Overview's snapshot
//! (`box_snapshot`), so it follows the timeline's pause like the boxes do.

use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, SharedString, Subscription, WeakEntity,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::devices::DeviceIdentity;
use tl_model::health::{SourceId, SourceState as CollectorState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::system::{GpuEngine, GpuTotals, Metric};
use tl_model::validity::Validity;
use tl_ui_settings::layout::{
    DeviceSelector, FormSet, METRIC_CLOCK_GFX, METRIC_CLOCK_MEM, METRIC_POWER, METRIC_TEMP,
    METRIC_VRAM, MetricKey, SourceSelector,
};
use tl_ui_settings::layout_boxes::{
    CPU_LOGICAL_PREFIX, DeviceKind, METRIC_CORE_CLOCKS, METRIC_CPU_KERNEL, METRIC_CPU_TOTAL,
    METRIC_DISK_ACTIVE, METRIC_DISK_IO, METRIC_DISK_READ, METRIC_DISK_WRITE, METRIC_FREQ,
    METRIC_LOAD, METRIC_MEM_AVAILABLE, METRIC_MEM_CACHED, METRIC_MEM_COMMIT,
    METRIC_MEM_COMPOSITION, METRIC_MEM_SWAP, METRIC_MEM_USED, METRIC_NET_RX, METRIC_NET_TRAFFIC,
    METRIC_NET_TX, METRIC_POWER_CORES, METRIC_POWER_DRAW, METRIC_PRESSURE, MetricSeries,
    metric_series,
};
use tl_ui_settings::layout_pages::{
    METRIC_CLOCK, METRIC_COMMIT_PCT, METRIC_CORES_OVERLAID, METRIC_CORES_STACKED, METRIC_DIRTY,
    METRIC_FAN, METRIC_FREQ_MAX, METRIC_GPU_CLOCKS, METRIC_GPU_MEMORY, METRIC_PERCENT,
    METRIC_PHYSICAL, METRIC_PWM, METRIC_RESPONSE, METRIC_RPM, METRIC_SIGNAL, METRIC_SPEED,
    METRIC_TOTAL_READ, METRIC_TOTAL_RX, METRIC_TOTAL_TX, METRIC_TOTAL_WRITTEN, METRIC_UTIL,
    METRIC_VRAM_SHARED,
};
use tl_ui_settings::settings::OverviewRange;

use super::meta::{MetricMeta, NetView};
use crate::chart::labels::{NetFormat, format_value};
use crate::chart::{ChartState, YScale};
use crate::components::validity::copy_text;
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::net_totals::NetTotal;
use crate::pages::performance::core::DeviceKey;
use crate::pages::performance::core::core_clocks::core_lines;
use crate::pages::performance::core::gpu::{GpuVendor, gpu_totals};
use crate::pages::performance::core::logical_devices;
use crate::pages::performance::core::sensors::{default_sensor, sensor_device};
use crate::pages::performance::overview::cpu_box::CoreAverage;
use crate::pages::performance::overview::picked::{box_mask, box_snapshot};
use crate::pages::performance::overview::range::{Figures, current_range, figures_text};
use crate::pages::performance::overview::timeline::{OverviewPause, paused};
use crate::settings_global::TlSettings;
use crate::template::Template;

/// The name of VRAM.
const KEY_VRAM: &str = "perf-overview-gpu-vram";
/// The stats line, "min {$min}  avg {$avg}  peak {$peak}".
const KEY_FIGURES: &str = "perf-overview-figures";
/// The swap row's value without swap.
const KEY_NO_SWAP: &str = "perf-overview-mem-no-swap";
/// Why a GPU widget is empty: "Not reported by this GPU".
const KEY_NOT_REPORTED_GPU: &str = "perf-widget-not-reported";
/// Why a widget of another device is empty: "Not reported by this device".
const KEY_NOT_REPORTED_DEVICE: &str = "perf-widget-not-reported-device";
/// A logical CPU's load, "C{$n}", and its clock, "C{$n} speed".
const KEY_CORE: &str = "perf-widget-metric-core";
const KEY_CORE_SPEED: &str = "perf-widget-metric-core-speed";
/// A disk's or an adapter's metric, "{$device} · {$metric}".
const KEY_DEVICE_METRIC: &str = "perf-widget-device-metric";

/// The parts of a snapshot a source reads: the device record, the series and the history for
/// the stats, the catalog for the device, the inventory for the vendor's engine names and the
/// CPU's maximum clock, and the device details for a disk's SMART temperature.
const MASK: ComponentMask = ComponentMask(
    ComponentMask::SYSTEM.0
        | ComponentMask::SERIES.0
        | ComponentMask::DEVICES.0
        | ComponentMask::DEVICE_HISTORY.0
        | ComponentMask::INVENTORY.0
        | ComponentMask::DEVICE_DETAILS.0
        | ComponentMask::HEALTH.0,
);

/// Whether a source can be shown, and if not, why (UI-PRF-061 "Unavailable widget").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Availability {
    Available,
    /// The device is gone, or the slot has no device: "Not connected".
    Gone,
    /// The device does not report the metric: "Not reported by this GPU" or "… by this
    /// device" ([`SourceData::not_reported_key`]).
    NotReported,
    /// The metric key is unknown to this build: "Needs a newer tasklens".
    Unknown,
}

/// Emitted when a source's [`Availability`] changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvailabilityChanged(pub Availability);

/// The `en-US.ftl` key of an engine's name on the GPU page for `vendor`.
fn engine_name_key(vendor: GpuVendor, engine: GpuEngine) -> &'static str {
    match engine {
        GpuEngine::Gpu if vendor == GpuVendor::Amd => "perf-gpu-engine-graphics",
        GpuEngine::Gpu => "perf-gpu-engine-3d",
        GpuEngine::MemCtl => "perf-gpu-engine-memctl",
        GpuEngine::Enc => "perf-gpu-engine-enc",
        GpuEngine::Dec => "perf-gpu-engine-dec",
        GpuEngine::Jpeg => "perf-gpu-engine-jpeg",
        GpuEngine::Ofa => "perf-gpu-engine-ofa",
        GpuEngine::Compute => "perf-gpu-engine-compute",
        GpuEngine::Dma => "perf-gpu-engine-dma",
        GpuEngine::Render => "perf-gpu-engine-render",
        GpuEngine::Copy => "perf-gpu-engine-copy",
        GpuEngine::Video => "perf-gpu-engine-video",
        GpuEngine::VideoEnhance => "perf-gpu-engine-video-enhance",
    }
}

/// The `en-US.ftl` key of the name of `metric` on a GPU of `vendor`; `None` for an unknown key.
pub fn metric_name_key(metric: &MetricKey, vendor: GpuVendor) -> Option<&'static str> {
    if let Some(engine) = metric.gpu_engine() {
        return Some(engine_name_key(vendor, engine));
    }
    match metric.as_str() {
        METRIC_VRAM => Some(KEY_VRAM),
        METRIC_CLOCK_GFX => Some("perf-widget-metric-clock-gfx"),
        METRIC_CLOCK_MEM => Some("perf-widget-metric-clock-mem"),
        METRIC_POWER => Some("perf-widget-metric-power"),
        METRIC_TEMP => Some("perf-widget-metric-temp"),
        METRIC_GPU_MEMORY => Some("perf-widget-metric-gpu-memory"),
        METRIC_VRAM_SHARED => Some("perf-widget-metric-vram-shared"),
        METRIC_GPU_CLOCKS => Some("perf-widget-metric-gpu-clocks"),
        METRIC_FAN => Some("perf-widget-metric-fan"),
        _ => None,
    }
}

/// The `en-US.ftl` key of the name of `metric` on a device of `kind` (ADR-019 phase 2 "Metric
/// keys"): "Utilization", "In use", "Download" and so on. GPU keys are [`metric_name_key`]. A
/// logical CPU's names take the CPU number ([`KEY_CORE`]). `None` for an unknown key.
pub fn box_metric_name_key(kind: DeviceKind, metric: &MetricKey) -> Option<&'static str> {
    let key = match (kind, metric.as_str()) {
        (DeviceKind::Gpu, _) => return metric_name_key(metric, GpuVendor::Unknown),
        (DeviceKind::Cpu, METRIC_CPU_TOTAL) => "perf-widget-metric-cpu-total",
        (DeviceKind::Cpu, METRIC_CPU_KERNEL) => "perf-widget-metric-cpu-kernel",
        (DeviceKind::Cpu, METRIC_FREQ) => "perf-widget-metric-speed",
        (DeviceKind::Cpu | DeviceKind::Disk, METRIC_TEMP) => "perf-widget-metric-temp",
        (DeviceKind::Cpu, METRIC_POWER) => "perf-widget-metric-cpu-power",
        (DeviceKind::Cpu, METRIC_POWER_CORES) => "perf-widget-metric-cpu-power-cores",
        (DeviceKind::Cpu, METRIC_POWER_DRAW) => "perf-widget-metric-power",
        (DeviceKind::Cpu, METRIC_CORE_CLOCKS) => "perf-widget-metric-core-clocks",
        (DeviceKind::Cpu, METRIC_PRESSURE) => "perf-widget-metric-cpu-pressure",
        (DeviceKind::CpuLogical, METRIC_LOAD) => KEY_CORE,
        (DeviceKind::CpuLogical, METRIC_FREQ) => KEY_CORE_SPEED,
        (DeviceKind::Memory, METRIC_MEM_USED) => "perf-overview-mem-used",
        (DeviceKind::Memory, METRIC_MEM_CACHED) => "perf-overview-mem-cached",
        (DeviceKind::Memory, METRIC_MEM_AVAILABLE) => "perf-overview-mem-available",
        (DeviceKind::Memory, METRIC_MEM_SWAP) => "perf-overview-mem-swap",
        (DeviceKind::Memory, METRIC_MEM_COMMIT) => "perf-overview-mem-commit",
        (DeviceKind::Memory, METRIC_PRESSURE) => "perf-widget-metric-mem-pressure",
        (DeviceKind::Memory, METRIC_MEM_COMPOSITION) => "perf-widget-metric-mem-composition",
        (DeviceKind::Disk | DeviceKind::Disks, METRIC_DISK_ACTIVE) => {
            "perf-widget-metric-disk-active"
        }
        (DeviceKind::Disk | DeviceKind::Disks, METRIC_DISK_READ) => "perf-widget-metric-disk-read",
        (DeviceKind::Disk | DeviceKind::Disks, METRIC_DISK_WRITE) => {
            "perf-widget-metric-disk-write"
        }
        (DeviceKind::Disk | DeviceKind::Disks, METRIC_DISK_IO) => "perf-widget-metric-disk-io",
        (DeviceKind::Net | DeviceKind::Nets, METRIC_NET_RX) => "perf-widget-metric-net-rx",
        (DeviceKind::Net | DeviceKind::Nets, METRIC_NET_TX) => "perf-widget-metric-net-tx",
        (DeviceKind::Net | DeviceKind::Nets, METRIC_NET_TRAFFIC) => {
            "perf-widget-metric-net-traffic"
        }
        // Phase 3 (UI-PRF-061 "Phase 3" strings).
        (DeviceKind::Cpu, METRIC_CORES_OVERLAID) => "perf-widget-metric-cores-overlaid",
        (DeviceKind::Cpu, METRIC_CORES_STACKED) => "perf-widget-metric-cores-stacked",
        (DeviceKind::Cpu, METRIC_CLOCK) => "perf-widget-metric-clock",
        (DeviceKind::Cpu, METRIC_FREQ_MAX) => "perf-widget-metric-freq-max",
        (DeviceKind::Memory, METRIC_PHYSICAL) => "perf-widget-metric-physical",
        (DeviceKind::Memory, METRIC_COMMIT_PCT) => "perf-widget-metric-commit-pct",
        (DeviceKind::Memory, METRIC_DIRTY) => "perf-widget-metric-dirty",
        (DeviceKind::Disk, METRIC_RESPONSE) => "perf-widget-metric-response",
        (DeviceKind::Disk, METRIC_TOTAL_READ) => "perf-widget-metric-total-read",
        (DeviceKind::Disk, METRIC_TOTAL_WRITTEN) => "perf-widget-metric-total-written",
        (DeviceKind::Net, METRIC_TOTAL_RX) => "perf-widget-metric-total-rx",
        (DeviceKind::Net, METRIC_TOTAL_TX) => "perf-widget-metric-total-tx",
        (DeviceKind::Net, METRIC_SIGNAL) => "perf-widget-metric-signal",
        (DeviceKind::Fan, METRIC_SPEED) => "perf-widget-metric-fan",
        (DeviceKind::Fan, METRIC_RPM) => "perf-widget-metric-rpm",
        (DeviceKind::Fan, METRIC_PWM) => "perf-widget-metric-pwm",
        (DeviceKind::Fan, METRIC_TEMP) => "perf-widget-metric-temp",
        (DeviceKind::Battery, METRIC_POWER) => "perf-widget-metric-battery-power",
        (DeviceKind::Battery, METRIC_PERCENT) => "perf-widget-metric-battery-percent",
        (DeviceKind::AggregateGpu, METRIC_UTIL) => "perf-widget-metric-util",
        (DeviceKind::AggregateGpu, METRIC_GPU_MEMORY) => "perf-widget-metric-gpu-memory",
        _ => return None,
    };
    Some(key)
}

/// The CPU number of a logical CPU's persistent key, `cpu:<n>`.
fn cpu_number(key: &str) -> Option<u32> {
    key.strip_prefix(CPU_LOGICAL_PREFIX)?.parse().ok()
}

/// A value of the record as `f32`.
fn as_f32_64(m: Metric<f64>) -> Metric<f32> {
    as_f32(m, |v| v as f32)
}

/// The newest valid sample of `id`: `Missing` when the series has none yet, `Unsupported` when
/// the snapshot has no such series.
fn newest(snap: &Snapshot, id: SeriesId) -> Metric<f32> {
    let Some(view) = snap.series.get(id) else {
        return Metric::with(Validity::Unsupported);
    };
    match view.samples.iter().rev().find(|s| s.is_valid()) {
        Some(s) => Metric::valid(s.value),
        None => Metric::with(Validity::Missing),
    }
}

/// The current reading of `metric` on the device `idx` of `kind`, and the second line's value
/// of a pair. Each value comes from the record the box shows it from today. `Unsupported` when
/// the device does not report it, or the key is unknown.
pub(super) fn box_reading(
    kind: DeviceKind,
    metric: &MetricKey,
    snap: &Snapshot,
    idx: DeviceIdx,
    net: Option<&NetTotal>,
) -> (Metric<f32>, Option<f32>) {
    let unsupported = (Metric::with(Validity::Unsupported), None);
    let one = |m: Metric<f32>| (m, None);
    let pair = |a: Metric<f32>, b: Metric<f32>| (a, shown(b.validity).then_some(b.value));
    let key = metric.as_str();
    let series = |metric| {
        newest(
            snap,
            SeriesId {
                device: idx,
                metric,
            },
        )
    };
    match kind {
        DeviceKind::Gpu => {
            let Some(t) = gpu_totals(snap, idx) else {
                return one(Metric::default());
            };
            let second = match key {
                METRIC_GPU_MEMORY => as_f32(t.shared_used, |v| v as f32),
                METRIC_FAN => as_f32(t.fan_rpm, |v| v as f32),
                _ => return one(reading(metric, t)),
            };
            pair(reading(metric, t), second)
        }
        DeviceKind::Cpu => {
            let c = &snap.system.cpu;
            match key {
                METRIC_CPU_TOTAL => one(c.util_pct),
                METRIC_CPU_KERNEL => one(c.kernel_pct),
                METRIC_FREQ => one(as_f32(c.freq_mhz, |v| v as f32)),
                METRIC_TEMP => one(c.temp_c),
                METRIC_POWER => one(c.package_power_w),
                METRIC_POWER_CORES => one(c.core_power_w),
                METRIC_POWER_DRAW => pair(c.package_power_w, c.core_power_w),
                METRIC_PRESSURE => one(c.psi_some_pct),
                METRIC_CLOCK => pair(
                    as_f32(c.freq_mhz, |v| v as f32),
                    series(MetricId::CPU_CLOCK_MAX),
                ),
                METRIC_FREQ_MAX => one(series(MetricId::CPU_CLOCK_MAX)),
                // No value: available while a core reports a clock, or while a logical CPU is
                // present (see `refresh`).
                METRIC_CORE_CLOCKS | METRIC_CORES_OVERLAID | METRIC_CORES_STACKED => {
                    one(Metric::valid(0.0))
                }
                _ => unsupported,
            }
        }
        DeviceKind::CpuLogical => match key {
            METRIC_LOAD => one(newest(
                snap,
                SeriesId {
                    device: idx,
                    metric: MetricId::CPU_TOTAL,
                },
            )),
            METRIC_FREQ => one(newest(
                snap,
                SeriesId {
                    device: idx,
                    metric: MetricId::CPU_FREQ,
                },
            )),
            _ => unsupported,
        },
        DeviceKind::Memory => {
            let m = &snap.system.memory;
            let bytes = |v: u64| Metric {
                value: v as f32,
                validity: m.validity,
            };
            match key {
                METRIC_MEM_USED | METRIC_MEM_COMPOSITION | METRIC_PHYSICAL => one(bytes(m.used)),
                METRIC_MEM_COMMIT | METRIC_COMMIT_PCT => one(bytes(m.committed)),
                METRIC_DIRTY => one(bytes(m.dirty)),
                METRIC_MEM_CACHED => one(bytes(m.cached)),
                METRIC_MEM_AVAILABLE => one(bytes(m.available)),
                METRIC_MEM_SWAP => one(bytes(m.swap_total.saturating_sub(m.swap_free))),
                METRIC_PRESSURE => one(m.psi_some_pct),
                _ => unsupported,
            }
        }
        DeviceKind::Disk => {
            let record = snap.system.disks.iter().find(|(i, _)| *i == idx);
            let Some((_, d)) = record else {
                return (Metric::with(Validity::Missing), None);
            };
            match key {
                METRIC_DISK_ACTIVE => one(d.active_pct),
                METRIC_DISK_READ => one(as_f32_64(d.read_bps)),
                METRIC_DISK_WRITE => one(as_f32_64(d.write_bps)),
                METRIC_DISK_IO => pair(as_f32_64(d.read_bps), as_f32_64(d.write_bps)),
                // The drive's own sensor, else SMART, as the Disk page's Temperature row.
                METRIC_TEMP if d.temp_c.validity == Validity::Unsupported => one(snap
                    .device_details
                    .disks
                    .iter()
                    .find(|(i, _)| *i == idx)
                    .and_then(|(_, details)| details.smart_temp_c)
                    .map_or(Metric::with(Validity::Unsupported), Metric::valid)),
                METRIC_TEMP => one(d.temp_c),
                METRIC_RESPONSE => one(d.response_ms),
                METRIC_TOTAL_READ => one(Metric::valid(d.read_total as f32)),
                METRIC_TOTAL_WRITTEN => one(Metric::valid(d.written_total as f32)),
                _ => unsupported,
            }
        }
        DeviceKind::Disks => {
            let id = |metric| {
                newest(
                    snap,
                    SeriesId {
                        device: idx,
                        metric,
                    },
                )
            };
            match key {
                METRIC_DISK_ACTIVE => one(id(MetricId::DISK_ACTIVE)),
                METRIC_DISK_READ => one(id(MetricId::DISK_READ)),
                METRIC_DISK_WRITE => one(id(MetricId::DISK_WRITE)),
                METRIC_DISK_IO => pair(id(MetricId::DISK_READ), id(MetricId::DISK_WRITE)),
                _ => unsupported,
            }
        }
        DeviceKind::Net => {
            let record = snap.system.nets.iter().find(|(i, _)| *i == idx);
            let Some((_, n)) = record else {
                return (Metric::with(Validity::Missing), None);
            };
            match key {
                METRIC_NET_RX => one(as_f32_64(n.rx_bps)),
                METRIC_NET_TX => one(as_f32_64(n.tx_bps)),
                METRIC_NET_TRAFFIC => pair(as_f32_64(n.rx_bps), as_f32_64(n.tx_bps)),
                METRIC_TOTAL_RX => one(Metric::valid(n.rx_total as f32)),
                METRIC_TOTAL_TX => one(Metric::valid(n.tx_total as f32)),
                // The Wi-Fi signal percent, and its dBm when nl80211 gives it. A wired
                // adapter does not report it.
                METRIC_SIGNAL => {
                    let wifi = snap
                        .device_details
                        .nets
                        .iter()
                        .find(|(i, _)| *i == idx)
                        .and_then(|(_, d)| d.wifi.as_ref());
                    match wifi {
                        Some(w) => (
                            Metric::valid(f32::from(w.signal_pct)),
                            w.signal_dbm.map(|d| d as f32),
                        ),
                        None => unsupported,
                    }
                }
                _ => unsupported,
            }
        }
        DeviceKind::Fan => {
            let record = snap.system.sensors.iter().find(|(i, _)| *i == idx);
            let Some((_, r)) = record else {
                return (Metric::with(Validity::Missing), None);
            };
            match key {
                METRIC_SPEED => pair(r.value, r.pwm_pct),
                METRIC_RPM => one(r.value),
                METRIC_PWM => one(r.pwm_pct),
                // The paired sensor's reading: `refresh` reads it from its series.
                METRIC_TEMP => one(Metric::with(Validity::Missing)),
                _ => unsupported,
            }
        }
        DeviceKind::Battery => match key {
            METRIC_POWER => one(series(MetricId::BATTERY_POWER)),
            METRIC_PERCENT => one(series(MetricId::BATTERY_PCT)),
            _ => unsupported,
        },
        DeviceKind::AggregateGpu => match key {
            METRIC_UTIL => one(series(MetricId::GPU_UTIL)),
            METRIC_GPU_MEMORY => one(series(MetricId::GPU_MEM)),
            _ => unsupported,
        },
        DeviceKind::Nets => {
            // The Network box's counted total: the engine total less the hidden kinds.
            let total = |metric| match net.and_then(|n| n.newest(snap, metric)) {
                Some(v) => Metric::valid(v),
                None => Metric::with(Validity::Missing),
            };
            match key {
                METRIC_NET_RX => one(total(MetricId::NET_RX)),
                METRIC_NET_TX => one(total(MetricId::NET_TX)),
                METRIC_NET_TRAFFIC => pair(total(MetricId::NET_RX), total(MetricId::NET_TX)),
                _ => unsupported,
            }
        }
        DeviceKind::Other => unsupported,
    }
}

fn as_f32<T: Copy + Default>(m: Metric<T>, f: impl Fn(T) -> f32) -> Metric<f32> {
    Metric {
        value: f(m.value),
        validity: m.validity,
    }
}

/// The current reading of `metric` in the record `t`: 3D, encode and decode from their totals,
/// the other engines from the engine list, and the device facts. `Unsupported` when the device
/// does not report it, or the key is unknown.
pub fn reading(metric: &MetricKey, t: &GpuTotals) -> Metric<f32> {
    if let Some(engine) = metric.gpu_engine() {
        return match engine {
            GpuEngine::Gpu => t.util_pct,
            GpuEngine::Enc => t.enc_pct,
            GpuEngine::Dec => t.dec_pct,
            e => t.engines.get(e),
        };
    }
    match metric.as_str() {
        METRIC_VRAM | METRIC_GPU_MEMORY => as_f32(t.mem_used, |v| v as f32),
        METRIC_VRAM_SHARED => as_f32(t.shared_used, |v| v as f32),
        METRIC_FAN => t.fan_pct,
        METRIC_CLOCK_GFX | METRIC_GPU_CLOCKS => as_f32(t.clock_gfx_mhz, |v| v as f32),
        METRIC_CLOCK_MEM => as_f32(t.clock_mem_mhz, |v| v as f32),
        METRIC_POWER => t.power_w,
        METRIC_TEMP => t.temp_c,
        _ => Metric::with(Validity::Unsupported),
    }
}

fn shown(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

/// The resolved device of a source.
#[derive(Clone, Debug, PartialEq)]
struct Resolved {
    idx: DeviceIdx,
    name: SharedString,
}

/// What a source last read, compared to tell whether its widgets must notify.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    availability: Availability,
    value: Metric<f32>,
    /// The second line's value of a pair (`io`, `traffic`, `power_draw`).
    second: Option<f32>,
    bar_max: Option<f32>,
    /// The total an "x / y" value names.
    total: Option<f32>,
    y_scale: YScale,
    offered: FormSet,
    figures: Option<Figures>,
}

/// The settings a source follows, compared to tell whether a settings change concerns it.
#[derive(Clone, Debug, PartialEq)]
struct Followed {
    range: OverviewRange,
    net: NetView,
    /// The network kinds the Network box leaves out (`show_network_*`).
    hidden_nets: SmallVec<[tl_model::devices::NetKind; 5]>,
    /// The core smoothing's time constant in seconds; 0 is off.
    tau_s: f32,
    /// A fan `temp` source: the sensor the fan's `[fans]` entry names (UI-PRF-034).
    fan_temp: Option<String>,
}

impl Followed {
    /// The settings a source follows. `fan` is the fan's key of a fan `temp` source, whose
    /// sensor choice it follows too.
    fn read(cx: &App, fan: Option<&str>) -> Self {
        let range = current_range(cx);
        let Some(settings) = cx
            .try_global::<TlSettings>()
            .map(|_| TlSettings::settings(cx))
        else {
            return Self {
                range,
                net: NetView::default(),
                hidden_nets: SmallVec::new(),
                tau_s: 0.0,
                fan_temp: None,
            };
        };
        let fan_temp = fan
            .and_then(|key| settings.fans.get(key))
            .and_then(|f| f.temp.clone());
        let p = &settings.performance;
        Self {
            range,
            net: NetView {
                format: NetFormat::of(cx),
                dynamic_scale: p.network_dynamic_scale,
            },
            hidden_nets: crate::net_totals::hidden_kinds(p).collect(),
            tau_s: p.overview_core_smoothing.tau_s(),
            fan_temp,
        }
    }
}

/// Everything a widget reads from a source, and every input its text is formatted from.
/// [`SourceData::update`] compares the whole state before and after a change and notifies when
/// it differs, so a field added here joins the comparison by itself (ADR-019).
#[derive(Clone, Debug, PartialEq)]
struct SourceState {
    /// The device key the selector stands for: its own key, or the slot's GPU from the store.
    key: Option<Arc<str>>,
    device: Option<Resolved>,
    /// The series the chart draws, resolved with the device: the metric's series on the
    /// device, one per physical core for `core_clocks`, and for the network aggregate the
    /// engine total. Empty while the device is gone.
    series: SmallVec<[SeriesId; 4]>,
    /// The network aggregate's total and hidden kinds, whose series the total leaves out.
    net_total: Option<NetTotal>,
    /// The hidden kinds' series of each line of the network aggregate's metric.
    minus: SmallVec<[SeriesId; 5]>,
    vendor: GpuVendor,
    followed: Followed,
    reading: Reading,
    /// The version of the strings the name and the stats words come from.
    strings_version: u64,
    /// The source serves a device page: the value is written in the page format
    /// (`MetricMeta::value_text_in`, ADR-019 phase 3).
    page: bool,
    /// On a device page, the error of the collector of the source's kind while it has failed
    /// ([`failed_of`]). Always `None` in an Overview box, which shows no failed state.
    failed: Option<SharedString>,
}

/// One source in use: its device, its current value and its stats over the Overview range.
pub struct SourceData {
    hub: Entity<SnapshotHub>,
    selector: SourceSelector,
    /// The kind of device the selector names; a slot is a GPU.
    kind: DeviceKind,
    meta: Option<&'static MetricMeta>,
    /// The series the metric names on its kind of device (`None` for an unknown key).
    series_of: Option<MetricSeries>,
    /// The catalog version the device was resolved at; `None` forces a new lookup.
    catalog_version: Option<u64>,
    /// The smoothed load of a logical CPU (UI-PRF-018).
    average: CoreAverage,
    state: SourceState,
    name: SharedString,
    value_text: SharedString,
    stats_text: SharedString,
    figures_words: Template,
    /// Bumped each time the source notifies.
    revision: u64,
    _subs: [Subscription; 4],
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: std::cell::Cell<u32>,
    /// How many times the stats were worked out.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub stats_count: std::cell::Cell<u32>,
}

impl EventEmitter<AvailabilityChanged> for SourceData {}

impl SourceData {
    /// A source for `selector`. `slot_key` is the device key of the GPU in the selector's slot
    /// (ignored for a `Key` selector).
    pub fn new(
        hub: &Entity<SnapshotHub>,
        selector: SourceSelector,
        slot_key: Option<Arc<str>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let key = match &selector.device {
            DeviceSelector::Key(k) => Some(Arc::clone(k)),
            DeviceSelector::Slot(_) => slot_key,
        };
        let kind = DeviceKind::of(&selector.device);
        let meta = MetricMeta::of(kind, &selector.metric);
        let series_of = metric_series(kind, &selector.metric);
        let followed = Followed::read(cx, paired_key(series_of, key.as_deref()));
        let mut this = Self {
            hub: hub.clone(),
            selector,
            kind,
            meta,
            series_of,
            catalog_version: None,
            average: CoreAverage::default(),
            state: SourceState {
                key,
                device: None,
                series: SmallVec::new(),
                net_total: None,
                minus: SmallVec::new(),
                vendor: GpuVendor::Unknown,
                followed,
                reading: Reading {
                    availability: Availability::Gone,
                    value: Metric::default(),
                    second: None,
                    bar_max: None,
                    total: None,
                    y_scale: YScale::StickyUp { hw_max: None },
                    offered: FormSet::empty(),
                    figures: None,
                },
                strings_version: cx.try_global::<Strings>().map_or(0, |s| s.strings_version),
                page: false,
                failed: None,
            },
            name: SharedString::default(),
            value_text: SharedString::default(),
            stats_text: SharedString::default(),
            figures_words: Template::new(cx, KEY_FIGURES, &["min", "avg", "peak"]),
            revision: 0,
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<OverviewPause>(Self::on_pause),
                cx.observe_global::<TlSettings>(Self::on_settings),
                cx.observe_global::<Strings>(Self::on_strings),
            ],
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: std::cell::Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            stats_count: std::cell::Cell::new(0),
        };
        let snap = box_snapshot(hub, cx);
        this.refresh(&snap, true, cx);
        this.format_all(cx);
        this
    }

    pub fn selector(&self) -> &SourceSelector {
        &self.selector
    }

    /// The metric's metadata; `None` for a key this build does not know.
    pub fn meta(&self) -> Option<&'static MetricMeta> {
        self.meta
    }

    pub fn availability(&self) -> Availability {
        self.state.reading.availability
    }

    /// The device the source reads, while it is present.
    pub fn device(&self) -> Option<DeviceIdx> {
        self.state.device.as_ref().map(|d| d.idx)
    }

    /// The device's name in the catalog, while it is present.
    pub fn device_name(&self) -> Option<&SharedString> {
        self.state.device.as_ref().map(|d| &d.name)
    }

    /// The first series a chart of the source draws, while the source is available.
    pub fn series(&self) -> Option<SeriesId> {
        self.series_list().first().copied()
    }

    /// Every series a chart of the source draws, while the source is available: one for most
    /// metrics, two for a pair, three for `composition`, one per physical core for
    /// `core_clocks`. On the network aggregate it is the engine's total; the hidden kinds'
    /// totals it leaves out are [`Self::minus_list`]. The page asks for every series in it.
    pub fn series_list(&self) -> &[SeriesId] {
        if self.state.reading.availability != Availability::Available {
            return &[];
        }
        &self.state.series
    }

    /// The series a network aggregate source takes away from its total: the per-kind totals of
    /// the kinds `show_network_*` hides, for each of its lines (download and upload for
    /// `traffic`). [`Self::minus_of`] picks one line's. Empty for every other source.
    pub fn minus_list(&self) -> &[SeriesId] {
        if self.state.reading.availability != Availability::Available {
            return &[];
        }
        &self.state.minus
    }

    /// The series of [`Self::minus_list`] to take away from `line`: the ones of its metric.
    pub fn minus_of(&self, line: SeriesId) -> SmallVec<[SeriesId; 5]> {
        line_minus(self.minus_list(), line)
    }

    /// The `en-US.ftl` key of the text a widget shows while the device does not report the
    /// metric: "Not reported by this GPU" on a GPU, else "Not reported by this device".
    pub fn not_reported_key(&self) -> &'static str {
        if self.kind == DeviceKind::Gpu {
            KEY_NOT_REPORTED_GPU
        } else {
            KEY_NOT_REPORTED_DEVICE
        }
    }

    /// The kind of device the source reads.
    pub fn kind(&self) -> DeviceKind {
        self.kind
    }

    /// The metric's name: the engine's name for the GPU's vendor, "VRAM", "Graphics clock" and
    /// so on. The raw key for a key this build does not know.
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// The current value in the metric's format, empty while the source is unavailable.
    pub fn value_text(&self) -> &SharedString {
        &self.value_text
    }

    /// "min 6%  avg 42%  peak 100%" over the Overview range, empty without a valid sample.
    pub fn stats_text(&self) -> &SharedString {
        &self.stats_text
    }

    /// The current value while it is shown (valid or stale), in the metric's unit: the core
    /// rows' clock column reads a logical CPU's `freq` from it (tasklens-6swdr.12).
    pub fn value(&self) -> Option<f32> {
        let r = &self.state.reading;
        (r.availability == Availability::Available && shown(r.value.validity))
            .then_some(r.value.value)
    }

    /// How full the bar is, 0 to 1; `None` without a bar or a value.
    pub fn bar_fraction(&self) -> Option<f32> {
        let r = &self.state.reading;
        let max = r.bar_max?;
        if !shown(r.value.validity) {
            return None;
        }
        let v = r.value.value;
        Some(if v.is_finite() {
            (v / max).clamp(0., 1.)
        } else {
            0.
        })
    }

    /// The chart's Y axis on this device.
    pub fn y_scale(&self) -> YScale {
        self.state.reading.y_scale
    }

    /// The forms the metric offers on this device (`MetricMeta::offered_forms`).
    pub fn offered_forms(&self) -> FormSet {
        self.state.reading.offered
    }

    /// The state a chart of the source shows over its data: Failed with the collector's error
    /// while the collector of the source's kind has failed on a device page (the 1.3.0 pages'
    /// `health_state`), else Live. The chart works out WarmingUp and Stale itself.
    pub fn chart_state(&self) -> ChartState {
        match &self.state.failed {
            Some(error) => ChartState::Failed(error.clone()),
            None => ChartState::Live,
        }
    }

    /// Bumped each time the source notifies.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The slot's GPU changed (store): the source reads the new device.
    /// The source serves a device page (on) or an Overview box: the value text takes that
    /// format.
    pub fn set_page(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.state.page == on {
            return;
        }
        let snap = box_snapshot(&self.hub, cx);
        self.update(&snap, false, cx, |this, _| this.state.page = on);
    }

    pub fn set_slot_key(&mut self, key: Option<Arc<str>>, cx: &mut Context<Self>) {
        if !matches!(self.selector.device, DeviceSelector::Slot(_)) || self.state.key == key {
            return;
        }
        let snap = box_snapshot(&self.hub, cx);
        self.update(&snap, true, cx, |this, _| {
            this.state.key = key;
            this.catalog_version = None;
        });
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let mask = box_mask(ev.mask, cx);
        if !mask.intersects(MASK) {
            return;
        }
        let changed = |id: &SeriesId| ev.changed_series.binary_search(id).is_ok();
        let series_changed = self.series_list().iter().any(changed)
            || self.minus_list().iter().any(changed)
            || mask.intersects(ComponentMask::DEVICE_HISTORY);
        let snap = box_snapshot(&hub, cx);
        self.update(&snap, series_changed, cx, |_, _| {});
    }

    /// The Overview was held at a time or made live: read the other snapshot.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        let snap = box_snapshot(&self.hub, cx);
        self.update(&snap, true, cx, |this, _| {
            this.average = CoreAverage::default()
        });
    }

    /// A new Overview range, network view, set of hidden network kinds or core smoothing: new
    /// stats, scale, text or series. The settings are part of the state, so a change of them
    /// formats and notifies even when the reading is the same.
    fn on_settings(&mut self, cx: &mut Context<Self>) {
        let fan = paired_key(self.series_of, self.state.key.as_deref());
        let followed = Followed::read(cx, fan);
        if followed == self.state.followed {
            return;
        }
        let snap = box_snapshot(&self.hub, cx);
        self.update(&snap, true, cx, |this, _| {
            if followed.hidden_nets != this.state.followed.hidden_nets
                || followed.fan_temp != this.state.followed.fan_temp
            {
                // The network total's series change with the hidden kinds, a fan
                // temperature's with its sensor.
                this.catalog_version = None;
            }
            if followed.tau_s != this.state.followed.tau_s {
                this.average = CoreAverage::default();
            }
            this.state.followed = followed;
        });
    }

    /// A new language: new name and stats words.
    fn on_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.state.strings_version {
            return;
        }
        let snap = box_snapshot(&self.hub, cx);
        self.update(&snap, false, cx, |this, cx| {
            this.state.strings_version = version;
            this.figures_words = Template::new(cx, KEY_FIGURES, &["min", "avg", "peak"]);
        });
    }

    /// Applies `input` (a change of what the source follows), reads `snap` and, with `stats`,
    /// works the stats out again. Compares the whole [`SourceState`] before and after: when it
    /// differs, formats, notifies, and emits [`AvailabilityChanged`] on an availability change.
    fn update(
        &mut self,
        snap: &Snapshot,
        stats: bool,
        cx: &mut Context<Self>,
        input: impl FnOnce(&mut Self, &App),
    ) {
        let before = self.state.clone();
        input(self, cx);
        self.refresh(snap, stats, cx);
        if self.state == before {
            return;
        }
        self.format_all(cx);
        if self.state.reading.availability != before.reading.availability {
            cx.emit(AvailabilityChanged(self.state.reading.availability));
        }
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// Finds the device of the key in the catalog: a present entry whose persistent key is the
    /// key (or, for a GPU, whose identity gives the key, as phase 1 did).
    fn resolve_device(&self, snap: &Snapshot) -> Option<Resolved> {
        let key = self.state.key.as_deref()?;
        snap.devices
            .entries
            .iter()
            .filter(|e| e.present)
            .find(|e| {
                &*e.persistent_key == key
                    || DeviceKey::from_identity(&e.identity).is_some_and(|k| &*k.0 == key)
            })
            .map(|e| Resolved {
                idx: e.idx,
                name: SharedString::from(e.display_name.to_string()),
            })
    }

    /// The series of the metric on the resolved device: one per physical core for
    /// `core_clocks` (the CPU page's Core clocks lines), the engine total for the network
    /// aggregate (and its hidden kinds in `net_total`), else the metric's series on the device.
    fn resolve_series(&mut self, snap: &Snapshot, cx: &App) {
        self.state.series.clear();
        self.state.net_total = None;
        self.state.minus.clear();
        let (Some(idx), Some(of)) = (self.state.device.as_ref().map(|d| d.idx), self.series_of)
        else {
            return;
        };
        match of {
            MetricSeries::PerLogical(metric) => {
                // Every present logical CPU, by CPU number (UI-PRF-023).
                let mut logical: SmallVec<[(u32, DeviceIdx); 32]> = snap
                    .devices
                    .entries
                    .iter()
                    .filter(|e| e.present)
                    .filter_map(|e| match e.identity {
                        DeviceIdentity::CpuLogical { id } => Some((u32::from(id), e.idx)),
                        _ => None,
                    })
                    .collect();
                logical.sort_unstable_by_key(|(id, _)| *id);
                self.state.series.extend(
                    logical
                        .iter()
                        .map(|&(_, device)| SeriesId { device, metric }),
                );
            }
            MetricSeries::Paired(metric) => {
                // A fan's temperature: the sensor its `[fans]` entry names, else its default
                // pairing (PERF-077). No sensor: no series.
                let fan = self.state.key.as_deref().unwrap_or_default();
                let sensor = match self.state.followed.fan_temp.as_deref() {
                    Some(s) => sensor_device(snap, s),
                    None => default_sensor(snap, fan).and_then(|s| sensor_device(snap, &s)),
                };
                if let Some(device) = sensor {
                    self.state.series.push(SeriesId { device, metric });
                }
            }
            MetricSeries::PerCore(metric) => {
                let mut logical = Vec::new();
                logical_devices(&snap.devices, &mut logical);
                let lines = core_lines(&snap.system.cpu_topology, &logical);
                self.state.series.extend(
                    lines
                        .devices
                        .iter()
                        .map(|&device| SeriesId { device, metric }),
                );
            }
            _ if self.kind == DeviceKind::Nets => {
                if cx.has_global::<TlSettings>() {
                    let perf = &TlSettings::settings(cx).performance;
                    self.state.net_total = Some(NetTotal::of(&snap.devices, perf));
                } else {
                    self.state.net_total = Some(NetTotal {
                        total: Some(idx),
                        hidden: SmallVec::new(),
                    });
                }
                let ids = of.ids().iter().map(|&metric| SeriesId {
                    device: idx,
                    metric,
                });
                self.state.series.extend(ids);
                if let Some(n) = &self.state.net_total {
                    for &metric in of.ids() {
                        self.state.minus.extend(n.minus(metric).iter().copied());
                    }
                }
            }
            _ => {
                let ids = of.ids().iter().map(|&metric| SeriesId {
                    device: idx,
                    metric,
                });
                self.state.series.extend(ids);
            }
        }
    }

    /// The current value of a smoothed metric: the logical CPU's load averaged over the core
    /// smoothing's time constant, as the core rows do (UI-PRF-018). The raw value while the
    /// smoothing is off or the Overview is paused.
    fn smooth(&mut self, snap: &Snapshot, raw: Metric<f32>, cx: &App) -> Metric<f32> {
        let tau_s = self.state.followed.tau_s;
        let Some(id) = self.state.series.first().copied() else {
            return raw;
        };
        if tau_s <= 0.0 || paused(cx).is_some() {
            return raw;
        }
        let Some(view) = snap.series.get(id) else {
            return raw;
        };
        self.average.advance(&view.samples, tau_s);
        match self.average.value {
            Some(v) => Metric::valid(v),
            None => raw,
        }
    }

    /// Resolves the device when the catalog changed and reads the record and the stats. The
    /// stats are worked out again with `stats`, and after a new lookup, whose series or minus
    /// list may differ.
    fn refresh(&mut self, snap: &Snapshot, mut stats: bool, cx: &App) {
        // The collector's health is read live, as the 1.3.0 pages read it.
        self.state.failed = if self.state.page {
            failed_of(self.kind, self.hub.read(cx).current())
        } else {
            None
        };
        if self.catalog_version != Some(snap.devices.version) {
            self.catalog_version = Some(snap.devices.version);
            self.state.device = self.resolve_device(snap);
            self.resolve_series(snap, cx);
            self.average = CoreAverage::default();
            stats = true;
        }
        let idx = self.state.device.as_ref().map(|d| d.idx);
        self.state.vendor = match (self.kind, idx) {
            (DeviceKind::Gpu, Some(i)) => GpuVendor::of(snap, i),
            _ => GpuVendor::Unknown,
        };
        let metric = &self.selector.metric;
        let (mut value, second) = match idx {
            Some(i) => box_reading(self.kind, metric, snap, i, self.state.net_total.as_ref()),
            None => (Metric::default(), None),
        };
        match self.series_of {
            Some(MetricSeries::PerCore(_) | MetricSeries::PerLogical(_))
                if self.state.series.is_empty() =>
            {
                value = Metric::with(Validity::Unsupported);
            }
            // A fan's temperature: the paired sensor's newest sample; "-" without a sensor.
            Some(MetricSeries::Paired(_)) if idx.is_some() => {
                value = self
                    .state
                    .series
                    .first()
                    .map_or(Metric::with(Validity::Missing), |&id| newest(snap, id));
            }
            _ => {}
        }
        let availability = match (self.meta, idx) {
            (None, _) => Availability::Unknown,
            (_, None) => Availability::Gone,
            _ if value.validity == Validity::Unsupported => Availability::NotReported,
            _ => Availability::Available,
        };
        let Some(meta) = self.meta else {
            self.state.reading.availability = availability;
            return;
        };
        if meta.smoothed && availability == Availability::Available {
            value = self.smooth(snap, value, cx);
        }
        let figures = if availability != Availability::Available {
            None
        } else if stats || self.state.reading.availability != Availability::Available {
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            self.stats_count.set(self.stats_count.get() + 1);
            let range = self.state.followed.range;
            // `self.state.minus` itself: the reading's availability is still the last one here.
            self.state.series.first().and_then(|&id| {
                let minus = line_minus(&self.state.minus, id);
                meta.stats_minus(snap, id, &minus, range)
            })
        } else {
            self.state.reading.figures
        };
        let on = idx.map(|i| (snap, i));
        self.state.reading = Reading {
            availability,
            value,
            second,
            bar_max: meta.bar_max(on),
            total: meta.value_total(on),
            y_scale: meta.y_scale(on, self.state.followed.net),
            offered: meta.offered_forms(on),
            figures,
        };
    }

    /// The metric's name, by the ADR-019 phase 2 rule: the metric's name on a CPU, memory, an
    /// aggregate or a GPU; "C{n}" and "C{n} speed" on a logical CPU; on a disk or an adapter
    /// the device's name for `io` and `traffic`, else "{device} · {metric}". The raw key for a
    /// key this build does not know.
    fn format_name(&self, cx: &App) -> SharedString {
        let metric = &self.selector.metric;
        let key = match self.kind {
            DeviceKind::Gpu => metric_name_key(metric, self.state.vendor),
            kind => box_metric_name_key(kind, metric),
        };
        let Some(key) = key else {
            return SharedString::from(metric.as_str().to_string());
        };
        match self.kind {
            DeviceKind::CpuLogical => {
                let n = self
                    .state
                    .key
                    .as_deref()
                    .and_then(cpu_number)
                    .unwrap_or_default();
                tr_args(cx, key, &[("n", FluentValue::from(n))])
            }
            DeviceKind::Disk | DeviceKind::Net => {
                let device = self.device_name().cloned().unwrap_or_default();
                let whole = matches!(metric.as_str(), METRIC_DISK_IO | METRIC_NET_TRAFFIC);
                if whole && !device.is_empty() {
                    return device;
                }
                if device.is_empty() {
                    return tr(cx, key);
                }
                tr_args(
                    cx,
                    KEY_DEVICE_METRIC,
                    &[
                        ("device", FluentValue::from(device.to_string())),
                        ("metric", FluentValue::from(tr(cx, key).to_string())),
                    ],
                )
            }
            _ => tr(cx, key),
        }
    }

    /// The name, value and stats text from the last reading.
    fn format_all(&mut self, cx: &App) {
        self.name = self.format_name(cx);
        let r = self.state.reading;
        let Some(meta) = self
            .meta
            .filter(|_| r.availability == Availability::Available)
        else {
            self.value_text = SharedString::default();
            self.stats_text = SharedString::default();
            return;
        };
        let net = self.state.followed.net.format;
        let no_swap = self.kind == DeviceKind::Memory
            && self.selector.metric.as_str() == METRIC_MEM_SWAP
            && shown(r.value.validity)
            && snap_swap_total_is_zero(r.total);
        self.value_text = if no_swap {
            tr(cx, KEY_NO_SWAP)
        } else if shown(r.value.validity) {
            let mut out = String::new();
            let text = meta.value_text_in(self.state.page);
            meta.write_value_as(text, r.value.value, r.second, r.total, net, &mut out);
            out.into()
        } else {
            copy_text(r.value.validity, &SharedString::default())
        };
        let format = meta.stats_format(r.total);
        self.stats_text = figures_text(&self.figures_words, r.figures, |v, out| {
            format_value(format, v, net, out);
        });
    }
}

/// The fan key of a fan `temp` source, whose sensor choice the source follows; `None` for every
/// other source.
fn paired_key(series_of: Option<MetricSeries>, key: Option<&str>) -> Option<&str> {
    matches!(series_of, Some(MetricSeries::Paired(_)))
        .then_some(key)
        .flatten()
}

/// The series of `minus` with the metric of `line`.
fn line_minus(minus: &[SeriesId], line: SeriesId) -> SmallVec<[SeriesId; 5]> {
    minus
        .iter()
        .copied()
        .filter(|m| m.metric == line.metric)
        .collect()
}

/// Whether the swap total a reading took is 0 or unknown, which the box shows as "No swap".
fn snap_swap_total_is_zero(total: Option<f32>) -> bool {
    total.is_none_or(|t| t <= 0.0)
}
/// The error of the collector that reads devices of `kind` while it has failed, as the 1.3.0
/// CPU, Memory, Disk and Network pages show it on their charts (`health_state`). `None` while it
/// runs, and for a kind whose page shows another state: the GPU pages show their probe state
/// (`gpu::probe_chart_state`), and fans, batteries and other keys have no failed chart.
fn failed_of(kind: DeviceKind, snap: &Snapshot) -> Option<SharedString> {
    let source = match kind {
        DeviceKind::Cpu | DeviceKind::CpuLogical => SourceId::Cpu,
        DeviceKind::Memory => SourceId::Memory,
        DeviceKind::Disk | DeviceKind::Disks => SourceId::Disk,
        DeviceKind::Net | DeviceKind::Nets => SourceId::Net,
        DeviceKind::Gpu
        | DeviceKind::AggregateGpu
        | DeviceKind::Fan
        | DeviceKind::Battery
        | DeviceKind::Other => return None,
    };
    let h = snap.health.get(source);
    (h.state == CollectorState::Failed)
        .then(|| SharedString::from(h.last_error.as_deref().unwrap_or_default().to_owned()))
}

/// The sources of one Overview page or one device page, by selector.
pub struct SourceStore {
    hub: Entity<SnapshotHub>,
    /// The store serves a device page: its sources write their values in the page format.
    page: bool,
    /// The device keys of the GPUs the box shows, in sidebar order: slot 0, then slot 1.
    slots: Vec<Arc<str>>,
    sources: FxHashMap<SourceSelector, WeakEntity<SourceData>>,
}

impl SourceStore {
    pub fn new(hub: &Entity<SnapshotHub>) -> Self {
        Self {
            hub: hub.clone(),
            page: false,
            slots: Vec::new(),
            sources: FxHashMap::default(),
        }
    }

    /// The store of one device page (ADR-019 phase 3: each page has its own).
    pub fn for_page(hub: &Entity<SnapshotHub>) -> Self {
        Self {
            page: true,
            ..Self::new(hub)
        }
    }

    /// The source of `selector`: the one in use, or a new one. Widgets on the same selector get
    /// the same entity, so they share its value and stats.
    pub fn source(&mut self, selector: &SourceSelector, cx: &mut App) -> Entity<SourceData> {
        if let Some(data) = self.sources.get(selector).and_then(WeakEntity::upgrade) {
            return data;
        }
        self.prune();
        let slot_key = self.slot_key(&selector.device);
        let hub = self.hub.clone();
        let selector = selector.clone();
        let page = self.page;
        let data = cx.new(|cx| {
            let mut data = SourceData::new(&hub, selector.clone(), slot_key, cx);
            data.set_page(page, cx);
            data
        });
        self.sources.insert(selector, data.downgrade());
        data
    }

    /// The device keys of the GPUs the box shows, slot 0 first. Slot sources follow.
    pub fn set_slots(&mut self, keys: &[Arc<str>], cx: &mut App) {
        if self.slots == keys {
            return;
        }
        self.slots = keys.to_vec();
        self.prune();
        for (selector, data) in &self.sources {
            let Some(data) = data.upgrade() else {
                continue;
            };
            let key = self.slot_key(&selector.device);
            data.update(cx, |d, cx| d.set_slot_key(key, cx));
        }
    }

    /// The number of sources in use.
    pub fn len(&self) -> usize {
        self.sources
            .values()
            .filter(|w| w.upgrade().is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The device key of the GPU in slot `n`, if the box shows one there.
    pub fn slot(&self, n: u8) -> Option<Arc<str>> {
        self.slots.get(usize::from(n)).cloned()
    }

    fn slot_key(&self, device: &DeviceSelector) -> Option<Arc<str>> {
        match device {
            DeviceSelector::Slot(n) => self.slots.get(usize::from(*n)).cloned(),
            DeviceSelector::Key(_) => None,
        }
    }

    /// Forgets the sources no widget uses any more.
    fn prune(&mut self) {
        self.sources.retain(|_, w| w.upgrade().is_some());
    }
}
