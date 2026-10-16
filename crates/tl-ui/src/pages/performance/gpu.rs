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
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Div, ElementId, Entity, EntityId,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, StyleRefinement, Styled as _, Subscription, Window, canvas, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{GraphicsAdapter, Snapshot};
use tl_model::system::{GpuTotals, Metric};
use tl_model::time::now_boot_ns;
use tl_model::units;
use tl_model::validity::Validity;

use super::details::{Fallback, KeyValueModel};
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_STACKED, header_text, page_words};
use super::{DeviceKey, DeviceKind, DeviceRow};
use crate::chart::{
    ChartBg, ChartGridSpec, ChartGridView, ChartLabels, ChartSeries, ChartSpec, ChartState,
    ChartView, GridCellSpec, GridStyle, LegendStyle, ValueAlign, ValueFormat, YScale, grid_height,
    grid_layout,
};
use crate::components::menu::TlMenu;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_en};
use crate::settings_global::{Section, TlSettings};
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

/// The `last_error` of the GPU source when the probe binary is missing. The engine's
/// `gpu::NOT_INSTALLED` holds the same text.
pub const PROBE_NOT_INSTALLED: &str = "GPU probe not installed";

const KEY_MEMORY: &str = "perf-gpu-heading-memory";
const KEY_TEMPERATURE: &str = "perf-gpu-heading-temperature";
const KEY_MEMORY_USAGE: &str = "perf-gpu-range-memory";
const KEY_DEDICATED: &str = "perf-gpu-legend-dedicated";
const KEY_SHARED: &str = "perf-gpu-legend-shared";
const KEY_RESTARTING: &str = "perf-gpu-probe-restarting";
const KEY_MISSING: &str = "perf-gpu-probe-missing";

