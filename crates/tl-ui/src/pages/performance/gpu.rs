//! The GPU page: the engine grid, the Memory and Temperature charts side by side, and the probe
//! states (docs/ui/performance.md UI-PRF-032; PERF-062 to PERF-065, PERF-068; R-054).
//!
//! The page re-renders only when its header, probe state, engine list or Details rows change.
//! The engine grid is one `ChartGridView` (ADR-009 rule 7) and each chart is a cached view that
//! re-renders for its own series. A sample that changes nothing the page draws costs one compare
//! of the reported engine bits and the memory totals.
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, AppContext as _, AsyncApp,
    Context, Div, ElementId, Entity, EntityId, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    Point, Render, SharedString, StyleRefinement, Styled as _, Subscription, WeakEntity, Window,
    canvas, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::devices::{DeviceEntry, DeviceIdentity};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::GpuOptionalGraph;
use tl_model::snapshot::{DisplayMode, GraphicsAdapter, Snapshot};
use tl_model::system::{ClockEventReasons, GpuEngine, GpuTotals, Metric};
use tl_model::time::now_boot_ns;
use tl_model::units;
use tl_model::validity::Validity;

use super::actions::{PerfToggleGpuEngine, PerfToggleGpuOptionalGraph};
use super::details::{Fallback, KeyValueModel};
use super::menu::ClickedEngine;
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_STACKED, header_text, page_words};
use super::{DeviceKey, DeviceKind, DeviceRow};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartGridSpec, ChartGridView, ChartLabels, ChartSeries, ChartSpec, ChartState,
    ChartView, GridCellSpec, GridStyle, LegendStyle, ValueAlign, ValueFormat, YScale, grid_height,
    grid_layout,
};
use crate::components::menu::TlMenu;
use crate::components::motion::{M_BASE, ease_out_cubic};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args, tr_en};
use crate::settings_global::{Section, TlSettings, animations_on};
use crate::text::style::label_style;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// `grid_cell_h`, × ui_scale.
pub const ENGINE_CELL_H: f32 = 112.;
/// Minimum engine cell width, × ui_scale.
pub const ENGINE_CELL_MIN_W: f32 = 160.;

/// Debug selector of the engine grid box.
pub const ENGINES_ID: &str = "perf-gpu-engines";
/// Debug selector of the Memory chart box.
pub const MEMORY_ID: &str = "perf-gpu-memory";
/// Debug selector of the Temperature chart box.
pub const TEMPERATURE_ID: &str = "perf-gpu-temperature";
/// Debug selector of the Power chart box.
pub const POWER_ID: &str = "perf-gpu-power";
/// Debug selector of the Clocks chart box.
pub const CLOCKS_ID: &str = "perf-gpu-clocks";
/// The trigger over the Power heading.
pub const POWER_TIP_ID: &str = "perf-gpu-power-tooltip";
/// The trigger over the Clocks heading.
pub const CLOCKS_TIP_ID: &str = "perf-gpu-clocks-tooltip";

/// The `last_error` of the GPU source when the probe binary is missing. The engine's
/// `gpu::NOT_INSTALLED` holds the same text.
pub const PROBE_NOT_INSTALLED: &str = "GPU probe not installed";

const KEY_MEMORY: &str = "perf-gpu-heading-memory";
const KEY_TEMPERATURE: &str = "perf-gpu-heading-temperature";
const KEY_MEMORY_USAGE: &str = "perf-gpu-range-memory";
const KEY_DEDICATED: &str = "perf-gpu-legend-dedicated";
const KEY_SHARED: &str = "perf-gpu-legend-shared";
const KEY_POWER: &str = "perf-gpu-heading-power";
const KEY_CLOCKS: &str = "perf-gpu-heading-clocks";
const KEY_POWER_DRAW: &str = "perf-gpu-row-power";
const KEY_CLOCK_SPEED: &str = "perf-gpu-row-clock";
const KEY_GPU_CLOCK: &str = "perf-gpu-legend-gpu-clock";
const KEY_MEM_CLOCK: &str = "perf-gpu-legend-mem-clock";
const KEY_RESTARTING: &str = "perf-gpu-probe-restarting";
const KEY_MISSING: &str = "perf-gpu-probe-missing";

const NS_PER_S: u64 = 1_000_000_000;

/// The trigger over a hovered engine heading that has a tooltip.
pub const ENGINE_TIP_ID: &str = "perf-gpu-engine-tooltip";

/// Width used for the grid height before the first paint has measured the box.
const UNMEASURED_W: f32 = 1.0e6;

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

/// The state of the NVIDIA probe as the page shows it (PERF-062).
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

/// One engine: its settings key, its name's `en-US.ftl` key and its series.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Engine {
    pub(crate) key: &'static str,
    pub(crate) name: &'static str,
    pub(crate) metric: MetricId,
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
struct EngineCell {
    engine: GpuEngine,
    name: &'static str,
}

const fn cell(engine: GpuEngine, name: &'static str) -> EngineCell {
    EngineCell { engine, name }
}

const NAME_3D: &str = "perf-gpu-engine-3d";

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

/// Every engine name key and the "Own processes only" tooltip key, for [`Words`].
const WORD_KEYS: [&str; 14] = [
    NAME_3D,
    "perf-gpu-engine-memctl",
    "perf-gpu-engine-enc",
    "perf-gpu-engine-dec",
    "perf-gpu-engine-graphics",
    "perf-gpu-engine-compute",
    "perf-gpu-engine-dma",
    "perf-gpu-engine-jpeg",
    "perf-gpu-engine-ofa",
    "perf-gpu-engine-render",
    "perf-gpu-engine-copy",
    "perf-gpu-engine-video",
    "perf-gpu-engine-video-enhance",
    "perf-gpu-tip-own-processes",
];

/// The grid engines of `vendor` in grid order.
fn vendor_cells(vendor: GpuVendor) -> &'static [EngineCell] {
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
struct Reported {
    mask: u16,
    partial: bool,
}

impl Reported {
    fn of(vendor: GpuVendor, totals: Option<&GpuTotals>) -> Self {
        Self {
            mask: engine_mask(vendor, totals),
            partial: totals.is_some_and(|t| t.fdinfo_partial),
        }
    }

    fn has(self, engine: GpuEngine) -> bool {
        self.mask & engine_bit(engine) != 0
    }
}

/// The engines that AMD reads only from fdinfo, so "Own processes only" applies to them.
fn fdinfo_engine(engine: GpuEngine) -> bool {
    matches!(
        engine,
        GpuEngine::Compute | GpuEngine::Dma | GpuEngine::Jpeg
    )
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
pub(crate) fn shown_engines<'a, H: AsRef<str>>(
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
pub(crate) fn grid_engines<'a, H: AsRef<str>>(
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

/// Columns of the engine grid: `ceil(sqrt(2n))` capped by `floor(width / min_cell_w)`, at
/// least 1.
pub fn engine_columns(n: usize, width_px: f32, min_cell_w: f32) -> usize {
    let mut want = 0usize;
    while want * want < 2 * n {
        want += 1;
    }
    let fit = if min_cell_w > 0.0 {
        (width_px / min_cell_w).floor().max(0.0) as usize
    } else {
        usize::MAX
    };
    want.min(fit).max(1)
}

/// The engines a GPU reports, as a mask of [`Engine::bit`]: every engine whose metric is not
/// Unsupported. The memory controller counts only when the record's engine list reports it. A GPU
/// without a record yet reports the three P1 engines, so the page keeps its shape.
fn reported_bits(totals: Option<&GpuTotals>) -> u8 {
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

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    /// The engine names and the tooltip, by `en-US.ftl` key ([`WORD_KEYS`]).
    pub engines: Vec<(&'static str, SharedString)>,
    pub memory: SharedString,
    pub temperature: SharedString,
    pub memory_usage: SharedString,
    pub dedicated: SharedString,
    pub shared: SharedString,
    pub power: SharedString,
    pub clocks: SharedString,
    pub power_draw: SharedString,
    pub clock_speed: SharedString,
    pub gpu_clock: SharedString,
    pub mem_clock: SharedString,
    pub restarting: SharedString,
    pub missing: SharedString,
    pub displays: SharedString,
    pub no_display: SharedString,
}

impl Words {
    /// Translates every word when the strings changed since the last call. True when it did.
    fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return false;
        }
        self.version = version;
        let t = |key| tr(cx, key);
        self.engines = WORD_KEYS.iter().map(|k| (*k, t(k))).collect();
        self.memory = t(KEY_MEMORY);
        self.temperature = t(KEY_TEMPERATURE);
        self.memory_usage = t(KEY_MEMORY_USAGE);
        self.dedicated = t(KEY_DEDICATED);
        self.shared = t(KEY_SHARED);
        self.power = t(KEY_POWER);
        self.clocks = t(KEY_CLOCKS);
        self.power_draw = t(KEY_POWER_DRAW);
        self.clock_speed = t(KEY_CLOCK_SPEED);
        self.gpu_clock = t(KEY_GPU_CLOCK);
        self.mem_clock = t(KEY_MEM_CLOCK);
        self.restarting = t(KEY_RESTARTING);
        self.missing = t(KEY_MISSING);
        self.displays = t("perf-gpu-block-displays");
        self.no_display = t("perf-gpu-no-display");
        true
    }

    /// The word of the `en-US.ftl` key `name`; empty before the first refresh.
    fn engine(&self, name: &str) -> SharedString {
        self.engines
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, w)| w.clone())
            .unwrap_or_default()
    }

    /// The tooltip of every GPU chart value in `probe`.
    pub fn probe_tooltip(&self, probe: ProbeState) -> Option<&SharedString> {
        match probe {
            ProbeState::Ok => None,
            ProbeState::Restarting => Some(&self.restarting),
            ProbeState::Missing => Some(&self.missing),
        }
    }
}

/// The settings the page draws from.
#[derive(Clone, Debug, PartialEq)]
struct PageSettings {
    range_ns: u64,
    smooth: bool,
    rounded: bool,
    hidden: Vec<Arc<str>>,
    /// `performance.gpu_optional_graphs`.
    optional: Vec<GpuOptionalGraph>,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
            hidden: s
                .performance
                .hidden_gpu_engines
                .iter()
                .map(|k| Arc::from(k.as_str()))
                .collect(),
            optional: s.performance.gpu_optional_graphs.clone(),
        }
    }
}

/// The versions of the sections [`PageSettings`] reads.
fn section_versions(cx: &App) -> [u64; 3] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Appearance),
    ]
}

/// The memory facts the Memory chart is built from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct MemoryShape {
    /// VRAM total in bytes, 0 when unknown.
    total: u64,
    /// The device reports shared memory (never on NVIDIA).
    shared: bool,
}

impl MemoryShape {
    fn of(totals: Option<&GpuTotals>) -> Self {
        let Some(t) = totals else {
            return Self::default();
        };
        let total = match t.mem_total.validity {
            Validity::Valid | Validity::Stale => t.mem_total.value,
            _ => 0,
        };
        Self {
            total,
            shared: t.shared_total.validity == Validity::Valid && t.shared_total.value > 0,
        }
    }
}

fn plot_grid(s: &PageSettings) -> GridStyle {
    GridStyle {
        lines: true,
        baseline: false,
        outline: true,
        rounded: s.rounded,
        vertical_at: None,
    }
}

