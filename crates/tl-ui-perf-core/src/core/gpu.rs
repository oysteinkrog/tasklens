//! The GPU engine model the Overview, the widgets and the GPU page share: the vendor, the
//! engines each vendor reports, and the snapshot lookups for a GPU's record and adapter
//! (PERF-062, PERF-063). Moved from `gpu.rs` (tasklens-zljr5.22). The probe state and the
//! Power and Clocks heading tooltips moved here too (`tasklens-hzire.4`), so the GPU page's
//! chart widgets read them from their source.

use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{App, SharedString};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId};
use tl_model::snapshot::{GraphicsAdapter, Snapshot};
use tl_model::system::{ClockEventReasons, GpuEngine, GpuTotals, Metric};
use tl_model::validity::Validity;

use tl_ui_base::i18n::{tr, tr_args, tr_en};

/// The GPU maker. Only NVIDIA has a probe, so only NVIDIA shows probe states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

impl GpuVendor {
    /// The vendor from the adapter's kernel module: `nvidia`, `amdgpu` or `radeon`, `i915` or
    /// `xe`. Anything else is `Unknown`. The inventory adds the module version after the name
    /// ("nvidia 615.71.09"), which is ignored here.
    pub fn from_kernel_module(module: &str) -> Self {
        match module.split_whitespace().next().unwrap_or_default() {
            "nvidia" => Self::Nvidia,
            "amdgpu" | "radeon" => Self::Amd,
            "i915" | "xe" => Self::Intel,
            _ => Self::Unknown,
        }
    }

    /// The vendor of `device` from the inventory's graphics adapters; `Unknown` while the
    /// inventory has no adapter for it or no kernel module.
    pub fn of(snap: &Snapshot, device: DeviceIdx) -> Self {
        adapter(snap, device)
            .filter(|a| matches!(a.kernel_module.validity, Validity::Valid | Validity::Stale))
            .map_or(Self::Unknown, |a| {
                Self::from_kernel_module(&a.kernel_module.value)
            })
    }
}

/// The inventory's graphics adapter of `device`.
pub fn adapter(snap: &Snapshot, device: DeviceIdx) -> Option<&GraphicsAdapter> {
    snap.inventory
        .graphics
        .iter()
        .find(|a| a.device == Some(device))
}

/// One engine: its settings key, its name's `en-US.ftl` key and its series.
#[derive(Clone, Copy, Debug)]
pub struct Engine {
    pub key: &'static str,
    pub name: &'static str,
    pub metric: MetricId,
    /// The engine's bit in the reported-engine mask.
    bit: u8,
}

/// The Overview box's engines in row order: NVIDIA has all four, other vendors skip the memory
/// controller. The Overview draws only the three series of `GpuTotals`. The page's grid uses
/// [`vendor_cells`].
const ENGINES: [Engine; 4] = [
    Engine {
        key: "gpu",
        name: "perf-gpu-engine-3d",
        metric: MetricId::GPU_UTIL,
        bit: 1,
    },
    Engine {
        key: "memctl",
        name: "perf-gpu-engine-memctl",
        metric: MetricId::GPU_ENGINE_MEMCTL,
        bit: 8,
    },
    Engine {
        key: "enc",
        name: "perf-gpu-engine-enc",
        metric: MetricId::GPU_ENC,
        bit: 2,
    },
    Engine {
        key: "dec",
        name: "perf-gpu-engine-dec",
        metric: MetricId::GPU_DEC,
        bit: 4,
    },
];

/// The Overview engines of the other vendors.
const OTHER_ENGINES: [Engine; 3] = [ENGINES[0], ENGINES[2], ENGINES[3]];

/// The Overview engines of `vendor` in row order.
fn vendor_engines(vendor: GpuVendor) -> &'static [Engine] {
    match vendor {
        GpuVendor::Nvidia => &ENGINES,
        _ => &OTHER_ENGINES,
    }
}

/// One cell of the page's engine grid: the engine and its name's `en-US.ftl` key.
#[derive(Clone, Copy, Debug)]
pub struct EngineCell {
    pub engine: GpuEngine,
    pub name: &'static str,
}

const fn cell(engine: GpuEngine, name: &'static str) -> EngineCell {
    EngineCell { engine, name }
}

pub const NAME_3D: &str = "perf-gpu-engine-3d";