const NS_PER_S: u64 = 1_000_000_000;

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
    /// `xe`. Anything else is `Unknown`.
    pub fn from_kernel_module(module: &str) -> Self {
        match module.trim() {
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

/// The P1 engines in grid order (PERF-063). E25-022 adds the full per-vendor lists.
const P1_ENGINES: [Engine; 3] = [
    Engine {
        key: "gpu",
        name: "perf-gpu-engine-3d",
        metric: MetricId::GPU_UTIL,
        bit: 1,
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

/// The engines of `vendor` in grid order.
fn vendor_engines(vendor: GpuVendor) -> &'static [Engine] {
    // E25-022: AMD and Intel get their own lists.
    let _ = vendor;
    &P1_ENGINES
}

/// The engine cells of a GPU, (key, name), in the vendor's order: every engine in `reported`
/// that is not in `hidden` (`performance.hidden_gpu_engines`). Names are en-US.
pub fn engine_cells(
    vendor: GpuVendor,
    reported: &[Arc<str>],
    hidden: &[Arc<str>],
) -> Vec<(Arc<str>, SharedString)> {
    vendor_engines(vendor)
        .iter()
        .filter(|e| reported.iter().any(|k| &**k == e.key))
        .filter(|e| !hidden.iter().any(|k| &**k == e.key))
        .map(|e| (Arc::from(e.key), tr_en(e.name, &[])))
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
        .filter(move |e| !hidden.iter().any(|k| k.as_ref() == e.key))
}

/// Every engine `device` reports, (key, name), in the vendor's order, hidden ones included.
/// The Settings window's hidden engine checklist lists these (E21-023). Names are en-US.
pub fn device_engines(snap: &Snapshot, device: DeviceIdx) -> Vec<(Arc<str>, SharedString)> {
    let reported = reported_bits(gpu_totals(snap, device));
    vendor_engines(GpuVendor::of(snap, device))
        .iter()
        .filter(|e| reported & e.bit != 0)
        .map(|e| (Arc::from(e.key), tr_en(e.name, &[])))
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

/// The engines a GPU reports, as a mask of [`Engine::bit`]: every P1 engine whose metric is not
/// Unsupported. A GPU without a record yet reports all of them, so the page keeps its shape.
fn reported_bits(totals: Option<&GpuTotals>) -> u8 {
    let Some(t) = totals else {
        return P1_ENGINES.iter().fold(0, |m, e| m | e.bit);
    };
    let mut bits = 0;
    for (e, validity) in
        P1_ENGINES
            .iter()
            .zip([t.util_pct.validity, t.enc_pct.validity, t.dec_pct.validity])
    {
        if validity != Validity::Unsupported {
            bits |= e.bit;
        }
    }
    bits
}

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    /// In [`P1_ENGINES`] order.
    pub engines: [SharedString; 3],
    pub memory: SharedString,
    pub temperature: SharedString,
    pub memory_usage: SharedString,
    pub dedicated: SharedString,
    pub shared: SharedString,
    pub restarting: SharedString,
    pub missing: SharedString,
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
        self.engines = P1_ENGINES.map(|e| t(e.name));
        self.memory = t(KEY_MEMORY);
        self.temperature = t(KEY_TEMPERATURE);
        self.memory_usage = t(KEY_MEMORY_USAGE);
        self.dedicated = t(KEY_DEDICATED);
        self.shared = t(KEY_SHARED);
        self.restarting = t(KEY_RESTARTING);
        self.missing = t(KEY_MISSING);
        true
    }

    fn engine(&self, key: &str) -> SharedString {
        P1_ENGINES
            .iter()
            .position(|e| e.key == key)
            .map(|i| self.engines[i].clone())
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

/// The placeholder cell shown in place of the grid while the probe is missing: "3D / Compute"
/// reading `n/a`.
fn placeholder_labels(words: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(words.engines[0].clone()),
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
    /// The reported engines of the last build, as a mask of [`Engine::bit`].
    reported: u8,
    /// The one-cell "3D / Compute" chart shown while the probe is missing. Created the first
    /// time the probe is missing.
    placeholder: Option<Entity<ChartView>>,
    memory: Entity<ChartView>,
    memory_shape: MemoryShape,
    /// Reused buffer for the Memory value text.
    memory_text: String,
    temperature: Entity<ChartView>,
    /// E25-023.
    #[allow(dead_code, reason = "E25-023 creates it")]
    power: Option<Entity<ChartView>>,
    /// E25-023.
    #[allow(dead_code, reason = "E25-023 creates it")]
    clocks: Option<Entity<ChartView>>,
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
    /// The engine box width of the last paint; 0 before the first.
    width: Rc<Cell<Pixels>>,
    _subs: SmallVec<[Subscription; 3]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
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

        let reported = reported_bits(totals);
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
        let engines = cx.new(|cx| ChartGridView::new(spec, &hub, cx));

        let memory_shape = MemoryShape::of(totals);
        let spec = with_settings(
            memory_spec(row.idx, memory_shape.shared, memory_shape.total, &theme),
            &settings,
        );
        let labels = memory_labels(memory_shape.shared, &words);
        let memory = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));
        let spec = with_settings(temperature_spec(row.idx, &theme), &settings);
        let labels = temperature_labels(&words);
        let temperature = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));

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
            width: Rc::new(Cell::new(px(0.))),
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
            let reported = reported_bits(totals);
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
        if mask.intersects(ComponentMask::SYSTEM | ComponentMask::INVENTORY | ComponentMask::HEALTH)
            || vendor_changed
        {
            let inputs = GpuDetailInputs {
                now_boot_ns: now_boot_ns(),
                observed_end_ns: snap.system.header.observed_end,
                totals: gpu_totals(snap, self.row.idx),
                adapter: adapter(snap, self.row.idx),
                vendor: self.vendor,
                probe: self.probe,
            };
            changed |= update_gpu_details_p1(&mut self.details, &inputs);
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
            self.placeholder = Some(cx.new(|cx| ChartView::new(spec, labels, &hub, cx)));
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
        self.engines.update(cx, |v, cx| v.set_spec(spec, cx));
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
    /// placeholder cell while the probe is missing.
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
            canvas(
                move |bounds, window, cx| {
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

        let root = div()
            .debug_selector(|| ENGINES_ID.into())
            .relative()
            .w_full()
            .h(height);
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
                let grid = div().size_full().child(
                    self.engines
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                );
                root.child(self.with_probe_tip(grid, "perf-gpu-tip-engines", window, cx))
            }
        };
        root.child(measure).into_any_element()
    }

    /// A chart box of `(W − sp4) / 2 × GRAPH_H_STACKED`.
    fn chart_box(
        &self,
        view: &Entity<ChartView>,
        selector: &'static str,
        m: &UiMetrics,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let chart = div()
            .debug_selector(move || selector.into())
            .flex_1()
            .min_w_0()
            .h(px(GRAPH_H_STACKED * m.ui_scale))
            .child(view.clone().cached(StyleRefinement::default().size_full()));
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
/// `Video decode`. Rows 9 to 28 are hidden until E25-024; the merged row is hidden until a
/// record reports a shared engine.
pub fn gpu_details_model() -> KeyValueModel {
    let mut labels = GPU_DETAIL_ROWS.to_vec();
    labels.insert(6, ROW_ENCODE_DECODE);
    let mut model = KeyValueModel::new(&labels).with_counted_text(TIP_SESSIONS, &["n"], 0);
    for label in GPU_DETAIL_ROWS[P1_ROWS..]
        .iter()
        .chain([&ROW_ENCODE_DECODE])
    {
        model.hide(label);
    }
    model
}

/// What rows 1 to 8 are read from.
#[derive(Clone, Copy)]
pub struct GpuDetailInputs<'a> {
    pub now_boot_ns: u64,
    /// `observed_end` of the system component, for the Stale age.
    pub observed_end_ns: u64,
    pub totals: Option<&'a GpuTotals>,
    pub adapter: Option<&'a GraphicsAdapter>,
    pub vendor: GpuVendor,
    pub probe: ProbeState,
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
        ..ChartSpec::default()
    }
}

