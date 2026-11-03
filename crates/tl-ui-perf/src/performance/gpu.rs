//! The GPU page: the engine grid, the Memory and Temperature charts side by side, Details and
//! Displays, and the probe states (docs/ui/performance.md UI-PRF-032; PERF-062 to PERF-065,
//! PERF-068; R-054).
//!
//! The page body is one widget container (ADR-019 phase 3, `tasklens-hzire.4`). By default it
//! holds the 1.3.0 parts in their 1.3.0 places (`page_flow::gpu`): the engine grid
//! ([`GpuEngineGridView`] as the fixed widget `gpu_engine_grid`), the Memory and Temperature
//! chart widgets, Power and Clocks when `performance.gpu_optional_graphs` lists them, the
//! Details widget and the Displays block ([`GpuDisplaysView`], fixed widget `gpu_displays`).
//! Edit Widgets adds any engine, all engines, VRAM, shared memory, clocks, power, temperature
//! and fan as chart, bar, value, stats or any mix. Each GPU has its own layout,
//! `page.<gpu key>`.
//!
//! A GPU page converts `gpu_optional_graphs` the first time the window builds it, when the page
//! has no entry and the list is not empty: it arms the `settings.toml.pre-layouts-v2` copy
//! (`AppEnv.keep_pre_layouts_copy`) and writes `convert_gpu_page` as its entry. The "Power" and
//! "Clocks" items of the page menu add or remove those widgets in the page's flow layout; they
//! never write `gpu_optional_graphs`, which every GPU page reads.
//!
//! Render contract: the page renders on a header change, a range change and a layout change,
//! never because of a sample. The engine grid, the charts, Details and Displays are views of
//! their own that notify only for their own changes. The engine grid is one `ChartGridView`
//! (ADR-009 rule 7).
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::devices::{DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{DisplayMode, GraphicsAdapter, Snapshot};
use tl_model::system::{GpuEngine, GpuTotals, Metric};
use tl_model::units;
use tl_model::validity::Validity;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, FixedWidget, FormSet, METRIC_POWER, METRIC_TEMP, MetricKey,
    SourceSelector, WidgetContent, WidgetOptions, WidgetSetting, container_layout,
};
use tl_ui_settings::layout_pages::{
    METRIC_GPU_CLOCKS, METRIC_GPU_MEMORY, convert_page, page_container, page_needs_conversion,
};
use tl_ui_settings::settings::GpuOptionalGraph;

use super::actions::{
    PerfToggleGpuEngine, PerfToggleGpuOptionalGraph, PerfWidgetEditStart, PerfWidgetResetPage,
};
use super::aggregate::{add_demand, page_layout, reset_page, start_page_edit, strings_version};
// The engine model moved to `core/gpu.rs` (tasklens-zljr5.22), and the probe state with it
// (tasklens-hzire.4). These keep the old paths working.
pub(crate) use super::core::gpu::gpu_totals;
#[cfg(test)]
use super::core::gpu::reported_bits;
pub use super::core::gpu::{
    GpuVendor, PROBE_NOT_INSTALLED, ProbeState, adapter, device_engines, engine_cells, probe_state,
};
use super::core::gpu::{KEY_MISSING, KEY_RESTARTING, NAME_3D, Reported, vendor_cells};
use super::details::{Fallback, KeyValueModel};
use super::gpu_views::{
    DisplaysShown, EnginesShown, GpuDisplaysView, GpuEngineGridView, PartResized,
};
pub use super::gpu_views::{ENGINE_CELL_H, ENGINE_CELL_MIN_W, ENGINE_TIP_ID};
use super::page::{
    DevicePage, DevicePageFrame, details_draw, header_text, page_history_request, page_range_ns,
    page_words,
};
use super::page_flow::gpu::{GpuParts, gpu_flow};
use super::widget::container::WidgetContainer;
use super::widget::details_widget::{DetailsResized, DetailsShown, DetailsView};
use super::widget::edit::{WidgetEditor, default_layout};
use super::widget::fixed_widget::FixedContent;
use super::widget::source::SourceStore;
use super::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_base::app_env::AppEnv;
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use tl_ui_base::i18n::{Strings, tr, tr_args, tr_en};
use tl_ui_base::settings_global::{Section, TlSettings};
use tl_ui_base::theme::{TlTheme, UiMetrics, current, metrics};
use tl_ui_chart::chart::{
    ChartBg, ChartGridSpec, ChartGridView, ChartLabels, ChartSeries, ChartSpec, ChartState,
    ChartView, GridCellSpec, GridStyle, LegendStyle, ValueAlign, ValueFormat, YScale,
};
use tl_ui_kit::components::menu::TlMenu;