/// NVIDIA's engines in grid order (PERF-063). JPEG and optical flow show only when the engine
/// list reports them.
const NVIDIA_CELLS: [EngineCell; 6] = [
    cell(GpuEngine::Gpu, NAME_3D),
    cell(GpuEngine::MemCtl, "perf-gpu-engine-memctl"),
    cell(GpuEngine::Enc, "perf-gpu-engine-enc"),
    cell(GpuEngine::Dec, "perf-gpu-engine-dec"),
    cell(GpuEngine::Jpeg, "perf-gpu-engine-jpeg"),
    cell(GpuEngine::Ofa, "perf-gpu-engine-ofa"),
];

const AMD_CELLS: [EngineCell; 7] = [
    cell(GpuEngine::Gpu, "perf-gpu-engine-graphics"),
    cell(GpuEngine::Compute, "perf-gpu-engine-compute"),
    cell(GpuEngine::MemCtl, "perf-gpu-engine-memctl"),
    cell(GpuEngine::Enc, "perf-gpu-engine-enc"),
    cell(GpuEngine::Dec, "perf-gpu-engine-dec"),
    cell(GpuEngine::Jpeg, "perf-gpu-engine-jpeg"),
    cell(GpuEngine::Dma, "perf-gpu-engine-dma"),
];

/// Intel's video engine both encodes and decodes, so it is one "Video" cell.
const INTEL_CELLS: [EngineCell; 5] = [
    cell(GpuEngine::Render, "perf-gpu-engine-render"),
    cell(GpuEngine::Compute, "perf-gpu-engine-compute"),
    cell(GpuEngine::Copy, "perf-gpu-engine-copy"),
    cell(GpuEngine::Video, "perf-gpu-engine-video"),
    cell(GpuEngine::VideoEnhance, "perf-gpu-engine-video-enhance"),
];

/// A GPU of unknown make: the three engines every `GpuTotals` carries.
const UNKNOWN_CELLS: [EngineCell; 3] = [
    cell(GpuEngine::Gpu, NAME_3D),
    cell(GpuEngine::Enc, "perf-gpu-engine-enc"),
    cell(GpuEngine::Dec, "perf-gpu-engine-dec"),
];

/// The grid engines of `vendor` in grid order.
pub fn vendor_cells(vendor: GpuVendor) -> &'static [EngineCell] {
    match vendor {
        GpuVendor::Nvidia => &NVIDIA_CELLS,
        GpuVendor::Amd => &AMD_CELLS,
        GpuVendor::Intel => &INTEL_CELLS,
        GpuVendor::Unknown => &UNKNOWN_CELLS,
    }
}

/// The bit of `engine` in an engine mask.
const fn engine_bit(engine: GpuEngine) -> u16 {
    1 << engine as u16
}

/// The engines a GPU of `vendor` reports, as a mask of [`engine_bit`]. Gpu, Enc and Dec count
/// when their `GpuTotals` metric is not Unsupported; every other engine counts when the record's
/// engine list reports it. A GPU without a record yet reports the engines every card of the
/// vendor has, so the page keeps its shape.
fn engine_mask(vendor: GpuVendor, totals: Option<&GpuTotals>) -> u16 {
    vendor_cells(vendor)
        .iter()
        .filter(|c| match (totals, c.engine) {
            (None, GpuEngine::Gpu | GpuEngine::Enc | GpuEngine::Dec) => true,
            (None, GpuEngine::Render | GpuEngine::Copy | GpuEngine::Video) => true,
            (None, _) => false,
            (Some(t), GpuEngine::Gpu) => t.util_pct.validity != Validity::Unsupported,
            (Some(t), GpuEngine::Enc) => t.enc_pct.validity != Validity::Unsupported,
            (Some(t), GpuEngine::Dec) => t.dec_pct.validity != Validity::Unsupported,
            (Some(t), e) => t.engines.is_reported(e),
        })
        .fold(0, |m, c| m | engine_bit(c.engine))
}

/// What the grid is built from besides the settings: which engines the GPU reports, and whether
/// fdinfo-based values miss other users' processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reported {
    mask: u16,
    pub partial: bool,
}

impl Reported {
    pub fn of(vendor: GpuVendor, totals: Option<&GpuTotals>) -> Self {
        Self {
            mask: engine_mask(vendor, totals),
            partial: totals.is_some_and(|t| t.fdinfo_partial),
        }
    }

    pub fn has(self, engine: GpuEngine) -> bool {
        self.mask & engine_bit(engine) != 0
    }
}

/// The engine cells of a GPU, (key, name), in the vendor's order: every engine in `reported`
/// that is not in `hidden` (`performance.hidden_gpu_engines`). Names are en-US.
pub fn engine_cells(
    vendor: GpuVendor,
    reported: &[Arc<str>],
    hidden: &[Arc<str>],
) -> Vec<(Arc<str>, SharedString)> {
    vendor_cells(vendor)
        .iter()
        .filter(|c| reported.iter().any(|k| &**k == c.engine.key()))
        .filter(|c| !c.engine.is_hidden(hidden))
        .map(|c| (Arc::from(c.engine.key()), tr_en(c.name, &[])))
        .collect()
}