fn series(device: DeviceIdx, metric: MetricId) -> SeriesId {
    SeriesId { device, metric }
}

/// The Memory chart: dedicated filled, shared as a dashed line when the device reports it,
/// 0..VRAM total.
pub fn memory_spec(device: DeviceIdx, shared: bool, total: u64, theme: &TlTheme) -> ChartSpec {
    let mut series_list = smallvec![ChartSeries {
        id: series(device, MetricId::GPU_MEM),
        fill: Some(theme.graph.gpu_fill),
        line: theme.graph.gpu_line,
        ..ChartSeries::default()
    }];
    if shared {
        series_list.push(ChartSeries {
            id: series(device, MetricId::GPU_MEM_SHARED),
            fill: None,
            line: theme.graph.gpu_line,
            dashed: true,
            ..ChartSeries::default()
        });
    }
    let y = if total > 0 {
        YScale::FixedTotal(total as f32)
    } else {
        YScale::StickyUp { hw_max: None }
    };
    ChartSpec {
        series: series_list,
        y,
        background: ChartBg::Window,
        hover: true,
        ..ChartSpec::default()
    }
}

/// The Memory chart's labels; the "Shared" legend entry only with the shared series.
pub fn memory_labels(shared: bool, words: &Words) -> ChartLabels {
    let mut legend = smallvec![(words.dedicated.clone(), LegendStyle::Solid)];
    if shared {
        legend.push((words.shared.clone(), LegendStyle::Dashed));
    }
    ChartLabels {
        heading: Some(words.memory.clone()),
        value_fmt: ValueFormat::Bytes,
        value_align: ValueAlign::Right,
        range_word: words.memory_usage.clone(),
        legend,
        show_axis_max: false,
    }
}

fn temperature_spec(device: DeviceIdx, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: series(device, MetricId::GPU_TEMP),
            fill: Some(theme.graph.gpu_fill),
            line: theme.graph.gpu_line,
            ..ChartSeries::default()
        }],
        y: YScale::StickyTemp,
        background: ChartBg::Window,
        hover: true,
        ..ChartSpec::default()
    }
}

fn temperature_labels(words: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(words.temperature.clone()),
        value_fmt: ValueFormat::Celsius,
        value_align: ValueAlign::Right,
        range_word: words.temperature.clone(),
        legend: SmallVec::new(),
        show_axis_max: true,
    }
}

/// The device facts the Power and Clocks scales are built from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct OptionalShape {
    /// The enforced power limit in W, else the card's highest limit.
    power_max: Option<f32>,
    /// The maximum graphics clock in MHz.
    clock_max: Option<u32>,
}

impl OptionalShape {
    fn of(totals: Option<&GpuTotals>) -> Self {
        let Some(t) = totals else {
            return Self::default();
        };
        Self {
            power_max: shown(t.power_limit_w).or(t.info.power_max_limit_w),
            clock_max: t.info.clock_gfx_max_mhz,
        }
    }
}

/// The Power chart: the draw filled, 0 up to the power limit.
pub fn power_spec(device: DeviceIdx, hw_max: Option<f32>, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: series(device, MetricId::GPU_POWER),
            fill: Some(theme.graph.gpu_fill),
            line: theme.graph.gpu_line,
            ..ChartSeries::default()
        }],
        y: YScale::StickyUp { hw_max },
        background: ChartBg::Window,
        hover: true,
        ..ChartSpec::default()
    }
}

pub fn power_labels(words: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(words.power.clone()),
        value_fmt: ValueFormat::Watts,
        value_align: ValueAlign::Right,
        range_word: words.power_draw.clone(),
        legend: SmallVec::new(),
        show_axis_max: true,
    }
}

/// The Clocks chart: the GPU clock filled, the memory clock as a dashed line, 0 up to the maximum
/// graphics clock.
pub fn clocks_spec(device: DeviceIdx, hw_max: Option<f32>, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![
            ChartSeries {
                id: series(device, MetricId::GPU_CLOCK),
                fill: Some(theme.graph.gpu_fill),
                line: theme.graph.gpu_line,
                ..ChartSeries::default()
            },
            ChartSeries {
                id: series(device, MetricId::GPU_MEM_CLOCK),
                fill: None,
                line: theme.graph.gpu_line,
                dashed: true,
                ..ChartSeries::default()
            },
        ],
        y: YScale::StickyUp { hw_max },
        background: ChartBg::Window,
        hover: true,
        ..ChartSpec::default()
    }
}

pub fn clocks_labels(words: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(words.clocks.clone()),
        value_fmt: ValueFormat::Mhz,
        value_align: ValueAlign::Right,
        range_word: words.clock_speed.clone(),
        legend: smallvec![
            (words.gpu_clock.clone(), LegendStyle::Solid),
            (words.mem_clock.clone(), LegendStyle::Dashed),
        ],
        show_axis_max: false,
    }
}

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