/// The debug selector prefix of the engine grid box; the box's selector adds the page
/// container id (`gpu_views::engines_id`).
pub const ENGINES_ID: &str = "perf-gpu-engines";

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

const KEY_MEMORY: &str = "perf-gpu-heading-memory";
const KEY_MEMORY_USAGE: &str = "perf-gpu-range-memory";
const KEY_DEDICATED: &str = "perf-gpu-legend-dedicated";
const KEY_SHARED: &str = "perf-gpu-legend-shared";

/// The engines that AMD reads only from fdinfo, so "Own processes only" applies to them.
fn fdinfo_engine(engine: GpuEngine) -> bool {
    matches!(
        engine,
        GpuEngine::Compute | GpuEngine::Dma | GpuEngine::Jpeg
    )
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

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    /// The engine names and the tooltip, by `en-US.ftl` key ([`WORD_KEYS`]).
    pub engines: Vec<(&'static str, SharedString)>,
    /// The Memory chart's words, which the Aggregate graphics page uses too.
    pub memory: SharedString,
    pub memory_usage: SharedString,
    pub dedicated: SharedString,
    pub shared: SharedString,
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
        self.memory_usage = t(KEY_MEMORY_USAGE);
        self.dedicated = t(KEY_DEDICATED);
        self.shared = t(KEY_SHARED);
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
    /// The shared range the engine grid draws.
    range_ns: u64,
    smooth: bool,
    rounded: bool,
    hidden: Vec<Arc<str>>,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: page_range_ns(cx),
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
/// 0..VRAM total. The Aggregate graphics page draws its Memory chart with it.
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

/// A page chart that reads the device metric history at 1 h, 24 h and 7 d.
fn history_chart(
    spec: ChartSpec,
    labels: ChartLabels,
    hub: &Entity<SnapshotHub>,
    cx: &mut Context<ChartView>,
) -> ChartView {
    let snap = Arc::clone(hub.read(cx).current());
    ChartView::new(spec, labels, hub, cx)
        .with_newest_marks()
        .with_device_history(&snap)
}

/// Applies the range, smoothing and corner settings to a chart spec.
fn with_settings(mut spec: ChartSpec, s: &PageSettings) -> ChartSpec {
    spec.range_ns = s.range_ns;
    spec.smooth = s.smooth;
    spec.grid = plot_grid(s);
    spec
}

/// The chart state the engine grid and the placeholder show in `probe`. The chart widgets
/// read the same state from their sources (`SourceData::chart_state`).
fn probe_chart_state(probe: ProbeState, words: &Words) -> ChartState {
    match probe {
        ProbeState::Ok => ChartState::Live,
        ProbeState::Restarting => ChartState::Failed(words.restarting.clone()),
        ProbeState::Missing => ChartState::Unsupported,
    }
}

/// Converts `gpu_optional_graphs` to the page's entry the first time the window builds the
/// GPU page `container` (ADR-019 phase 3 "When"): only when GPU pages are widget containers,
/// the page has no entry and the list is not empty. The first save then keeps
/// `settings.toml.pre-layouts-v2`. The old key stays as it is.
fn convert_on_first_build(container: &str, cx: &mut App) {
    if !cx.has_global::<TlSettings>()
        || !page_needs_conversion(&TlSettings::settings(cx).performance, container)
    {
        return;
    }
    if let Some(env) = cx.try_global::<AppEnv>() {
        (env.keep_pre_layouts_copy)();
    }
    TlSettings::update(cx, |d| {
        let perf = &mut d.settings.performance;
        if let Some(layout) = convert_page(perf, container) {
            perf.layouts.set(layout);
        }
    });
}

/// The metric key of an optional graph: `power` or `clocks`.
fn optional_metric(which: GpuOptionalGraph) -> &'static str {
    match which {
        GpuOptionalGraph::Power => METRIC_POWER,
        GpuOptionalGraph::Clocks => METRIC_GPU_CLOCKS,
    }
}

/// Whether `w` is a metric widget of `metric`.
fn is_metric(w: &WidgetSetting, metric: &str) -> bool {
    matches!(&w.content, WidgetContent::Metric(s) if s.metric.as_str() == metric)
}

/// Adds or removes the `which` chart in the flow layout `layout` of the GPU page of `key`, as
/// the 1.3.0 "Power" and "Clocks" items add or remove the chart (ADR-019 phase 3 "Header
/// buttons and page menu"). An added chart goes where conversion puts it: Power before Clocks,
/// both before Details. The other widgets keep their ids.
pub fn toggle_optional(layout: &mut ContainerLayout, key: &str, which: GpuOptionalGraph) {
    let metric = optional_metric(which);
    if let Some(i) = layout.widgets.iter().position(|w| is_metric(w, metric)) {
        layout.widgets.remove(i);
        return;
    }
    let before = |w: &WidgetSetting| match which {
        GpuOptionalGraph::Power => {
            is_metric(w, METRIC_GPU_CLOCKS) || matches!(w.content, WidgetContent::Fixed(_))
        }
        GpuOptionalGraph::Clocks => {
            matches!(w.content, WidgetContent::Fixed(_))
                && !matches!(w.content, WidgetContent::Fixed(FixedWidget::GpuEngineGrid))
        }
    };
    // After the first chart widget, so the engine grid stays first.
    let first_chart = layout
        .widgets
        .iter()
        .position(|w| matches!(w.content, WidgetContent::Metric(_)))
        .unwrap_or(0);
    let at = layout
        .widgets
        .iter()
        .enumerate()
        .skip(first_chart)
        .find(|(_, w)| before(w))
        .map_or(layout.widgets.len(), |(i, _)| i);
    let id = layout.next_id;
    layout.next_id += 1;
    layout.widgets.insert(
        at,
        WidgetSetting {
            id,
            content: WidgetContent::Metric(SourceSelector {
                device: DeviceSelector::Key(Arc::from(key)),
                metric: MetricKey::new(metric),
            }),
            forms: FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
            cell: None,
            options: WidgetOptions::default(),
            device_name: None,
        },
    );
}

/// The GPU device page.
pub struct GpuPage {
    key: DeviceKey,
    row: DeviceRow,
    vendor: GpuVendor,
    /// The container id, `page.<gpu key>`.
    container_id: SharedString,
    engines: Entity<ChartGridView>,
    /// Engine keys in grid order, one per grid cell.
    engine_keys: Vec<Arc<str>>,
    /// The engines the last grid build drew from, and the "Own processes only" flag.
    reported: Reported,
    /// The one-cell "3D / Compute" chart shown while the probe is missing. Created the first
    /// time the probe is missing.
    placeholder: Option<Entity<ChartView>>,
    engines_view: Entity<GpuEngineGridView>,
    /// The cells the engine grid draws, shared with the flow placer: 0 takes no space.
    engine_cells: Rc<Cell<usize>>,
    displays_view: Entity<GpuDisplaysView>,
    details: KeyValueModel,
    details_view: Entity<DetailsView>,
    container: Entity<WidgetContainer>,
    editor: Entity<WidgetEditor>,
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
    /// `UiMetrics.style_version` the grid's colours and sizes were built for.
    style_version: u64,
    words: Words,
    /// One formatted row per display of this GPU's adapter (PERF-073).
    display_rows: Vec<SharedString>,
    _subs: SmallVec<[Subscription; 10]>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl GpuPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let container_id = SharedString::from(page_container(&key.0));
        convert_on_first_build(&container_id, cx);
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
        let engines = cx.new(|cx| ChartGridView::new(spec, &hub, cx).with_device_history(&snap));
        let shown = EnginesShown {
            keys: engine_keys.clone(),
            missing: false,
            probe_tip: None,
        };
        let engine_cells = Rc::new(Cell::new(shown.cells()));
        let engines_view = {
            let (id, grid) = (container_id.clone(), engines.clone());
            cx.new(|_| GpuEngineGridView::new(id, grid, None, shown))
        };
        let displays_view = {
            let shown = DisplaysShown {
                heading: words.displays.clone(),
                no_display: words.no_display.clone(),
                rows: Vec::new(),
            };
            let id = container_id.clone();
            cx.new(|_| GpuDisplaysView::new(id, shown))
        };
        let mut details = gpu_details_model();
        details.refresh_strings(cx);
        let details_view = {
            let shown = DetailsShown::of(&details, strings_version(cx));
            let draw = details_draw(cx.weak_entity());
            let id = container_id.clone();
            cx.new(|_| DetailsView::new(id, shown, draw))
        };
        let parts = GpuParts {
            engines: engines_view.read(cx).natural_height(),
            engine_cells: Rc::clone(&engine_cells),
            details: details_view.read(cx).natural_height(),
            displays: displays_view.read(cx).natural_height(),
        };
        let content: FixedContent = {
            let details: AnyView = details_view.clone().into();
            let engines: AnyView = engines_view.clone().into();
            let displays: AnyView = displays_view.clone().into();
            Rc::new(move |kind: &FixedWidget, _: &mut App| match kind {
                FixedWidget::Details => Some(details.clone()),
                FixedWidget::GpuEngineGrid => Some(engines.clone()),
                FixedWidget::GpuDisplays => Some(displays.clone()),
                _ => None,
            })
        };
        let container = {
            let store = Rc::new(std::cell::RefCell::new(SourceStore::for_page(&hub)));
            let layout = page_layout(&container_id, cx);
            let id = container_id.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut c = WidgetContainer::new(id, layout, &hub, store, cx);
                c.set_flow_placer(Some(gpu_flow(parts)), cx);
                c.set_fixed_content(Some(content), cx);
                c
            })
        };
        let editor = {
            let container = container.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut e = WidgetEditor::new(container, &hub, cx);
                e.set_page_parts(vec![
                    FixedWidget::Details,
                    FixedWidget::GpuEngineGrid,
                    FixedWidget::GpuDisplays,
                ]);
                e
            })
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<UiMetrics>(Self::sync_style));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        // A part got taller or shorter: the parts under it move.
        subs.push(
            cx.subscribe(&details_view, |this, _, _: &DetailsResized, cx| {
                this.place_again(cx);
            }),
        );
        subs.push(cx.subscribe(&engines_view, |this, _, _: &PartResized, cx| {
            this.place_again(cx);
        }));
        subs.push(
            cx.subscribe(&displays_view, |this, _, _: &PartResized, cx| {
                this.place_again(cx);
            }),
        );
        // A layout change can change what the page asks for.
        subs.push(cx.observe(&container, |this, _, cx| this.send_demand(cx)));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            vendor,
            container_id,
            engines,
            engine_keys,
            reported,
            placeholder: None,
            engines_view,
            engine_cells,
            displays_view,
            details,
            details_view,
            container,
            editor,
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
            _subs: subs,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
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

    /// The engine grid part (fixed widget `gpu_engine_grid`).
    pub fn engines_view(&self) -> &Entity<GpuEngineGridView> {
        &self.engines_view
    }

    /// The Displays part (fixed widget `gpu_displays`).
    pub fn displays_view(&self) -> &Entity<GpuDisplaysView> {
        &self.displays_view
    }

    /// The page's widget container.
    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    /// The editor around the container (Edit Widgets).
    pub fn editor(&self) -> &Entity<WidgetEditor> {
        &self.editor
    }

    /// The Details widget's view.
    pub fn details_view(&self) -> &Entity<DetailsView> {
        &self.details_view
    }

    /// The container id, `page.<gpu key>`.
    pub fn container_id(&self) -> &SharedString {
        &self.container_id
    }

    /// The chart of the first placed widget of `metric` that draws one.
    pub fn chart_of(&self, metric: &str, cx: &App) -> Option<Entity<ChartView>> {
        let c = self.container.read(cx);
        c.layout()
            .widgets
            .iter()
            .filter(|w| is_metric(w, metric))
            .find_map(|w| c.widget(w.id)?.read(cx).chart().cloned())
    }

    /// The Memory chart, while the page shows it.
    pub fn memory(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_GPU_MEMORY, cx)
    }

    /// The Temperature chart, while the page shows it.
    pub fn temperature(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_TEMP, cx)
    }

    /// The Power chart, while the page shows it.
    pub fn power(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_POWER, cx)
    }

    /// The Clocks chart, while the page shows it.
    pub fn clocks(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_GPU_CLOCKS, cx)
    }

    /// The tooltip on the name of the first placed widget of `metric` (the Power and Clocks
    /// heading texts, or the probe's text).
    pub fn tip_of(&self, metric: &str, cx: &App) -> Option<SharedString> {
        let c = self.container.read(cx);
        c.layout()
            .widgets
            .iter()
            .filter(|w| is_metric(w, metric))
            .find_map(|w| c.source(w.id)?.read(cx).tip().cloned())
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

    /// This page's demand part while it is active: the GPU component, the inventory for the
    /// adapter's vendor and static rows, the device metric history at 1 h, 24 h and 7 d, and
    /// what the placed widgets read. With the default layout it equals 1.3.0's.
    pub fn demand_part(&self, cx: &App) -> Demand {
        let mut demand = Demand {
            any_surface_visible: true,
            gpus: true,
            inventory: true,
            device_history_range: page_history_request(self.settings.range_ns),
            ..Demand::default()
        };
        add_demand(&mut demand, &self.container.read(cx).demand_part());
        demand
    }

    fn send_demand(&self, cx: &mut App) {
        if !self.active {
            return;
        }
        let (id, window, part) = (self.id, self.window, self.demand_part(cx));
        self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
    }

    fn notify(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// A part's height changed: the flow layout places the parts again.
    fn place_again(&mut self, cx: &mut Context<Self>) {
        self.container.update(cx, |c, cx| {
            c.place_again(cx);
        });
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

    /// Brings the engine grid, the probe state, Details, Displays and the header up to date
    /// with the components in `mask`. Only a header change renders the page; each part
    /// notifies for its own change.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
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
            }
        }

        if mask.contains(ComponentMask::SYSTEM) {
            let totals = gpu_totals(snap, self.row.idx);
            let reported = Reported::of(self.vendor, totals);
            if reported != self.reported {
                self.reported = reported;
                self.rebuild_grid(cx);
            }
        }
        if mask.contains(ComponentMask::HEALTH) || vendor_changed {
            let probe = probe_state(self.vendor, snap);
            if probe != self.probe {
                self.probe = probe;
                self.apply_probe(cx);
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
                now_boot_ns: self.hub.read(cx).now_ns(),
                observed_end_ns: snap.system.header.observed_end,
                totals: gpu_totals(snap, self.row.idx),
                adapter: adapter(snap, self.row.idx),
                vendor: self.vendor,
                probe: self.probe,
                uuid,
                pci_bus_id,
            };
            let p1 = update_gpu_details_p1(&mut self.details, &inputs);
            if update_gpu_details_p2(&mut self.details, &inputs) || p1 {
                self.sync_details(cx);
            }
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
                self.sync_displays(cx);
            }
        }
        if (devices || inventory)
            && let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx)
        {
            let header = header_text(page_words(cx), DeviceKind::Gpu, 0, entry, snap);
            if header != self.header {
                let word = header.0.clone();
                self.editor.update(cx, |e, _| e.set_page_name(word));
                self.header = header;
                self.notify(cx);
            }
        }
        // The GPU went away: an edit ends as Cancel does.
        if devices
            && !snap
                .devices
                .entries
                .iter()
                .any(|e| e.idx == self.row.idx && e.present)
        {
            self.editor.update(cx, |e, cx| e.cancel(cx));
        }
    }

    /// Hands the Details view what the grid shows; it notifies only when that changed.
    fn sync_details(&mut self, cx: &mut Context<Self>) {
        let shown = DetailsShown::of(&self.details, strings_version(cx));
        self.details_view.update(cx, |v, cx| v.set_shown(shown, cx));
    }

    /// Hands the Displays view its heading and rows; it notifies only when they changed.
    fn sync_displays(&mut self, cx: &mut Context<Self>) {
        let shown = DisplaysShown {
            heading: self.words.displays.clone(),
            no_display: self.words.no_display.clone(),
            rows: self.display_rows.clone(),
        };
        self.displays_view
            .update(cx, |v, cx| v.set_shown(shown, cx));
    }

    /// Hands the engine grid view its cells and probe state. The flow places the parts again
    /// when the grid starts or stops taking space.
    fn sync_engines(&mut self, cx: &mut Context<Self>) {
        let shown = EnginesShown {
            keys: self.engine_keys.clone(),
            missing: self.probe == ProbeState::Missing,
            probe_tip: self.probe_tooltip().cloned(),
        };
        let cells = shown.cells();
        let placeholder = self.placeholder.clone();
        self.engines_view
            .update(cx, |v, cx| v.set_shown(shown, placeholder, cx));
        if (self.engine_cells.replace(cells) == 0) != (cells == 0) {
            self.place_again(cx);
        }
    }

    /// Sets the engine grid's state for the probe state, creating the placeholder cell the
    /// first time the probe is missing. The chart widgets read the probe state from their
    /// sources.
    fn apply_probe(&mut self, cx: &mut Context<Self>) {
        let state = probe_chart_state(self.probe, &self.words);
        if self.probe == ProbeState::Missing && self.placeholder.is_none() {
            let theme = current(cx).clone();
            let spec = with_settings(placeholder_spec(self.row.idx, &theme), &self.settings);
            let labels = placeholder_labels(&self.words);
            let hub = self.hub.clone();
            self.placeholder = Some(cx.new(|cx| history_chart(spec, labels, &hub, cx)));
        }
        if let Some(p) = &self.placeholder {
            let state = state.clone();
            p.update(cx, |v, cx| v.set_state(state, cx));
        }
        // Restarting keeps the per-engine cells, each reading `!`. Missing shows the placeholder.
        self.engines.update(cx, |v, cx| v.set_state(state, cx));
        self.sync_engines(cx);
    }

    fn rebuild_grid(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let mut spec = grid_spec(
            self.row.idx,
            self.vendor,
            self.reported,
            &self.settings,
            &self.words,
            &theme,
            metrics(cx),
            &mut self.engine_keys,
        );
        // The engine view gives the cells the row height its box gives them.
        spec.cell_h = spec.cell_h.max(self.engines.read(cx).spec.cell_h);
        self.engines.update(cx, |v, cx| v.set_spec(spec, cx));
        self.sync_engines(cx);
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

    /// `PerfToggleGpuOptionalGraph`: adds or removes the Power or Clocks chart in this page's
    /// flow layout. A page without an entry gets one, from its built-in default.
    /// `gpu_optional_graphs` is not written. Nothing with a grid layout, whose menu hides the
    /// items.
    fn on_toggle_optional(
        &mut self,
        a: &PerfToggleGpuOptionalGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if a.key != self.key {
            return;
        }
        let container = self.container_id.to_string();
        let key = Arc::clone(&self.key.0);
        let which = a.which;
        TlSettings::update(cx, |d| {
            let perf = &mut d.settings.performance;
            let mut layout =
                container_layout(perf, &container).unwrap_or_else(|| default_layout(&container));
            if layout.preset.is_none() {
                return;
            }
            toggle_optional(&mut layout, &key, which);
            perf.layouts.set(layout);
        });
        self.sync_settings(cx);
    }

    /// Applies the page's settings when one of their sections changed. A saved layout (Apply,
    /// Reset Page, a menu item) shows when the page is not in Edit Widgets.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        if !self.editor.read(cx).is_editing() {
            let layout = page_layout(&self.container_id, cx);
            self.container.update(cx, |c, cx| c.set_layout(layout, cx));
        }
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        if new == self.settings {
            return;
        }
        let range_moved = new.range_ns != self.settings.range_ns;
        self.settings = new;
        self.rebuild_specs(cx);
        if range_moved {
            // The header switch shows the new range.
            self.send_demand(cx);
            self.notify(cx);
        }
    }

    /// Rebuilds the engine grid's and the placeholder's specs from the current settings and
    /// theme.
    fn rebuild_specs(&mut self, cx: &mut Context<Self>) {
        self.rebuild_grid(cx);
        if let Some(p) = &self.placeholder {
            let theme = current(cx).clone();
            let spec = with_settings(placeholder_spec(self.row.idx, &theme), &self.settings);
            p.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }

    /// A new theme or UI scale: the grid takes the new colours and sizes.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_specs(cx);
    }

    /// New strings: the engine names, the placeholder, the probe text, Details, Displays and
    /// the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        if !self.words.refresh(cx) {
            return;
        }
        self.details.refresh_strings(cx);
        self.sync_details(cx);
        if let Some(p) = &self.placeholder {
            let labels = placeholder_labels(&self.words);
            p.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        self.rebuild_grid(cx);
        self.apply_probe(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.apply_snapshot(&snap, ComponentMask::INVENTORY, cx);
        self.sync_displays(cx);
    }

    /// "Edit Widgets…" in the page menu.
    fn on_edit_start(
        &mut self,
        _: &PerfWidgetEditStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        start_page_edit(&self.editor, &self.hub, window, cx);
    }

    /// "Reset Page" in the page menu (ADR-019 phase 3 "Reset"). A GPU page saves its built-in
    /// default as an explicit entry (`reset_page_to_default`).
    fn on_reset_page(
        &mut self,
        _: &PerfWidgetResetPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        reset_page(&self.container_id, self.header.0.clone(), window, cx);
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
    fn hub(&self) -> Option<&Entity<SnapshotHub>> {
        Some(&self.hub)
    }

    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, _: &mut App) -> AnyElement {
        // The engine grid and the charts are widgets of the container.
        div().into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        // Displays is a widget of the container.
        SmallVec::new()
    }

    /// "Select GPU engine graphs >": one check item per reported engine while the page has the
    /// engine grid, then the Power and Clocks graphs, checked from the page's flow layout
    /// (PERF-063, UI-PRF-032). A grid layout hides Power and Clocks (ADR-019 phase 3).
    fn menu_items(&self, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
        let layout = page_layout(&self.container_id, cx);
        let has_grid = layout
            .widgets
            .iter()
            .any(|w| matches!(w.content, WidgetContent::Fixed(FixedWidget::GpuEngineGrid)));
        let flow = layout.preset.is_some();
        if !has_grid && !flow {
            return menu;
        }
        let key = self.key.clone();
        let items: Vec<(Arc<str>, SharedString, bool)> = if has_grid {
            vendor_cells(self.vendor)
                .iter()
                .filter(|c| self.reported.has(c.engine))
                .map(|c| {
                    (
                        Arc::from(c.engine.key()),
                        self.words.engine(c.name),
                        !c.engine.is_hidden(&self.settings.hidden),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let on = |metric| layout.widgets.iter().any(|w| is_metric(w, metric));
        let optional = flow.then(|| {
            (
                (tr(cx, "perf-gpu-menu-power"), on(METRIC_POWER)),
                (tr(cx, "perf-gpu-menu-clocks"), on(METRIC_GPU_CLOCKS)),
            )
        });
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
                let Some(((power, power_on), (clocks, clocks_on))) = &optional else {
                    return menu;
                };
                let action = |which| {
                    Box::new(PerfToggleGpuOptionalGraph {
                        key: key.clone(),
                        which,
                    })
                };
                if !items.is_empty() {
                    menu = menu.separator();
                }
                menu.check_item(power.clone(), *power_on, action(GpuOptionalGraph::Power))
                    .check_item(clocks.clone(), *clocks_on, action(GpuOptionalGraph::Clocks))
            },
        )
    }

    /// The GPU copy text (PERF-011, UI-PRF-022): the header lines, the Details rows, then the
    /// Displays block with one indented line per display, whatever the layout holds.
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

    fn widget_editor(&self) -> Option<&Entity<WidgetEditor>> {
        Some(&self.editor)
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
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_toggle_engine))
            .on_action(cx.listener(Self::on_toggle_optional))
            .on_action(cx.listener(Self::on_edit_start))
            .on_action(cx.listener(Self::on_reset_page))
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