/// The engines of a GPU of `vendor` with `totals` that are reported and not in `hidden`
/// (`performance.hidden_gpu_engines`), in the vendor's order. The Overview GPU box draws one row
/// per engine from here (crw.47).
pub fn shown_engines<'a, H: AsRef<str>>(
    vendor: GpuVendor,
    totals: Option<&GpuTotals>,
    hidden: &'a [H],
) -> impl Iterator<Item = &'static Engine> + 'a {
    let reported = reported_bits(totals);
    vendor_engines(vendor)
        .iter()
        .filter(move |e| reported & e.bit != 0)
        // The Overview box has no memory controller row.
        .filter(|e| e.metric != MetricId::GPU_ENGINE_MEMCTL)
        .filter(move |e| !hidden.iter().any(|k| k.as_ref() == e.key))
}

/// The engines of the page's engine grid for a GPU of `vendor` with `totals`, as (engine, name
/// key), in grid order, without those `hidden` hides (`performance.hidden_gpu_engines`). The
/// Overview GPU box's `engine_charts` mode draws one chart per engine from here (crw.77).
pub fn grid_engines<'a, H: AsRef<str>>(
    vendor: GpuVendor,
    totals: Option<&GpuTotals>,
    hidden: &'a [H],
) -> impl Iterator<Item = (GpuEngine, &'static str)> + 'a {
    let reported = Reported::of(vendor, totals);
    vendor_cells(vendor)
        .iter()
        .filter(move |c| reported.has(c.engine))
        .filter(move |c| !c.engine.is_hidden(hidden))
        .map(|c| (c.engine, c.name))
}

/// Every engine `device` reports, (key, name), in the vendor's order, hidden ones included.
/// The Settings window's hidden engine checklist lists these (E21-023). Names are en-US.
pub fn device_engines(snap: &Snapshot, device: DeviceIdx) -> Vec<(Arc<str>, SharedString)> {
    let vendor = GpuVendor::of(snap, device);
    let reported = Reported::of(vendor, gpu_totals(snap, device));
    vendor_cells(vendor)
        .iter()
        .filter(|c| reported.has(c.engine))
        .map(|c| (Arc::from(c.engine.key()), tr_en(c.name, &[])))
        .collect()
}

/// The engines a GPU reports, as a mask of [`Engine::bit`]: every engine whose metric is not
/// Unsupported. The memory controller counts only when the record's engine list reports it. A GPU
/// without a record yet reports the three P1 engines, so the page keeps its shape.
pub fn reported_bits(totals: Option<&GpuTotals>) -> u8 {
    let Some(t) = totals else {
        return OTHER_ENGINES.iter().fold(0, |m, e| m | e.bit);
    };
    let mut bits = 0;
    for (e, validity) in
        OTHER_ENGINES
            .iter()
            .zip([t.util_pct.validity, t.enc_pct.validity, t.dec_pct.validity])
    {
        if validity != Validity::Unsupported {
            bits |= e.bit;
        }
    }
    if t.engines.is_reported(GpuEngine::MemCtl) {
        bits |= ENGINES[1].bit;
    }
    bits
}

/// The record of `device` in the snapshot.
pub fn gpu_totals(snap: &Snapshot, device: DeviceIdx) -> Option<&GpuTotals> {
    let gpus = &snap.system.gpus;
    gpus.binary_search_by_key(&device, |(i, _)| *i)
        .ok()
        .map(|i| &gpus[i].1)
}

/// The `last_error` of the GPU source when the probe binary is missing. The engine's
/// `gpu::NOT_INSTALLED` holds the same text.
pub const PROBE_NOT_INSTALLED: &str = "GPU probe not installed";

/// The state of the NVIDIA probe as the GPU page shows it (PERF-062). Moved from the GPU page
/// (`tasklens-hzire.4`), so the page's chart widgets show it too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProbeState {
    #[default]
    Ok,
    /// The GPU source is in backoff: values `!`, plots stop.
    Restarting,
    /// The probe binary is not installed: values `n/a`, one placeholder engine cell.
    Missing,
}