/// The placeholder cell shown in place of the grid while the probe is missing: "3D / Compute"
/// reading `n/a`.
fn placeholder_labels(words: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(words.engine(NAME_3D)),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: SharedString::default(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// Applies the range, smoothing and corner settings to a chart spec.
fn with_settings(mut spec: ChartSpec, s: &PageSettings) -> ChartSpec {
    spec.range_ns = s.range_ns;
    spec.smooth = s.smooth;
    spec.grid = plot_grid(s);
    spec
}

/// The chart state every GPU chart shows in `probe`.
fn probe_chart_state(probe: ProbeState, words: &Words) -> ChartState {
    match probe {
        ProbeState::Ok => ChartState::Live,
        ProbeState::Restarting => ChartState::Failed(words.restarting.clone()),
        ProbeState::Missing => ChartState::Unsupported,
    }
}

/// The GPU device page.
pub struct GpuPage {
    key: DeviceKey,
    row: DeviceRow,
    vendor: GpuVendor,
    engines: Entity<ChartGridView>,
    /// Engine keys in grid order, one per grid cell.
    engine_keys: Vec<Arc<str>>,
    /// The engines the last grid build drew from, and the "Own processes only" flag.
    reported: Reported,
    /// The one-cell "3D / Compute" chart shown while the probe is missing. Created the first
    /// time the probe is missing.
    placeholder: Option<Entity<ChartView>>,
    memory: Entity<ChartView>,
    memory_shape: MemoryShape,
    /// Reused buffer for the Memory value text.
    memory_text: String,
    temperature: Entity<ChartView>,
    /// The optional Power chart, present while `performance.gpu_optional_graphs` lists it.
    power: Option<Entity<ChartView>>,
    /// The optional Clocks chart.
    clocks: Option<Entity<ChartView>>,
    optional_shape: OptionalShape,
    /// The heading tooltips: "Limit range 100 to 350 W · 1.24 kWh since boot" and
    /// "P3 · Throttled: power cap".
    power_tip: Option<SharedString>,
    clocks_tip: Option<SharedString>,
    details: KeyValueModel,
    probe: ProbeState,
    focus: FocusHandle,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The page is the shown page (`set_active`).
    active: bool,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    /// `UiMetrics.style_version` the chart colours and grid sizes were built for.
    style_version: u64,
    words: Words,
    /// One formatted row per display of this GPU's adapter (PERF-073).
    display_rows: Vec<SharedString>,
    /// The engine box width of the last paint; 0 before the first.
    width: Rc<Cell<Pixels>>,
    /// The engine box's top-left corner in the window at the last paint, for the cell under a
    /// right-click.
    origin: Rc<Cell<Point<Pixels>>>,
    /// The number of cells the grid was last built with.
    shown_cells: usize,
    /// Row 1 growing from or shrinking to height 0 (UI-PRF-032).
    row_anim: Option<RowAnim>,
    anim_generation: u32,
    _subs: SmallVec<[Subscription; 3]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

/// Row 1's height animation after the last engine is hidden or the first is shown again.
#[derive(Clone, Copy, Debug)]
struct RowAnim {
    showing: bool,
    /// The cell count the full height is measured for.
    cells: usize,
    /// Bumped on each change, so a new change restarts the animation.
    generation: u32,
}

impl GpuPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = current(cx).clone();
        let settings = PageSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let vendor = GpuVendor::of(&snap, row.idx);
        let totals = gpu_totals(&snap, row.idx);
        let mut words = Words::default();
        words.refresh(cx);

        let reported = Reported::of(vendor, totals);
        let mut engine_keys = Vec::new();
        let spec = grid_spec(
            row.idx,
            vendor,
            reported,
            &settings,
            &words,
            &theme,
            metrics(cx),
            &mut engine_keys,
        );
        let shown_cells = spec.cells.len();
        let engines = cx.new(|cx| ChartGridView::new(spec, &hub, cx));

        let memory_shape = MemoryShape::of(totals);
        let spec = with_settings(
            memory_spec(row.idx, memory_shape.shared, memory_shape.total, &theme),
            &settings,
        );
        let labels = memory_labels(memory_shape.shared, &words);
        let memory = cx.new(|cx| ChartView::new(spec, labels, &hub, cx).with_newest_marks());
        let spec = with_settings(temperature_spec(row.idx, &theme), &settings);
        let labels = temperature_labels(&words);
        let temperature = cx.new(|cx| ChartView::new(spec, labels, &hub, cx).with_newest_marks());

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<UiMetrics>(Self::sync_style));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            vendor,
            engines,
            engine_keys,
            reported,
            placeholder: None,
            memory,
            memory_shape,
            memory_text: String::new(),
            temperature,
            power: None,
            clocks: None,
            optional_shape: OptionalShape::default(),
            power_tip: None,
            clocks_tip: None,
            details: gpu_details_model(),
            probe: ProbeState::Ok,
            focus: cx.focus_handle(),
            hub,
            demand,
            id,
            active: false,
            window: None,
            header: (SharedString::default(), SharedString::default()),
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            words,
            display_rows: Vec::new(),
            width: Rc::new(Cell::new(px(0.))),
            origin: Rc::new(Cell::new(Point::default())),
            shown_cells,
            row_anim: None,
            anim_generation: 0,
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    pub fn vendor(&self) -> GpuVendor {
        self.vendor
    }

    pub fn probe(&self) -> ProbeState {
        self.probe
    }

    pub fn engines(&self) -> &Entity<ChartGridView> {
        &self.engines
    }

    /// Engine keys in grid order.
    pub fn engine_keys(&self) -> &[Arc<str>] {
        &self.engine_keys
    }

    /// The "3D / Compute" placeholder cell, once the probe has been missing.
    pub fn placeholder(&self) -> Option<&Entity<ChartView>> {
        self.placeholder.as_ref()
    }

    pub fn memory(&self) -> &Entity<ChartView> {
        &self.memory
    }

    pub fn temperature(&self) -> &Entity<ChartView> {
        &self.temperature
    }

    /// The Power chart while it is enabled.
    pub fn power(&self) -> Option<&Entity<ChartView>> {
        self.power.as_ref()
    }

    /// The Clocks chart while it is enabled.
    pub fn clocks(&self) -> Option<&Entity<ChartView>> {
        self.clocks.as_ref()
    }

    pub fn power_tooltip(&self) -> Option<&SharedString> {
        self.power_tip.as_ref()
    }

    pub fn clocks_tooltip(&self) -> Option<&SharedString> {
        self.clocks_tip.as_ref()
    }

    /// The Displays block rows, one per display of this GPU's adapter.
    pub fn display_rows(&self) -> &[SharedString] {
        &self.display_rows
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// The tooltip every GPU chart value shows in the current probe state.
    pub fn probe_tooltip(&self) -> Option<&SharedString> {
        self.words.probe_tooltip(self.probe)
    }

    /// This page's demand part while it is active: the GPU component, and the inventory for the
    /// adapter's vendor and static rows.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            gpus: true,
            inventory: true,
            ..Demand::default()
        }
    }

    fn send_demand(&self, cx: &mut App) {
        if !self.active {
            return;
        }
        let (id, window, part) = (self.id, self.window, self.demand_part());
        self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let snap = Arc::clone(hub.read(cx).current());
        self.apply_snapshot(&snap, ev.mask, cx);
    }

    /// Brings the engine cells, Memory chart, probe state and header up to date with the
    /// components in `mask`. Notifies only when something the page draws changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let mut changed = false;
        let devices = mask.contains(ComponentMask::DEVICES);
        let inventory = mask.contains(ComponentMask::INVENTORY);
        // A new vendor can change the engine names and whether probe states apply.
        let mut vendor_changed = false;
        if inventory || devices {
            let vendor = GpuVendor::of(snap, self.row.idx);
            if vendor != self.vendor {
                self.vendor = vendor;
                vendor_changed = true;
                self.rebuild_grid(cx);
                changed = true;
            }
        }

        if mask.contains(ComponentMask::SYSTEM) {
            let totals = gpu_totals(snap, self.row.idx);
            let reported = Reported::of(self.vendor, totals);
            if reported != self.reported {
                self.reported = reported;
                self.rebuild_grid(cx);
                changed = true;
            }
            let shape = MemoryShape::of(totals);
            if shape != self.memory_shape {
                let labels_changed = shape.shared != self.memory_shape.shared;
                self.memory_shape = shape;
                self.rebuild_memory(labels_changed, cx);
            }
            self.sync_memory_text(totals, cx);
        }
        if mask.contains(ComponentMask::HEALTH) || vendor_changed {
            let probe = probe_state(self.vendor, snap);
            if probe != self.probe {
                self.probe = probe;
                self.apply_probe(cx);
                changed = true;
            }
        }
        if mask.intersects(
            ComponentMask::SYSTEM
                | ComponentMask::INVENTORY
                | ComponentMask::HEALTH
                | ComponentMask::DEVICES,
        ) || vendor_changed
        {
            let (uuid, pci_bus_id) =
                match snap.devices.entries.iter().find(|e| e.idx == self.row.idx) {
                    Some(DeviceEntry {
                        identity: DeviceIdentity::Gpu { uuid, pci_bus_id },
                        ..
                    }) => (Some(*uuid), Some(&**pci_bus_id)),
                    _ => (None, None),
                };
            let inputs = GpuDetailInputs {
                now_boot_ns: now_boot_ns(),
                observed_end_ns: snap.system.header.observed_end,
                totals: gpu_totals(snap, self.row.idx),
                adapter: adapter(snap, self.row.idx),
                vendor: self.vendor,
                probe: self.probe,
                uuid,
                pci_bus_id,
            };
            changed |= update_gpu_details_p1(&mut self.details, &inputs);
            changed |= update_gpu_details_p2(&mut self.details, &inputs);
            changed |= self.sync_optional(snap, false, cx);
        }
        if inventory {
            let rows: Vec<SharedString> = adapter(snap, self.row.idx)
                .map(|a| {
                    a.displays
                        .iter()
                        .map(|d| display_row_with(d, &|k, a| tr_args(cx, k, a)))
                        .collect()
                })
                .unwrap_or_default();
            if rows != self.display_rows {
                self.display_rows = rows;
                changed = true;
            }
        }
        if (devices || inventory)
            && let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx)
        {
            let header = header_text(page_words(cx), DeviceKind::Gpu, 0, entry, snap);
            if header != self.header {
                self.header = header;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Sets every GPU chart's state for the probe state, creating the placeholder cell the first
    /// time the probe is missing.
    fn apply_probe(&mut self, cx: &mut Context<Self>) {
        let state = probe_chart_state(self.probe, &self.words);
        if self.probe == ProbeState::Missing && self.placeholder.is_none() {
            let theme = current(cx).clone();
            let spec = with_settings(placeholder_spec(self.row.idx, &theme), &self.settings);
            let labels = placeholder_labels(&self.words);
            let hub = self.hub.clone();
            self.placeholder =
                Some(cx.new(|cx| ChartView::new(spec, labels, &hub, cx).with_newest_marks()));
        }
        for view in [&self.memory, &self.temperature]
            .into_iter()
            .chain(self.placeholder.as_ref())
        {
            let state = state.clone();
            view.update(cx, |v, cx| v.set_state(state, cx));
        }
        // Restarting keeps the per-engine cells, each reading `!`. Missing shows the placeholder.
        self.engines.update(cx, |v, cx| v.set_state(state, cx));
    }

    /// The Memory value as "2.80 GiB / 15.92 GiB" when used and total are known. The chart gets a
    /// new text only when the formatted text differs.
    fn sync_memory_text(&mut self, totals: Option<&GpuTotals>, cx: &mut Context<Self>) {
        let total = self.memory_shape.total;
        let used = totals.and_then(|t| shown_u64(t.mem_used.value, t.mem_used.validity));
        let Some(used) = used.filter(|_| total > 0) else {
            self.memory.update(cx, |v, cx| v.set_value_text(None, cx));
            return;
        };
        self.memory_text.clear();
        units::bytes_of(&mut self.memory_text, used, total);
        let text = &self.memory_text;
        if self.memory.read(cx).value_text().map(|t| t.as_ref()) == Some(text.as_str()) {
            return;
        }
        let text = SharedString::from(text.clone());
        self.memory
            .update(cx, |v, cx| v.set_value_text(Some(text), cx));
    }

    fn rebuild_grid(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let spec = grid_spec(
            self.row.idx,
            self.vendor,
            self.reported,
            &self.settings,
            &self.words,
            &theme,
            metrics(cx),
            &mut self.engine_keys,
        );
        let new = spec.cells.len();
        self.engines.update(cx, |v, cx| v.set_spec(spec, cx));
        self.sync_row_anim(new, cx);
    }

    /// Starts row 1's height animation when the cell count moved between 0 and more over
    /// `m_base`; with animations off the height jumps.
    fn sync_row_anim(&mut self, new: usize, cx: &mut Context<Self>) {
        let old = std::mem::replace(&mut self.shown_cells, new);
        if (old == 0) == (new == 0) {
            return;
        }
        self.anim_generation = self.anim_generation.wrapping_add(1);
        if !animations_on(cx) {
            self.row_anim = None;
            return;
        }
        let generation = self.anim_generation;
        self.row_anim = Some(RowAnim {
            showing: new > 0,
            cells: old.max(new),
            generation,
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(M_BASE).await;
            let _ = this.update(cx, |this, cx| {
                if this.anim_generation == generation {
                    this.row_anim = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// `PerfToggleGpuEngine`: hides or shows the graph of one engine in
    /// `performance.hidden_gpu_engines`. The page follows through its settings observer.
    fn on_toggle_engine(
        &mut self,
        a: &PerfToggleGpuEngine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if a.key != self.key {
            return;
        }
        let engine = GpuEngine::from_key(&a.engine);
        TlSettings::update(cx, |d| {
            let hidden = &mut d.settings.performance.hidden_gpu_engines;
            match engine {
                Some(e) => {
                    let hide = !e.is_hidden(hidden);
                    e.set_hidden(hidden, hide);
                }
                None => match hidden.iter().position(|k| **k == *a.engine) {
                    Some(i) => {
                        hidden.remove(i);
                    }
                    None => hidden.push(a.engine.to_string()),
                },
            }
        });
        self.sync_settings(cx);
    }

    /// Creates or drops the Power and Clocks charts to match the setting, and brings their
    /// scale, value text, state and heading tooltip up to date. `force` rebuilds the specs (a
    /// new setting, theme or scale). True when something the page draws outside the charts
    /// changed.
    fn sync_optional(&mut self, snap: &Snapshot, force: bool, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let totals = gpu_totals(snap, self.row.idx);
        let shape = OptionalShape::of(totals);
        let rescale = force || shape != self.optional_shape;
        self.optional_shape = shape;
        let theme = current(cx).clone();
        let hub = self.hub.clone();
        let idx = self.row.idx;
        let want = |g| self.settings.optional.contains(&g);

        if want(GpuOptionalGraph::Power) {
            let spec = || with_settings(power_spec(idx, shape.power_max, &theme), &self.settings);
            match &self.power {
                Some(v) if rescale => {
                    let spec = spec();
                    v.update(cx, |v, cx| v.set_spec(spec, cx));
                }
                Some(_) => {}
                None => {
                    let (spec, labels) = (spec(), power_labels(&self.words));
                    self.power = Some(
                        cx.new(|cx| ChartView::new(spec, labels, &hub, cx).with_newest_marks()),
                    );
                    changed = true;
                }
            }
        } else if self.power.take().is_some() {
            self.power_tip = None;
            changed = true;
        }
        if want(GpuOptionalGraph::Clocks) {
            let spec = || {
                let max = shape.clock_max.map(|m| m as f32);
                with_settings(clocks_spec(idx, max, &theme), &self.settings)
            };
            match &self.clocks {
                Some(v) if rescale => {
                    let spec = spec();
                    v.update(cx, |v, cx| v.set_spec(spec, cx));
                }
                Some(_) => {}
                None => {
                    let (spec, labels) = (spec(), clocks_labels(&self.words));
                    self.clocks = Some(
                        cx.new(|cx| ChartView::new(spec, labels, &hub, cx).with_newest_marks()),
                    );
                    changed = true;
                }
            }
        } else if self.clocks.take().is_some() {
            self.clocks_tip = None;
            changed = true;
        }

        let probe_state = probe_chart_state(self.probe, &self.words);
        let mut buf = String::new();
        if let Some(view) = self.power.clone() {
            let power = totals.map_or(missing(), |t| t.power_w);
            let inputs = GpuDetailInputs {
                now_boot_ns: 0,
                observed_end_ns: 0,
                totals,
                adapter: adapter(snap, idx),
                vendor: self.vendor,
                probe: self.probe,
                uuid: None,
                pci_bus_id: None,
            };
            // An Intel iGPU reads the CPU package meter, which needs setup.
            let state = if self.probe == ProbeState::Ok
                && is_intel_igpu(&inputs)
                && matches!(power.validity, Validity::Restricted | Validity::Unsupported)
            {
                ChartState::Unsupported
            } else {
                probe_state.clone()
            };
            let limit = totals.and_then(|t| shown(t.power_limit_w));
            let text = shown(power).zip(limit).map(|(p, l)| {
                units::watts_pair(&mut buf, p, l);
                SharedString::from(buf.clone())
            });
            view.update(cx, |v, cx| {
                v.set_state(state, cx);
                v.set_value_text(text, cx);
            });
            let tip = self.power_tooltip_text(totals, cx);
            if tip != self.power_tip {
                self.power_tip = tip;
                changed = true;
            }
        }
        if let Some(view) = self.clocks.clone() {
            let now = totals.and_then(|t| shown(t.clock_gfx_mhz));
            let text = now.zip(shape.clock_max).map(|(c, m)| {
                buf.clear();
                units::mhz_pair(&mut buf, c as f32, m as f32);
                SharedString::from(buf.clone())
            });
            view.update(cx, |v, cx| {
                v.set_state(probe_state, cx);
                v.set_value_text(text, cx);
            });
            let tip = self.clocks_tooltip_text(totals, cx);
            if tip != self.clocks_tip {
                self.clocks_tip = tip;
                changed = true;
            }
        }
        changed
    }

    /// "Limit range 100 to 350 W · 1.24 kWh since boot"; "max 350 W · 1.24 kWh since boot" while
    /// the card's lowest limit is unknown. None without the highest limit or the energy.
    fn power_tooltip_text(&self, totals: Option<&GpuTotals>, cx: &App) -> Option<SharedString> {
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

    /// "P3 · Throttled: power cap", reasons joined by ", "; "P3" while none is set or the
    /// reasons are not reported. None without a P-state.
    fn clocks_tooltip_text(&self, totals: Option<&GpuTotals>, cx: &App) -> Option<SharedString> {
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

    /// `PerfToggleGpuOptionalGraph`: adds or removes Power or Clocks in
    /// `performance.gpu_optional_graphs`. The page follows through its settings observer.
    fn on_toggle_optional(
        &mut self,
        a: &PerfToggleGpuOptionalGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if a.key != self.key {
            return;
        }
        TlSettings::update(cx, |d| {
            let list = &mut d.settings.performance.gpu_optional_graphs;
            match list.iter().position(|g| *g == a.which) {
                Some(i) => {
                    list.remove(i);
                }
                None => list.push(a.which),
            }
        });
        self.sync_settings(cx);
    }

    fn rebuild_memory(&mut self, labels_too: bool, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let shape = self.memory_shape;
        let spec = with_settings(
            memory_spec(self.row.idx, shape.shared, shape.total, &theme),
            &self.settings,
        );
        self.memory.update(cx, |v, cx| v.set_spec(spec, cx));
        if labels_too {
            let labels = memory_labels(shape.shared, &self.words);
            self.memory.update(cx, |v, cx| v.set_labels(labels, cx));
        }
    }

    /// Rebuilds every chart spec from the current settings and theme.
    fn rebuild_all(&mut self, cx: &mut Context<Self>) {
        self.rebuild_grid(cx);
        self.rebuild_memory(false, cx);
        let theme = current(cx).clone();
        let spec = with_settings(temperature_spec(self.row.idx, &theme), &self.settings);
        self.temperature.update(cx, |v, cx| v.set_spec(spec, cx));
        if let Some(p) = &self.placeholder {
            let spec = with_settings(placeholder_spec(self.row.idx, &theme), &self.settings);
            p.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_optional(&snap, true, cx);
    }

    /// Applies the page's settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        self.rebuild_all(cx);
        cx.notify();
    }

    /// A new theme or UI scale: every chart takes the new colours and the grid its new sizes.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_all(cx);
        cx.notify();
    }

    /// Row 1: the engine grid in a box as tall as its rows for the measured width, or the
    /// placeholder cell while the probe is missing. The box ends with the gap to row 2 and has
    /// height 0 when every engine is hidden.
    fn engines_element(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        let spec = &self.engines.read(cx).spec;
        let (min_w, cell_h, gap) = (spec.cell_min_w, spec.cell_h, spec.gap);
        let n = match self.probe {
            ProbeState::Missing => 1,
            _ => spec.cells.len(),
        };
        let width = self.width.get();
        let measured = if width > px(0.) {
            width
        } else {
            px(UNMEASURED_W)
        };
        let layout = grid_layout(n, measured, min_w, gap);
        let height = grid_height(&layout, cell_h, gap);

        let measure = {
            let cell = Rc::clone(&self.width);
            let origin = Rc::clone(&self.origin);
            canvas(
                move |bounds, window, cx| {
                    origin.set(bounds.origin);
                    let old = cell.replace(bounds.size.width);
                    let at = |w: Pixels| {
                        let w = if w > px(0.) { w } else { px(UNMEASURED_W) };
                        grid_layout(n, w, min_w, gap)
                    };
                    if at(old) != at(bounds.size.width) {
                        cx.notify(window.current_view());
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        // The cell under a right-press, named for the page menu's "Hide this engine graph".
        let hit = {
            let keys = self.engine_keys.clone();
            let origin = Rc::clone(&self.origin);
            move |ev: &MouseDownEvent, cx: &mut App| {
                if ev.button != MouseButton::Right {
                    return;
                }
                let p = ev.position - origin.get();
                let (x, y) = (f32::from(p.x), f32::from(p.y));
                let (w, h, g) = (f32::from(layout.cell_w), f32::from(cell_h), f32::from(gap));
                let (col, row) = (x / (w + g), y / (h + g));
                let inside = x >= 0.
                    && y >= 0.
                    && x - col.floor() * (w + g) < w
                    && y - row.floor() * (h + g) < h;
                let cell = (row.floor() as usize) * layout.cols + col.floor() as usize;
                if inside
                    && (col.floor() as usize) < layout.cols
                    && let Some(key) = keys.get(cell)
                {
                    cx.set_global(ClickedEngine(Some(key.clone())));
                }
            }
        };

        let root = div()
            .debug_selector(|| ENGINES_ID.into())
            .relative()
            .w_full()
            .h(height)
            .capture_any_mouse_down(move |ev, _, cx| hit(ev, cx));
        let root = match (&self.placeholder, self.probe) {
            (Some(p), ProbeState::Missing) => {
                let cell = div()
                    .w(layout.cell_w)
                    .h(cell_h)
                    .child(p.clone().cached(StyleRefinement::default().size_full()));
                let cell = self.with_probe_tip(cell, "perf-gpu-tip-engines", window, cx);
                root.child(cell)
            }
            _ => {
                let mut grid = div().relative().size_full().child(
                    self.engines
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                );
                let view = self.engines.read(cx);
                if let Some((i, text)) = view.hovered_cell().zip(view.hovered_tooltip().cloned()) {
                    let m = *metrics(cx);
                    let pad = m.sp[3];
                    let (col, row) = (i % layout.cols, i / layout.cols);
                    let trigger = div()
                        .debug_selector(|| ENGINE_TIP_ID.into())
                        .absolute()
                        .left((layout.cell_w + gap) * col as f32 + pad)
                        .top((cell_h + gap) * row as f32 + pad)
                        .w((layout.cell_w - pad * 2.).max(px(0.)))
                        .h(px(LABEL_ROW * m.ui_scale));
                    // One state per heading, so moving to another heading restarts the delay.
                    let state = HoverTooltip::state((ENGINE_TIP_ID, i), window, cx);
                    grid = grid.child(HoverTooltip::attach(
                        &state,
                        trigger,
                        DELAY_CONTROL,
                        TlTooltip::text(text).build(),
                        window,
                        cx,
                    ));
                }
                root.child(self.with_probe_tip(grid, "perf-gpu-tip-engines", window, cx))
            }
        };
        let root = root.child(measure);

        // The gap to row 2 belongs to the box, so rows 2 and 3 move up when it collapses.
        let outer = div().w_full().flex_none().overflow_hidden();
        match self.row_anim.filter(|_| self.probe != ProbeState::Missing) {
            Some(RowAnim {
                showing,
                cells,
                generation,
            }) => {
                let full =
                    grid_height(&grid_layout(cells, measured, min_w, gap), cell_h, gap) + gap;
                outer
                    .child(root.h(full - gap).mb(gap))
                    .with_animation(
                        ("perf-gpu-row-anim", generation as usize),
                        Animation::new(M_BASE).with_easing(ease_out_cubic),
                        move |el, t| el.h(full * if showing { t } else { 1. - t }),
                    )
                    .into_any_element()
            }
            None if n == 0 => outer.h(px(0.)).child(root).into_any_element(),
            None => outer.pb(gap).child(root).into_any_element(),
        }
    }

    /// A chart box of `(W − sp4) / 2 × GRAPH_H_STACKED`. With `tip` (the trigger id and text), a
    /// trigger over the heading row shows the text, unless the probe tooltip covers the box.
    fn chart_box(
        &self,
        view: &Entity<ChartView>,
        selector: &'static str,
        tip: Option<(&'static str, &SharedString)>,
        m: &UiMetrics,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let mut chart = div()
            .debug_selector(move || selector.into())
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(GRAPH_H_STACKED * m.ui_scale))
            .child(view.clone().cached(StyleRefinement::default().size_full()));
        if let Some((id, text)) = tip.filter(|_| self.probe_tooltip().is_none()) {
            let trigger = div()
                .debug_selector(move || id.into())
                .absolute()
                .top_0()
                .left_0()
                .w_1_2()
                .h(px(LABEL_ROW * m.ui_scale) + m.sp[3]);
            let state = HoverTooltip::state(id, window, cx);
            chart = chart.child(HoverTooltip::attach(
                &state,
                trigger,
                DELAY_CONTROL,
                TlTooltip::text(text.clone()).build(),
                window,
                cx,
            ));
        }
        self.with_probe_tip(chart, selector, window, cx)
    }

    /// Hangs the probe tooltip on `el` while the probe is not Ok.
    fn with_probe_tip(
        &self,
        el: Div,
        id: &'static str,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(text) = self.probe_tooltip() else {
            return el.into_any_element();
        };
        let id = ElementId::NamedChild(
            Arc::new(ElementId::Name(SharedString::from(self.key.0.clone()))),
            id.into(),
        );
        let state = HoverTooltip::state(id, window, cx);
        HoverTooltip::attach(
            &state,
            el,
            DELAY_CONTROL,
            TlTooltip::text(text.clone()).build(),
            window,
            cx,
        )
        .into_any_element()
    }
}

// ---- Details grid (UI-PRF-033) ----

pub const ROW_UTILIZATION: &str = "perf-gpu-row-utilization";
pub const ROW_DEDICATED: &str = "perf-gpu-row-dedicated";
pub const ROW_SHARED: &str = "perf-gpu-row-shared";
pub const ROW_RESERVED: &str = "perf-gpu-row-reserved";
pub const ROW_ENCODE: &str = "perf-gpu-row-encode";
pub const ROW_DECODE: &str = "perf-gpu-row-decode";
/// The one row that stands in for rows 5 and 6 when the hardware shares one engine.
pub const ROW_ENCODE_DECODE: &str = "perf-gpu-row-encode-decode";
pub const ROW_TEMPERATURE: &str = "perf-gpu-row-temperature";
pub const ROW_TEMP_SOURCE: &str = "perf-gpu-row-temp-source";

/// The 28 Details rows in grid order (UI-PRF-033). Rows 9 to 28 are P2 (E25-024).
pub const GPU_DETAIL_ROWS: [&str; 28] = [
    ROW_UTILIZATION,
    ROW_DEDICATED,
    ROW_SHARED,
    ROW_RESERVED,
    ROW_ENCODE,
    ROW_DECODE,
    ROW_TEMPERATURE,
    ROW_TEMP_SOURCE,
    "perf-gpu-row-fan",
    "perf-gpu-row-slowdown",
    "perf-gpu-row-shutdown",
    "perf-gpu-row-mem-temp",
    "perf-gpu-row-clock",
    "perf-gpu-row-mem-clock",
    "perf-gpu-row-video-clock",
    "perf-gpu-row-power",
    "perf-gpu-row-pcie",
    "perf-gpu-row-pcie-max",
    "perf-gpu-row-pcie-throughput",
    "perf-gpu-row-driver",
    "perf-gpu-row-module",
    "perf-gpu-row-cuda",
    "perf-gpu-row-vulkan",
    "perf-gpu-row-opengl",
    "perf-gpu-row-location",
    "perf-gpu-row-vbios",
    "perf-gpu-row-uuid",
    "perf-gpu-row-arch",
];

/// The rows 1 to 8 fill.
const P1_ROWS: usize = 8;
/// `{$n}`: the active NVENC sessions.
const TIP_SESSIONS: &str = "perf-gpu-enc-sessions";
/// Separates the sensors in the Temperature tooltip.
const DOT: &str = " · ";

/// The GPU Details model: the 28 rows in order plus `Video encode/decode` after
/// `Video decode`. The merged row is hidden until a record reports a shared engine.
pub fn gpu_details_model() -> KeyValueModel {
    let mut labels = GPU_DETAIL_ROWS.to_vec();
    labels.insert(6, ROW_ENCODE_DECODE);
    let mut model = KeyValueModel::new(&labels)
        .with_counted_text(TIP_SESSIONS, &["n"], 0)
        .with_texts(&[
            (TEXT_FAN, &["pct", "rpm"]),
            (TEXT_PCIE, &["gen", "width"]),
            (TEXT_THROUGHPUT, &["rx", "tx"]),
            (TEXT_LOCATION, &["bus", "dev", "fn"]),
            (TEXT_POWER_TIP, &["max", "kwh"]),
        ]);
    model.hide(ROW_ENCODE_DECODE);
    model
}

const TEXT_FAN: &str = "perf-gpu-fan-value";
const TEXT_PCIE: &str = "perf-gpu-pcie-value";
const TEXT_THROUGHPUT: &str = "perf-gpu-pcie-throughput-value";
const TEXT_LOCATION: &str = "perf-gpu-location-value";
const TEXT_POWER_TIP: &str = "perf-gpu-power-row-tooltip";

/// What the Details rows are read from.
#[derive(Clone, Copy)]
pub struct GpuDetailInputs<'a> {
    pub now_boot_ns: u64,
    /// `observed_end` of the system component, for the Stale age.
    pub observed_end_ns: u64,
    pub totals: Option<&'a GpuTotals>,
    pub adapter: Option<&'a GraphicsAdapter>,
    pub vendor: GpuVendor,
    pub probe: ProbeState,
    /// The NVML UUID from the device catalog; all zero for AMD and Intel.
    pub uuid: Option<[u8; 16]>,
    /// The PCI bus id from the device catalog, for example "0000:2d:00.0".
    pub pci_bus_id: Option<&'a str>,
}

/// A metric of a GPU with no record yet.
fn missing<T: Copy + Default>() -> Metric<T> {
    Metric::with(Validity::Missing)
}

/// The value of a `Metric` or an inventory field when it is shown.
fn shown_u64(value: u64, validity: Validity) -> Option<u64> {
    matches!(validity, Validity::Valid | Validity::Stale).then_some(value)
}

/// Fills rows 1 to 8 (UI-PRF-033) from the GPU record and the inventory adapter. A shared
/// encode and decode engine shows one `Video encode/decode` row in place of rows 5 and 6. In a
/// probe state every row reads "Unavailable" (restarting) or "Not supported" (missing). True
/// when the grid needs a redraw.
pub fn update_gpu_details_p1(model: &mut KeyValueModel, inp: &GpuDetailInputs) -> bool {
    let probe_word = match inp.probe {
        ProbeState::Ok => None,
        ProbeState::Restarting => Some(Fallback::Unavailable),
        ProbeState::Missing => Some(Fallback::NotSupported),
    };
    if let Some(f) = probe_word {
        let mut changed = false;
        for label in GPU_DETAIL_ROWS[..P1_ROWS]
            .iter()
            .chain([&ROW_ENCODE_DECODE])
        {
            changed |= model.set_fallback(label, Validity::Unsupported, f, None);
        }
        return changed;
    }
    let age_ns = inp.now_boot_ns.saturating_sub(inp.observed_end_ns);
    let rows = DetailRows { age_ns };
    let mut changed = false;
    let t = inp.totals;

    let pct = units::percent;
    let pct_bits = |v: f32| u64::from(v.to_bits());
    changed |= rows.metric(
        model,
        ROW_UTILIZATION,
        t.map_or(missing(), |t| t.util_pct),
        pct_bits,
        pct,
    );

    // Dedicated: used of the adapter's size, else of the NVML total.
    let dedicated_total = inp
        .adapter
        .and_then(|a| shown_u64(a.dedicated_bytes.value, a.dedicated_bytes.validity))
        .or_else(|| t.and_then(|t| shown_u64(t.mem_total.value, t.mem_total.validity)));
    changed |= rows.pair(
        model,
        ROW_DEDICATED,
        t.map_or(missing(), |t| t.mem_used),
        dedicated_total,
    );

    let shared_total = inp
        .adapter
        .and_then(|a| shown_u64(a.shared_bytes.value, a.shared_bytes.validity))
        .or_else(|| t.and_then(|t| shown_u64(t.shared_total.value, t.shared_total.validity)))
        .filter(|&n| n > 0);
    changed |= match (inp.vendor, shared_total) {
        // NVIDIA has no shared memory pool.
        (GpuVendor::Nvidia, _) | (_, None) => model.set_fallback(
            ROW_SHARED,
            Validity::Unsupported,
            Fallback::NotSupported,
            None,
        ),
        (_, Some(total)) => rows.pair(
            model,
            ROW_SHARED,
            t.map_or(missing(), |t| t.shared_used),
            Some(total),
        ),
    };

    changed |= rows.metric(
        model,
        ROW_RESERVED,
        t.map_or(missing(), |t| t.mem_reserved),
        |v| v,
        units::bytes,
    );

    // A shared engine: one row with the encode value, which equals the decode value.
    let shared = t.is_some_and(|t| t.enc_dec_shared);
    changed |= show_if(model, ROW_ENCODE, !shared)
        | show_if(model, ROW_DECODE, !shared)
        | show_if(model, ROW_ENCODE_DECODE, shared);
    let enc = t.map_or(missing(), |t| t.enc_pct);
    if shared {
        changed |= rows.metric(model, ROW_ENCODE_DECODE, enc, pct_bits, pct);
    } else {
        changed |= rows.metric(model, ROW_ENCODE, enc, pct_bits, pct);
        // NVENC sessions in the tooltip. A Stale value keeps its age tooltip.
        if enc.validity == Validity::Valid {
            let sessions = t.map_or(missing(), |t| t.enc_sessions);
            let tip = model.text(TIP_SESSIONS);
            changed |= if matches!(sessions.validity, Validity::Valid | Validity::Stale) {
                set_tip(model, ROW_ENCODE, |o| {
                    let _ = tip.write_count(o, u64::from(sessions.value), &[&sessions.value]);
                })
            } else {
                clear_tip(model, ROW_ENCODE)
            };
        }
        changed |= rows.metric(
            model,
            ROW_DECODE,
            t.map_or(missing(), |t| t.dec_pct),
            pct_bits,
            pct,
        );
    }

    let temp = t.map_or(missing(), |t| t.temp_c);
    changed |= rows.metric(model, ROW_TEMPERATURE, temp, pct_bits, units::celsius);
    // Every amdgpu sensor in the tooltip: "edge 48 °C · junction 52 °C · mem 60 °C".
    if temp.validity == Validity::Valid {
        let sensors = t.map_or([missing(); 3], |t| {
            [t.temp_edge_c, t.temp_junction_c, t.temp_mem_c]
        });
        let shown = || {
            ["edge", "junction", "mem"]
                .into_iter()
                .zip(sensors)
                .filter(|(_, m)| matches!(m.validity, Validity::Valid | Validity::Stale))
        };
        changed |= if shown().next().is_none() {
            clear_tip(model, ROW_TEMPERATURE)
        } else {
            set_tip(model, ROW_TEMPERATURE, |o| {
                for (n, (name, m)) in shown().enumerate() {
                    if n > 0 {
                        o.push_str(DOT);
                    }
                    o.push_str(name);
                    o.push(' ');
                    units::celsius(o, m.value);
                }
            })
        };
    }

    changed |= match inp.adapter.map(|a| &a.temperature_source) {
        Some(src)
            if matches!(src.validity, Validity::Valid | Validity::Stale)
                && !src.value.trim().is_empty() =>
        {
            model.set_text(ROW_TEMP_SOURCE, src.value.trim())
        }
        _ => model.set_fallback(
            ROW_TEMP_SOURCE,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        ),
    };
    changed
}

const ROW_FAN: &str = "perf-gpu-row-fan";
const ROW_SLOWDOWN: &str = "perf-gpu-row-slowdown";
const ROW_SHUTDOWN: &str = "perf-gpu-row-shutdown";
const ROW_MEM_TEMP: &str = "perf-gpu-row-mem-temp";
const ROW_CLOCK: &str = "perf-gpu-row-clock";
const ROW_MEM_CLOCK: &str = "perf-gpu-row-mem-clock";
const ROW_VIDEO_CLOCK: &str = "perf-gpu-row-video-clock";
const ROW_POWER: &str = "perf-gpu-row-power";
const ROW_PCIE: &str = "perf-gpu-row-pcie";
const ROW_PCIE_MAX: &str = "perf-gpu-row-pcie-max";
const ROW_PCIE_THROUGHPUT: &str = "perf-gpu-row-pcie-throughput";
const ROW_DRIVER: &str = "perf-gpu-row-driver";
const ROW_MODULE: &str = "perf-gpu-row-module";
const ROW_CUDA: &str = "perf-gpu-row-cuda";
const ROW_VULKAN: &str = "perf-gpu-row-vulkan";
const ROW_OPENGL: &str = "perf-gpu-row-opengl";
const ROW_LOCATION: &str = "perf-gpu-row-location";
const ROW_VBIOS: &str = "perf-gpu-row-vbios";
const ROW_UUID: &str = "perf-gpu-row-uuid";
const ROW_ARCH: &str = "perf-gpu-row-arch";

/// The rows 9 to 28 that come from the GPU record or the probe's static info. They read
/// "Unavailable" or "Not supported" in a probe state. The inventory rows (driver, module, Vulkan,
/// OpenGL, location, UUID) do not.
const PROBE_ROWS: [&str; 14] = [
    ROW_FAN,
    ROW_SLOWDOWN,
    ROW_SHUTDOWN,
    ROW_MEM_TEMP,
    ROW_CLOCK,
    ROW_MEM_CLOCK,
    ROW_VIDEO_CLOCK,
    ROW_POWER,
    ROW_PCIE,
    ROW_PCIE_MAX,
    ROW_PCIE_THROUGHPUT,
    ROW_CUDA,
    ROW_VBIOS,
    ROW_ARCH,
];

/// The value of a `Metric` when it is shown.
fn shown<T: Copy + Default>(m: Metric<T>) -> Option<T> {
    matches!(m.validity, Validity::Valid | Validity::Stale).then_some(m.value)
}

/// Two metrics as one value: shown when both are, Stale when either is, else the first one's
/// state that is not shown.
fn both<A: Copy + Default, B: Copy + Default>(a: Metric<A>, b: Metric<B>) -> Metric<(A, B)> {
    let validity = match (shown(a).is_some(), shown(b).is_some()) {
        (true, true) if a.validity == Validity::Stale || b.validity == Validity::Stale => {
            Validity::Stale
        }
        (true, true) => Validity::Valid,
        (false, _) => a.validity,
        (true, false) => b.validity,
    };
    Metric {
        value: (a.value, b.value),
        validity,
    }
}

/// A row from a text the inventory or the probe reports once. A blank or missing text reads
/// "Not reported"; `failed_unknown` makes a failed read "Unknown".
fn static_text(
    model: &mut KeyValueModel,
    label: &'static str,
    text: Option<&str>,
    validity: Validity,
    failed_unknown: bool,
) -> bool {
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    match (text, validity) {
        (Some(t), Validity::Valid | Validity::Stale) => model.set_text(label, t),
        (_, Validity::Failed) if failed_unknown => {
            model.set_fallback(label, Validity::Unsupported, Fallback::Unknown, None)
        }
        (_, Validity::WarmingUp) => {
            model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None)
        }
        _ => model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None),
    }
}

/// Bus, device and function of a PCI id such as "0000:2d:00.0", in decimal.
fn parse_pci(id: &str) -> Option<[u32; 3]> {
    let mut parts = id.trim().rsplit(':');
    let (dev, func) = parts.next()?.split_once('.')?;
    let bus = parts.next()?;
    let hex = |s: &str| u32::from_str_radix(s, 16).ok();
    Some([hex(bus)?, hex(dev)?, hex(func)?])
}

/// The NVML text form of a UUID: `GPU-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
fn write_uuid(out: &mut String, uuid: &[u8; 16]) {
    out.push_str("GPU");
    for (i, byte) in uuid.iter().enumerate() {
        if matches!(i, 0 | 4 | 6 | 8 | 10) {
            out.push('-');
        }
        let _ = write!(out, "{byte:02x}");
    }
}

/// An Intel GPU with no dedicated memory shares the CPU package's power meter.
fn is_intel_igpu(inp: &GpuDetailInputs) -> bool {
    let dedicated = inp
        .adapter
        .and_then(|a| shown_u64(a.dedicated_bytes.value, a.dedicated_bytes.validity))
        .or_else(|| {
            inp.totals
                .and_then(|t| shown_u64(t.mem_total.value, t.mem_total.validity))
        });
    inp.vendor == GpuVendor::Intel && dedicated.is_none_or(|n| n == 0)
}

/// Fills rows 9 to 28 (UI-PRF-033). `Max PCI Express` is hidden when it equals the current link,
/// and `CUDA version` off NVIDIA. An Intel iGPU whose power is Restricted or Unsupported reads
/// "Needs setup" (a link). In a probe state the rows that come from the probe read "Unavailable"
/// or "Not supported". True when the grid needs a redraw.
pub fn update_gpu_details_p2(model: &mut KeyValueModel, inp: &GpuDetailInputs) -> bool {
    let mut changed = show_if(model, ROW_CUDA, inp.vendor == GpuVendor::Nvidia);
    let probe_word = match inp.probe {
        ProbeState::Ok => None,
        ProbeState::Restarting => Some(Fallback::Unavailable),
        ProbeState::Missing => Some(Fallback::NotSupported),
    };
    if let Some(f) = probe_word {
        changed |= show_if(model, ROW_PCIE_MAX, true);
        for label in PROBE_ROWS {
            changed |= model.set_fallback(label, Validity::Unsupported, f, None);
        }
    } else {
        changed |= update_gpu_live_rows(model, inp);
    }

    let a = inp.adapter;
    changed |= static_text(
        model,
        ROW_DRIVER,
        a.map(|a| &*a.driver_version.value),
        a.map_or(Validity::Missing, |a| a.driver_version.validity),
        false,
    );
    changed |= static_text(
        model,
        ROW_MODULE,
        a.map(|a| &*a.kernel_module.value),
        a.map_or(Validity::Missing, |a| a.kernel_module.validity),
        false,
    );
    changed |= static_text(
        model,
        ROW_VULKAN,
        a.map(|a| &*a.vulkan_version.value),
        a.map_or(Validity::Missing, |a| a.vulkan_version.validity),
        true,
    );
    changed |= static_text(
        model,
        ROW_OPENGL,
        a.map(|a| &*a.opengl_version.value),
        a.map_or(Validity::Missing, |a| a.opengl_version.validity),
        true,
    );

    let slot = inp
        .pci_bus_id
        .or_else(|| a.and_then(|a| a.pci_slot.as_deref()));
    changed |= match slot.and_then(parse_pci) {
        Some(ids) => {
            let bits = ids.iter().fold(0u64, |acc, &n| acc << 16 | u64::from(n));
            let tmpl = model.text(TEXT_LOCATION);
            model.set_valid(ROW_LOCATION, bits, |o| {
                tmpl.write_each(o, |i, o| {
                    let _ = write!(o, "{}", ids[i]);
                })
            })
        }
        None => model.set_fallback(
            ROW_LOCATION,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        ),
    };

    match inp.uuid.filter(|u| *u != [0; 16]) {
        Some(uuid) => {
            let bits = uuid.chunks(8).fold(0u64, |acc, c| {
                acc ^ c.iter().fold(0u64, |n, &b| n << 8 | u64::from(b))
            });
            changed |= model.set_valid(ROW_UUID, bits, |o| write_uuid(o, &uuid));
            // The cell cuts a long value with "…"; the tooltip holds the whole UUID.
            changed |= set_tip(model, ROW_UUID, |o| write_uuid(o, &uuid));
        }
        None => {
            changed |=
                model.set_fallback(ROW_UUID, Validity::Unsupported, Fallback::NotReported, None);
        }
    }
    changed
}

/// The rows of 9 to 28 that read the GPU record and the probe's static info, while the probe is
/// Ok.
fn update_gpu_live_rows(model: &mut KeyValueModel, inp: &GpuDetailInputs) -> bool {
    let age_ns = inp.now_boot_ns.saturating_sub(inp.observed_end_ns);
    let rows = DetailRows { age_ns };
    let t = inp.totals;
    let info = t.map(|t| &*t.info);
    let f32_bits = |v: f32| u64::from(v.to_bits());
    let mut changed = false;

    // Fan speed: "0% (0 RPM)", or one of the two when the card reports only that one.
    let (pct, rpm) = t.map_or((None, None), |t| (shown(t.fan_pct), shown(t.fan_rpm)));
    let fan_tmpl = model.text(TEXT_FAN);
    changed |= if pct.is_none() && rpm.is_none() {
        fallback(
            model,
            ROW_FAN,
            t.map_or(Validity::Missing, |t| t.fan_pct.validity),
        )
    } else {
        let stale = t.is_some_and(|t| {
            (pct.is_some() && t.fan_pct.validity == Validity::Stale)
                || (rpm.is_some() && t.fan_rpm.validity == Validity::Stale)
        });
        let validity = if stale {
            Validity::Stale
        } else {
            Validity::Valid
        };
        let value = (pct.unwrap_or_default(), rpm.unwrap_or_default());
        let bits = f32_bits(value.0)
            ^ u64::from(value.1) << 32
            ^ u64::from(pct.is_some()) << 63
            ^ u64::from(rpm.is_some()) << 62;
        rows.metric(
            model,
            ROW_FAN,
            Metric { value, validity },
            |_| bits,
            |o, (p, r)| match (pct, rpm) {
                (Some(_), Some(_)) => fan_tmpl.write_each(o, |i, o| {
                    let _ = if i == 0 {
                        write!(o, "{p:.0}")
                    } else {
                        write!(o, "{r}")
                    };
                }),
                (Some(_), None) => units::percent(o, p),
                _ => {
                    let _ = write!(o, "{r} RPM");
                }
            },
        )
    };

    let temp_c = |model: &mut KeyValueModel, label, v: Option<f32>| match v {
        Some(c) => model.set_valid(label, f32_bits(c), |o| units::celsius(o, c)),
        None => model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None),
    };
    changed |= temp_c(model, ROW_SLOWDOWN, info.and_then(|i| i.slowdown_temp_c));
    changed |= temp_c(model, ROW_SHUTDOWN, info.and_then(|i| i.shutdown_temp_c));
    let mem_temp = t.map_or(missing(), |t| t.temp_mem_c);
    changed |= if mem_temp.validity == Validity::Unsupported {
        model.set_fallback(
            ROW_MEM_TEMP,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        )
    } else {
        rows.metric(model, ROW_MEM_TEMP, mem_temp, f32_bits, units::celsius)
    };

    // Clock speed and Memory speed: current / maximum.
    for (label, cur, max) in [
        (
            ROW_CLOCK,
            t.map_or(missing(), |t| t.clock_gfx_mhz),
            info.and_then(|i| i.clock_gfx_max_mhz),
        ),
        (
            ROW_MEM_CLOCK,
            t.map_or(missing(), |t| t.clock_mem_mhz),
            info.and_then(|i| i.clock_mem_max_mhz),
        ),
    ] {
        changed |= rows.metric(
            model,
            label,
            cur,
            |v| u64::from(v) | u64::from(max.unwrap_or(0)) << 32,
            |o, v| match max {
                Some(m) => units::mhz_pair(o, v as f32, m as f32),
                None => units::mhz(o, v as f32),
            },
        );
    }
    // The P-state, "P3", in the Clock speed tooltip.
    if t.is_some_and(|t| t.clock_gfx_mhz.validity == Validity::Valid) {
        changed |= match t.and_then(|t| shown(t.pstate)) {
            Some(p) => set_tip(model, ROW_CLOCK, |o| {
                let _ = write!(o, "P{p}");
            }),
            None => clear_tip(model, ROW_CLOCK),
        };
    }
    changed |= rows.metric(
        model,
        ROW_VIDEO_CLOCK,
        t.map_or(missing(), |t| t.clock_video_mhz),
        u64::from,
        |o, v| units::mhz(o, v as f32),
    );

    // Power draw: "31.9 / 300 W". An Intel iGPU reads the CPU package meter, which needs setup.
    let power = t.map_or(missing(), |t| t.power_w);
    if is_intel_igpu(inp) && matches!(power.validity, Validity::Restricted | Validity::Unsupported)
    {
        changed |= model.set_fallback(ROW_POWER, Validity::Unsupported, Fallback::NeedsSetup, None);
    } else {
        let limit = t.and_then(|t| shown(t.power_limit_w));
        changed |= rows.metric(
            model,
            ROW_POWER,
            power,
            |v| f32_bits(v) ^ limit.map_or(0, f32_bits) << 32,
            |o, v| match limit {
                Some(l) => units::watts_pair(o, v, l),
                None => units::watts(o, v),
            },
        );
        if power.validity == Validity::Valid {
            // "max 350 W · 1.24 kWh since boot".
            let max = info.and_then(|i| i.power_max_limit_w);
            let joules = t.and_then(|t| shown(t.energy_since_boot));
            let tmpl = model.text(TEXT_POWER_TIP);
            changed |= match (max, joules) {
                (Some(max), Some(joules)) => set_tip(model, ROW_POWER, |o| {
                    tmpl.write_each(o, |i, o| {
                        let _ = if i == 0 {
                            write!(o, "{max:.0}")
                        } else {
                            write!(o, "{:.2}", joules / 3.6e6)
                        };
                    })
                }),
                _ => clear_tip(model, ROW_POWER),
            };
        }
    }

    // PCI Express: "Gen 4 x16". The maximum is hidden when the link runs at it.
    let link = both(
        t.map_or(missing::<u8>(), |t| t.pcie_gen),
        t.map_or(missing::<u8>(), |t| t.pcie_width),
    );
    let pcie_tmpl = model.text(TEXT_PCIE);
    let write_link = |o: &mut String, (g, w): (u8, u8)| {
        pcie_tmpl.write_each(o, |i, o| {
            let _ = write!(o, "{}", if i == 0 { g } else { w });
        })
    };
    changed |= rows.metric(
        model,
        ROW_PCIE,
        link,
        |(g, w)| u64::from(g) << 8 | u64::from(w),
        write_link,
    );
    let max = info.and_then(|i| Some((i.pcie_max_gen?, i.pcie_max_width?)));
    match max {
        Some(m) => {
            changed |= show_if(model, ROW_PCIE_MAX, shown(link) != Some(m));
            changed |= model.set_valid(ROW_PCIE_MAX, u64::from(m.0) << 8 | u64::from(m.1), |o| {
                write_link(o, m)
            });
        }
        None => {
            changed |= show_if(model, ROW_PCIE_MAX, true);
            changed |= model.set_fallback(
                ROW_PCIE_MAX,
                Validity::Unsupported,
                Fallback::NotReported,
                None,
            );
        }
    }
    let rate = both(
        t.map_or(missing::<f32>(), |t| t.pcie_rx_bps),
        t.map_or(missing::<f32>(), |t| t.pcie_tx_bps),
    );
    let rate_tmpl = model.text(TEXT_THROUGHPUT);
    changed |= rows.metric(
        model,
        ROW_PCIE_THROUGHPUT,
        rate,
        |(rx, tx)| f32_bits(rx) << 32 | f32_bits(tx),
        |o, (rx, tx)| {
            rate_tmpl.write_each(o, |i, o| {
                units::rate_iec_auto(o, f64::from(if i == 0 { rx } else { tx }))
            })
        },
    );

    // Texts the probe reports once. A card without one reads "Not reported".
    for (label, text) in [
        (ROW_CUDA, info.and_then(|i| i.cuda_version.as_deref())),
        (ROW_VBIOS, info.and_then(|i| i.vbios.as_deref())),
        (ROW_ARCH, info.and_then(|i| i.architecture.as_deref())),
    ] {
        changed |= static_text(model, label, text, Validity::Valid, false);
    }
    changed
}

/// Shows or hides the row. True when that changed.
fn show_if(model: &mut KeyValueModel, label: &'static str, show: bool) -> bool {
    let Some(row) = model.rows.iter().find(|r| r.label == label) else {
        return false;
    };
    if row.hidden != show {
        return false;
    }
    if show {
        model.show(label);
    } else {
        model.hide(label);
    }
    true
}

/// Sets the row's tooltip to what `write` produces, through a reused buffer, so an unchanged
/// tooltip costs no allocation. True when it changed.
fn set_tip(
    model: &mut KeyValueModel,
    label: &'static str,
    write: impl FnOnce(&mut String),
) -> bool {
    thread_local! {
        static TIP: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    TIP.with_borrow_mut(|text| {
        text.clear();
        write(text);
        let current = model.rows.iter().find(|r| r.label == label);
        if current.and_then(|r| r.tooltip.as_deref()) == Some(text.as_str()) {
            return false;
        }
        model.set_tooltip(label, Some(SharedString::from(text.as_str())));
        true
    })
}

fn clear_tip(model: &mut KeyValueModel, label: &'static str) -> bool {
    let current = model.rows.iter().find(|r| r.label == label);
    if current.is_none_or(|r| r.tooltip.is_none()) {
        return false;
    }
    model.set_tooltip(label, None);
    true
}

/// The shared parts of a row update.
struct DetailRows {
    age_ns: u64,
}

impl DetailRows {
    /// One row from a metric: Valid formats on a bits change, Stale keeps the value and marks
    /// it, the rest show a fallback word.
    fn metric<T: Copy + Default>(
        &self,
        model: &mut KeyValueModel,
        label: &'static str,
        m: Metric<T>,
        bits: impl Fn(T) -> u64,
        write: impl FnOnce(&mut String, T),
    ) -> bool {
        match m.validity {
            Validity::Valid => model.set_valid(label, bits(m.value), |o| write(o, m.value)),
            Validity::Stale => {
                model.set_valid(label, bits(m.value), |o| write(o, m.value))
                    | model.set_stale(label, self.age_ns)
            }
            v => fallback(model, label, v),
        }
    }

    /// "2.80 GiB / 15.92 GiB" from `used` and `total`; "Not reported" without a total.
    fn pair(
        &self,
        model: &mut KeyValueModel,
        label: &'static str,
        used: Metric<u64>,
        total: Option<u64>,
    ) -> bool {
        let Some(total) = total else {
            return model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None);
        };
        // Both halves in the bits, so either change formats again.
        let bits = |u: u64| u.wrapping_mul(31).wrapping_add(total);
        self.metric(model, label, used, bits, |o, u| {
            units::bytes_of(o, u, total)
        })
    }
}

fn fallback(model: &mut KeyValueModel, label: &'static str, v: Validity) -> bool {
    match v {
        Validity::Unsupported => model.set_fallback(label, v, Fallback::NotSupported, None),
        Validity::Missing => {
            model.set_fallback(label, Validity::Unsupported, Fallback::Unavailable, None)
        }
        _ => model.set_fallback(label, v, Fallback::Unavailable, None),
    }
}

/// The record of `device` in the snapshot.
pub(crate) fn gpu_totals(snap: &Snapshot, device: DeviceIdx) -> Option<&GpuTotals> {
    let gpus = &snap.system.gpus;
    gpus.binary_search_by_key(&device, |(i, _)| *i)
        .ok()
        .map(|i| &gpus[i].1)
}

/// The placeholder cell's chart: the 3D/Compute series, which the missing probe never fills.
fn placeholder_spec(device: DeviceIdx, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: series(device, MetricId::GPU_UTIL),
            fill: Some(theme.graph.gpu_fill),
            line: theme.graph.gpu_line,
            ..ChartSeries::default()
        }],
        background: ChartBg::Window,
        hover: true,
        ..ChartSpec::default()
    }
}

/// The engine grid spec: one cell per reported engine that is not hidden, and the engine keys
/// in the same order into `keys`.
#[allow(clippy::too_many_arguments)]
fn grid_spec(
    device: DeviceIdx,
    vendor: GpuVendor,
    reported: Reported,
    s: &PageSettings,
    words: &Words,
    theme: &TlTheme,
    m: &UiMetrics,
    keys: &mut Vec<Arc<str>>,
) -> ChartGridSpec {
    keys.clear();
    let mut cells = Vec::new();
    for c in vendor_cells(vendor)
        .iter()
        .filter(|c| reported.has(c.engine))
        .filter(|c| !c.engine.is_hidden(&s.hidden))
    {
        keys.push(Arc::from(c.engine.key()));
        // AMD reads these from fdinfo, so a denied read leaves other users' processes out
        // (PERF-074).
        let own_only = reported.partial && vendor == GpuVendor::Amd && fdinfo_engine(c.engine);
        cells.push(GridCellSpec {
            heading: words.engine(c.name),
            fill_series: series(device, c.engine.metric()),
            overlay_series: None,
            offline: false,
            mark: None,
            tooltip: own_only.then(|| words.engine("perf-gpu-tip-own-processes")),
        });
    }
    ChartGridSpec {
        cells,
        fill: theme.graph.gpu_fill,
        line: theme.graph.gpu_line,
        overlay_line: theme.graph.gpu_line,
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        range_ns: s.range_ns,
        smooth: s.smooth,
        cell_min_w: px(ENGINE_CELL_MIN_W * m.ui_scale),
        cell_h: px(ENGINE_CELL_H * m.ui_scale),
        gap: m.sp[4],
        legend: SmallVec::new(),
        value_fmt: ValueFormat::Percent,
        hover: true,
    }
}

impl DevicePage for GpuPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        let row2 = h_flex()
            .w_full()
            .gap(m.sp[4])
            .child(self.chart_box(&self.memory, MEMORY_ID, None, &m, window, cx))
            .child(self.chart_box(&self.temperature, TEMPERATURE_ID, None, &m, window, cx));
        let mut page = v_flex()
            .w_full()
            .child(self.engines_element(window, cx))
            .child(row2);
        // Row 3: one chart takes the full width, two split it; none leaves no row.
        if self.power.is_some() || self.clocks.is_some() {
            let mut row3 = h_flex().w_full().pt(m.sp[4]).gap(m.sp[4]);
            if let Some(v) = &self.power {
                let tip = self.power_tip.as_ref().map(|t| (POWER_TIP_ID, t));
                row3 = row3.child(self.chart_box(v, POWER_ID, tip, &m, window, cx));
            }
            if let Some(v) = &self.clocks {
                let tip = self.clocks_tip.as_ref().map(|t| (CLOCKS_TIP_ID, t));
                row3 = row3.child(self.chart_box(v, CLOCKS_ID, tip, &m, window, cx));
            }
            page = page.child(row3);
        }
        page.into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 3]> {
        let (m, theme) = (metrics(cx), current(cx));
        let mut heading = div()
            .text_color(theme.text_primary)
            .child(self.words.displays.clone());
        *heading.text_style() = label_style(m, m.text.md, FontWeight::MEDIUM);
        let mut block = v_flex().gap(m.sp[4]).child(heading);
        if self.display_rows.is_empty() {
            let mut el = div()
                .h(px(24.))
                .text_color(theme.text_secondary)
                .child(self.words.no_display.clone());
            *el.text_style() = label_style(m, m.text.sm, FontWeight::NORMAL);
            block = block.child(el);
        }
        for row in &self.display_rows {
            let mut el = div()
                .h(px(24.))
                .text_color(theme.text_primary)
                .child(row.clone());
            *el.text_style() = label_style(m, m.text.sm, FontWeight::NORMAL);
            block = block.child(el);
        }
        smallvec![block.into_any_element()]
    }

    /// "Select GPU engine graphs >": one check item per reported engine, then the optional Power
    /// and Clocks graphs (PERF-063, UI-PRF-032).
    fn menu_items(&self, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
        let key = self.key.clone();
        let items: Vec<(Arc<str>, SharedString, bool)> = vendor_cells(self.vendor)
            .iter()
            .filter(|c| self.reported.has(c.engine))
            .map(|c| {
                (
                    Arc::from(c.engine.key()),
                    self.words.engine(c.name),
                    !c.engine.is_hidden(&self.settings.hidden),
                )
            })
            .collect();
        let (power, clocks) = (
            tr(cx, "perf-gpu-menu-power"),
            tr(cx, "perf-gpu-menu-clocks"),
        );
        let (power_on, clocks_on) = (self.power.is_some(), self.clocks.is_some());
        menu.submenu(
            tr(cx, "perf-gpu-menu-select-engines"),
            window,
            cx,
            move |mut menu, _, _| {
                for (engine, name, shown) in &items {
                    menu = menu.check_item(
                        name.clone(),
                        *shown,
                        Box::new(PerfToggleGpuEngine {
                            key: key.clone(),
                            engine: engine.clone(),
                        }),
                    );
                }
                let optional = |which| {
                    Box::new(PerfToggleGpuOptionalGraph {
                        key: key.clone(),
                        which,
                    })
                };
                menu.separator()
                    .check_item(power.clone(), power_on, optional(GpuOptionalGraph::Power))
                    .check_item(
                        clocks.clone(),
                        clocks_on,
                        optional(GpuOptionalGraph::Clocks),
                    )
            },
        )
    }

    /// The GPU copy text (PERF-011, UI-PRF-022): the header lines, the Details rows, then the
    /// Displays block with one indented line per display.
    fn copy_text(&self, out: &mut String) {
        let (word, model) = &self.header;
        out.push_str(word);
        out.push('\n');
        if !model.is_empty() {
            out.push_str(model);
            out.push('\n');
        }
        let rows_from = out.len();
        self.details.copy_into(out);
        if out.len() > rows_from {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&self.words.displays);
        out.push('\n');
        if self.display_rows.is_empty() {
            out.push_str("    ");
            out.push_str(&self.words.no_display);
            out.push('\n');
        }
        for row in &self.display_rows {
            out.push_str("    ");
            out.push_str(row);
            out.push('\n');
        }
    }

    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        SmallVec::new()
    }

    fn set_active(&mut self, active: bool, cx: &mut App) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
        } else {
            let id = self.id;
            self.demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }
}

