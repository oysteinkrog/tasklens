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
use tl_model::devices::DeviceIdentity;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::Snapshot;
use tl_model::system::GpuTotals;
use tl_model::validity::Validity;

use super::details::KeyValueModel;
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
    /// The catalog carries no vendor yet. A GPU with a UUID comes from NVML, so it is NVIDIA;
    /// every other GPU is `Unknown` until the device record names the vendor.
    pub fn from_identity(identity: &DeviceIdentity) -> Self {
        match identity {
            DeviceIdentity::Gpu { uuid, .. } if *uuid != [0; 16] => Self::Nvidia,
            _ => Self::Unknown,
        }
    }
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
struct Engine {
    key: &'static str,
    name: &'static str,
    metric: MetricId,
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
        let vendor = snap
            .devices
            .entries
            .iter()
            .find(|e| e.idx == row.idx)
            .map_or(GpuVendor::Unknown, |e| {
                GpuVendor::from_identity(&e.identity)
            });
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
            temperature,
            power: None,
            clocks: None,
            details: KeyValueModel::default(),
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

    /// This page's demand part while it is active: the GPU component.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            gpus: true,
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
        }
        if mask.contains(ComponentMask::HEALTH) {
            let probe = probe_state(self.vendor, snap);
            if probe != self.probe {
                self.probe = probe;
                self.apply_probe(cx);
                changed = true;
            }
        }
        if (devices || mask.contains(ComponentMask::INVENTORY))
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

/// The record of `device` in the snapshot.
fn gpu_totals(snap: &Snapshot, device: DeviceIdx) -> Option<&GpuTotals> {
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
            temp_c: Metric::default(),
            power_w: Metric::default(),
            power_limit_w: Metric::default(),
            clock_gfx_mhz: Metric::default(),
            clock_mem_mhz: Metric::default(),
            fan_pct: Metric::default(),
            pcie_rx_bps: Metric::default(),
            pcie_tx_bps: Metric::default(),
        };
        assert_eq!(reported_bits(Some(&t)), 1 | 4);
        t.enc_pct = Metric::valid(0.0);
        assert_eq!(reported_bits(Some(&t)), 7);
        assert_eq!(reported_bits(None), 7);
    }
}