/// The probe state for a GPU of `vendor` from the snapshot's source health. AMD, Intel and
/// unknown GPUs are always `Ok`.
pub fn probe_state(vendor: GpuVendor, snap: &Snapshot) -> ProbeState {
    if vendor != GpuVendor::Nvidia {
        return ProbeState::Ok;
    }
    let h = snap.health.get(SourceId::GpuNvidia);
    match h.state {
        SourceState::Failed | SourceState::Disabled { .. } => ProbeState::Restarting,
        SourceState::Unsupported
            if h.last_error
                .as_deref()
                .is_some_and(|e| e.contains(PROBE_NOT_INSTALLED)) =>
        {
            ProbeState::Missing
        }
        _ => ProbeState::Ok,
    }
}

/// The `en-US.ftl` key of the tooltip every GPU chart shows in `probe`, `None` while it is Ok.
pub fn probe_tooltip_key(probe: ProbeState) -> Option<&'static str> {
    match probe {
        ProbeState::Ok => None,
        ProbeState::Restarting => Some(KEY_RESTARTING),
        ProbeState::Missing => Some(KEY_MISSING),
    }
}

/// "The GPU probe is restarting" and "The GPU probe is not installed".
pub const KEY_RESTARTING: &str = "perf-gpu-probe-restarting";
pub const KEY_MISSING: &str = "perf-gpu-probe-missing";

/// The `en-US.ftl` keys of the reasons the clocks are held down, in bit order. Idle is not a
/// throttle, so it has no word.
const REASON_WORDS: [(ClockEventReasons, &str); 8] = [
    (ClockEventReasons::APP_CLOCKS, "perf-gpu-reason-app-clocks"),
    (ClockEventReasons::POWER_CAP, "perf-gpu-reason-power-cap"),
    (
        ClockEventReasons::HW_SLOWDOWN,
        "perf-gpu-reason-hw-slowdown",
    ),
    (ClockEventReasons::SYNC_BOOST, "perf-gpu-reason-sync-boost"),
    (ClockEventReasons::SW_THERMAL, "perf-gpu-reason-sw-thermal"),
    (ClockEventReasons::HW_THERMAL, "perf-gpu-reason-hw-thermal"),
    (
        ClockEventReasons::HW_POWER_BRAKE,
        "perf-gpu-reason-hw-power-brake",
    ),
    (
        ClockEventReasons::DISPLAY_CLOCKS,
        "perf-gpu-reason-display-clocks",
    ),
];

fn shown<T: Copy + Default>(m: Metric<T>) -> Option<T> {
    matches!(m.validity, Validity::Valid | Validity::Stale).then_some(m.value)
}

/// The Power heading's tooltip (UI-PRF-032): "Limit range 100 to 350 W · 1.24 kWh since
/// boot"; "max 350 W · 1.24 kWh since boot" while the card's lowest limit is unknown. `None`
/// without the highest limit or the energy.
pub fn power_tooltip_text(totals: Option<&GpuTotals>, cx: &App) -> Option<SharedString> {
    let t = totals?;
    let max = t.info.power_max_limit_w?;
    let joules = shown(t.energy_since_boot)?;
    let arg = |v: String| FluentValue::from(v);
    let kwh = ("kwh", arg(format!("{:.2}", joules / 3.6e6)));
    let max = ("max", arg(format!("{max:.0}")));
    Some(match t.info.power_min_limit_w {
        Some(min) => tr_args(
            cx,
            "perf-gpu-power-tooltip",
            &[("min", arg(format!("{min:.0}"))), max, kwh],
        ),
        None => tr_args(cx, "perf-gpu-power-row-tooltip", &[max, kwh]),
    })
}

/// The Clocks heading's tooltip (UI-PRF-032): "P3 · Throttled: power cap", reasons joined by
/// ", "; "P3" while none is set or the reasons are not reported. `None` without a P-state.
pub fn clocks_tooltip_text(totals: Option<&GpuTotals>, cx: &App) -> Option<SharedString> {
    let t = totals?;
    let pstate = ("pstate", FluentValue::from(shown(t.pstate)?.to_string()));
    let reasons = shown(t.clock_event_reasons).unwrap_or_default();
    let words: Vec<SharedString> = REASON_WORDS
        .iter()
        .filter(|(bit, _)| reasons.contains(*bit))
        .map(|(_, key)| tr(cx, key))
        .collect();
    if words.is_empty() {
        return Some(tr_args(cx, "perf-gpu-clocks-tooltip-plain", &[pstate]));
    }
    let joined = words
        .iter()
        .map(|w| w.as_ref())
        .collect::<Vec<_>>()
        .join(", ");
    Some(tr_args(
        cx,
        "perf-gpu-clocks-tooltip",
        &[pstate, ("reasons", FluentValue::from(joined))],
    ))
}