impl Focusable for GpuPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GpuPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        self.details.refresh_strings(cx);
        if self.words.refresh(cx) {
            let labels = memory_labels(self.memory_shape.shared, &self.words);
            self.memory.update(cx, |v, cx| v.set_labels(labels, cx));
            let labels = temperature_labels(&self.words);
            self.temperature
                .update(cx, |v, cx| v.set_labels(labels, cx));
            if let Some(v) = &self.power {
                let labels = power_labels(&self.words);
                v.update(cx, |v, cx| v.set_labels(labels, cx));
            }
            if let Some(v) = &self.clocks {
                let labels = clocks_labels(&self.words);
                v.update(cx, |v, cx| v.set_labels(labels, cx));
            }
            if let Some(p) = &self.placeholder {
                let labels = placeholder_labels(&self.words);
                p.update(cx, |v, cx| v.set_labels(labels, cx));
            }
            self.rebuild_grid(cx);
            self.apply_probe(cx);
            let snap = Arc::clone(self.hub.read(cx).current());
            self.apply_snapshot(&snap, ComponentMask::INVENTORY, cx);
        }
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_toggle_engine))
            .on_action(cx.listener(Self::on_toggle_optional))
            .child(
                DevicePageFrame {
                    page: self,
                    m: &m,
                    theme: &theme,
                }
                .render(window, cx),
            )
    }
}