/// The engine grid spec: one cell per reported engine that is not hidden, and the engine keys
/// in the same order into `keys`.
#[allow(clippy::too_many_arguments)]
fn grid_spec(
    device: DeviceIdx,
    vendor: GpuVendor,
    reported: u8,
    s: &PageSettings,
    words: &Words,
    theme: &TlTheme,
    m: &UiMetrics,
    keys: &mut Vec<Arc<str>>,
) -> ChartGridSpec {
    keys.clear();
    let mut cells = Vec::new();
    for e in vendor_engines(vendor)
        .iter()
        .filter(|e| reported & e.bit != 0)
        .filter(|e| !s.hidden.iter().any(|k| &**k == e.key))
    {
        keys.push(Arc::from(e.key));
        cells.push(GridCellSpec {
            heading: words.engine(e.key),
            fill_series: series(device, e.metric),
            overlay_series: None,
            offline: false,
            mark: None,
            tooltip: None,
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
        hover: false,
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
            .child(self.chart_box(&self.memory, MEMORY_ID, &m, window, cx))
            .child(self.chart_box(&self.temperature, TEMPERATURE_ID, &m, window, cx));
        v_flex()
            .w_full()
            .child(self.engines_element(window, cx))
            .child(div().h(m.sp[4]))
            .child(row2)
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    /// E25-022 adds the engine items.
    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
    }

    /// E25-029 replaces this with the full GPU copy text.
    fn copy_text(&self, out: &mut String) {
        let (word, model) = &self.header;
        out.push_str(word);
        if !model.is_empty() {
            out.push(' ');
            out.push_str(model);
        }
        out.push('\n');
        self.details.copy_into(out);
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
        div().size_full().track_focus(&self.focus).child(
            DevicePageFrame {
                page: self,
                m: &m,
                theme: &theme,
            }
            .render(window, cx),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            fdinfo_partial: false,
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
            fdinfo_partial: false,
        };
        Arc::make_mut(&mut snap.system).gpus = Arc::from([(device, t)].as_slice());
        let engines = device_engines(&snap, device);
        let keys: Vec<&str> = engines.iter().map(|(k, _)| &**k).collect();
        assert_eq!(keys, ["gpu", "dec"]);
        assert_eq!(engines[1].1.as_ref(), "Video Decode");
    }
}