/// The translator the Displays row formats with: a key and its Fluent arguments.
type Translate<'a> = &'a dyn Fn(&str, &[(&str, FluentValue<'static>)]) -> SharedString;

/// The Displays row of `d` in the en-US text (PERF-073):
/// `<name> · <connector> · 1920x1200 @ 59.95 Hz · Max bit depth 10 · Portrait (left) · 110%`.
pub fn display_row_text(d: &DisplayMode) -> SharedString {
    display_row_with(d, &tr_en)
}

fn display_row_with(d: &DisplayMode, t: Translate<'_>) -> SharedString {
    let text = |s: String| FluentValue::from(s);
    let not_reported = || t("perf-fallback-not-reported", &[]);
    let connector = d.connector.name();
    let mut name = match d.monitor_name.as_deref().map(str::trim) {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => connector.to_owned(),
    };
    if d.primary {
        name.push(' ');
        name.push_str(&t("perf-gpu-display-primary", &[]));
    }
    let (w, h) = (text(d.width.to_string()), text(d.height.to_string()));
    let mode = if d.refresh_mhz == 0 {
        t("perf-gpu-display-mode-no-hz", &[("w", w), ("h", h)])
    } else {
        let hz = format!("{:.2}", f64::from(d.refresh_mhz) / 1000.0);
        t(
            "perf-gpu-display-mode",
            &[("w", w), ("h", h), ("hz", text(hz))],
        )
    };
    let bpc = d
        .max_bpc
        .map_or_else(not_reported, |b| SharedString::from(b.to_string()));
    let orientation = match d.rotation {
        Some(1) => t("perf-gpu-orientation-landscape", &[]),
        Some(2) => t("perf-gpu-orientation-portrait-left", &[]),
        Some(4) => t("perf-gpu-orientation-landscape-flipped", &[]),
        Some(8) => t("perf-gpu-orientation-portrait-right", &[]),
        _ => not_reported(),
    };
    let scale = d.scale.map_or_else(not_reported, |f| {
        let pct = (f64::from(f) * 100.0).round() as i64;
        t("perf-gpu-display-scale", &[("pct", text(pct.to_string()))])
    });
    t(
        "perf-gpu-display-row",
        &[
            ("name", text(name)),
            ("connector", text(connector.to_owned())),
            ("mode", text(mode.to_string())),
            ("bpc", text(bpc.to_string())),
            ("orientation", text(orientation.to_string())),
            ("scale", text(scale.to_string())),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dp_display() -> DisplayMode {
        DisplayMode {
            width: 1920,
            height: 1200,
            refresh_mhz: 59_950,
            primary: true,
            monitor_name: Some(Arc::from("DELL U2415")),
            connector: tl_model::snapshot::DisplayConnector::DisplayPort,
            max_bpc: Some(10),
            rotation: Some(2),
            scale: Some(1.1),
        }
    }

    #[test]
    fn display_row_format() {
        assert_eq!(
            display_row_text(&dp_display()).as_ref(),
            "DELL U2415 (primary) · DisplayPort · 1920x1200 @ 59.95 Hz · Max bit depth 10 · Portrait (left) · 110%"
        );
    }

    #[test]
    fn display_row_missing_fields() {
        let d = DisplayMode {
            max_bpc: None,
            rotation: None,
            scale: Some(1.0),
            primary: false,
            ..dp_display()
        };
        assert_eq!(
            display_row_text(&d).as_ref(),
            "DELL U2415 · DisplayPort · 1920x1200 @ 59.95 Hz · Max bit depth Not reported · Not reported · 100%"
        );
        let d = DisplayMode {
            refresh_mhz: 0,
            scale: None,
            monitor_name: None,
            ..d
        };
        assert_eq!(
            display_row_text(&d).as_ref(),
            "DisplayPort · DisplayPort · 1920x1200 · Max bit depth Not reported · Not reported · Not reported"
        );
    }

    fn keys(list: &[&str]) -> Vec<Arc<str>> {
        list.iter().map(|k| Arc::from(*k)).collect()
    }

    #[test]
    fn gpu_engine_names_nvidia() {
        let cells = engine_cells(GpuVendor::Nvidia, &keys(&["dec", "gpu", "enc"]), &[]);
        let names: Vec<&str> = cells.iter().map(|(_, n)| n.as_ref()).collect();
        assert_eq!(names, ["3D / Compute", "Video Encode", "Video Decode"]);
        let order: Vec<&str> = cells.iter().map(|(k, _)| &**k).collect();
        assert_eq!(order, ["gpu", "enc", "dec"]);

        let hidden = engine_cells(GpuVendor::Nvidia, &keys(&["gpu", "enc"]), &keys(&["enc"]));
        assert_eq!(hidden.len(), 1);
        assert_eq!(&*hidden[0].0, "gpu");
    }

    #[test]
    fn engine_columns_rule() {
        assert_eq!(engine_columns(6, 1000.0, 160.0), 4);
        assert_eq!(engine_columns(6, 500.0, 160.0), 3);
        assert_eq!(engine_columns(3, 1000.0, 160.0), 3);
        assert_eq!(engine_columns(1, 100.0, 160.0), 1);
        assert_eq!(engine_columns(0, 1000.0, 160.0), 1);
    }

    #[test]
    fn reported_bits_skip_unsupported() {
        use tl_model::system::Metric;
        let mut t = GpuTotals {
            device: DeviceIdx(1),
            util_pct: Metric::valid(10.0),
            mem_util_pct: Metric::default(),
            mem_used: Metric::default(),
            mem_total: Metric::default(),
            shared_used: Metric::default(),
            shared_total: Metric::default(),
            enc_pct: Metric::with(Validity::Unsupported),
            dec_pct: Metric::valid(0.0),
            mem_reserved: Metric::default(),
            enc_sessions: Metric::default(),
            enc_dec_shared: false,
            temp_c: Metric::default(),
            temp_edge_c: Metric::default(),
            temp_junction_c: Metric::default(),
            temp_mem_c: Metric::default(),
            power_w: Metric::default(),
            power_limit_w: Metric::default(),
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
        };
        assert_eq!(reported_bits(Some(&t)), 1 | 4);
        t.enc_pct = Metric::valid(0.0);
        assert_eq!(reported_bits(Some(&t)), 7);
        assert_eq!(reported_bits(None), 7);
    }

    #[test]
    fn device_engines_follow_reported() {
        use tl_model::system::Metric;
        let device = DeviceIdx(1);
        let mut snap = Snapshot::empty(tl_model::time::BootId(0), 0);
        let all: Vec<String> = device_engines(&snap, device)
            .iter()
            .map(|(k, _)| k.to_string())
            .collect();
        assert_eq!(all, ["gpu", "enc", "dec"], "no record yet: every engine");
        let t = GpuTotals {
            device,
            util_pct: Metric::valid(10.0),
            mem_util_pct: Metric::default(),
            mem_used: Metric::default(),
            mem_total: Metric::default(),
            shared_used: Metric::default(),
            shared_total: Metric::default(),
            enc_pct: Metric::with(Validity::Unsupported),
            dec_pct: Metric::valid(0.0),
            mem_reserved: Metric::default(),
            enc_sessions: Metric::default(),
            enc_dec_shared: false,
            temp_c: Metric::default(),
            temp_edge_c: Metric::default(),
            temp_junction_c: Metric::default(),
            temp_mem_c: Metric::default(),
            power_w: Metric::default(),
            power_limit_w: Metric::default(),
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
        };
        Arc::make_mut(&mut snap.system).gpus = Arc::from([(device, t)].as_slice());
        let engines = device_engines(&snap, device);
        let keys: Vec<&str> = engines.iter().map(|(k, _)| &**k).collect();
        assert_eq!(keys, ["gpu", "dec"]);
        assert_eq!(engines[1].1.as_ref(), "Video Decode");
    }
}
