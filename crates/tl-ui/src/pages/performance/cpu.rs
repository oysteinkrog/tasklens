//! The CPU page: the Overall chart or the per-core grid, the mode and kernel toggles, the Details
//! grid, and the demand that switches the engine to 10 Hz per-core sampling (docs/ui/performance.md
//! UI-PRF-023, UI-PRF-053; PERF-027, PERF-028, PERF-029, PERF-033; ADR-008, ADR-014; R-126).
//!
//! The page re-renders only when its header, Details rows or toggles change. Both charts are
//! cached views that re-render for their own series. The settings observer reads the page's
//! settings only when the Performance, Motion or Appearance section version moved.
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, AppContext as _, AsyncApp,
    Bounds, Context, Entity, EntityId, FocusHandle, Focusable, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, StyleRefinement, Styled as _,
    Subscription, WeakEntity, Window, canvas, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::devices::{DeviceCatalog, DeviceIdentity};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::{CpuGraphMode, NetworkUnit};
use tl_model::snapshot::{CpuStaticInfo, Snapshot};
use tl_model::system::{CpuTopology, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use super::actions::{PerfSetCpuGraphMode, PerfToggleCpuKernel};
use super::cpu_details::{
    CpuLiveInputs, CpuSensor, cpu_details_model, cpu_temp_sensors, update_live, update_static,
};
use super::cpu_grid::CpuGrid;
use super::details::KeyValueModel;
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_OVERALL, header_text, page_words};
use super::{DeviceKey, DeviceKind, DeviceRow};
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartState, ChartView, GridStyle, LegendStyle,
    ValueAlign, ValueFormat, YScale, format_value, grid_height, grid_layout,
};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::motion::{M_BASE, ease_out_cubic};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, TlSettings, animations_on};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The overlaid and stacked modes are in the mode menu (E24-018).
pub const CPU_EXTRA_MODES_ENABLED: bool = true;

/// Element id of the mode button.
pub const MODE_BUTTON_ID: &str = "perf-cpu-mode";
/// Element id of the kernel toggle.
pub const KERNEL_BUTTON_ID: &str = "perf-cpu-kernel";
/// Debug selector of the box around the Overall chart.
pub const OVERALL_ID: &str = "perf-cpu-overall";
/// Debug selector of the box around the overlaid chart.
pub const OVERLAID_ID: &str = "perf-cpu-overlaid";
/// Debug selector of the box around the stacked chart.
pub const STACKED_ID: &str = "perf-cpu-stacked";

/// Lowest line alpha in the overlaid mode (PERF-029).
const OVERLAID_MIN_ALPHA: f32 = 0.08;
/// Band alphas in the stacked mode, alternating. No document gives a per-band colour step, so
/// every band is `cpu_fill` and the alpha alternates (plan open item 7).
const STACKED_ALPHAS: [f32; 2] = [0.85, 0.70];

const KEY_OVERALL: &str = "perf-cpu-mode-overall";
const KEY_PER_CORE: &str = "perf-cpu-mode-per-core";
const KEY_OVERLAID: &str = "perf-cpu-mode-overlaid";
const KEY_STACKED: &str = "perf-cpu-mode-stacked";
const KEY_KERNEL_ON: &str = "perf-cpu-kernel-on";
const KEY_KERNEL_OFF: &str = "perf-cpu-kernel-off";
const KEY_TIP_MODE: &str = "perf-cpu-tip-mode";
const KEY_TIP_KERNEL: &str = "perf-cpu-tip-kernel";
const KEY_LATER: &str = "perf-later-version";
const KEY_MENU_GRAPH: &str = "perf-cpu-menu-graph";
const KEY_MENU_KERNEL_TIMES: &str = "perf-cpu-menu-kernel-times";
const KEY_UTILIZATION: &str = "perf-cpu-heading-utilization";
const KEY_TOTAL: &str = "perf-legend-total";
const KEY_KERNEL: &str = "perf-legend-kernel";

const NS_PER_S: u64 = 1_000_000_000;

/// One entry of the mode menu. `label` and `tooltip` are `en-US.ftl` keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeItem {
    pub mode: CpuGraphMode,
    pub label: &'static str,
    pub enabled: bool,
    /// Why the item is disabled.
    pub tooltip: Option<&'static str>,
}

/// The four mode items in menu order. Overlaid and stacked are disabled until
/// [`CPU_EXTRA_MODES_ENABLED`].
pub const fn mode_items() -> [ModeItem; 4] {
    const fn item(mode: CpuGraphMode, extra: bool) -> ModeItem {
        let enabled = !extra || CPU_EXTRA_MODES_ENABLED;
        ModeItem {
            mode,
            label: mode_key(mode),
            enabled,
            tooltip: if enabled { None } else { Some(KEY_LATER) },
        }
    }
    [
        item(CpuGraphMode::Overall, false),
        item(CpuGraphMode::PerCore, false),
        item(CpuGraphMode::AllCoresOverlaid, true),
        item(CpuGraphMode::AllCoresStacked, true),
    ]
}

/// The `en-US.ftl` key of `mode`'s name.
pub const fn mode_key(mode: CpuGraphMode) -> &'static str {
    match mode {
        CpuGraphMode::Overall => KEY_OVERALL,
        CpuGraphMode::PerCore => KEY_PER_CORE,
        CpuGraphMode::AllCoresOverlaid => KEY_OVERLAID,
        CpuGraphMode::AllCoresStacked => KEY_STACKED,
    }
}

/// The index of `mode` in [`mode_items`].
const fn mode_index(mode: CpuGraphMode) -> usize {
    match mode {
        CpuGraphMode::Overall => 0,
        CpuGraphMode::PerCore => 1,
        CpuGraphMode::AllCoresOverlaid => 2,
        CpuGraphMode::AllCoresStacked => 3,
    }
}

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    /// In [`mode_items`] order.
    pub modes: [SharedString; 4],
    pub kernel_on: SharedString,
    pub kernel_off: SharedString,
    pub tip_mode: SharedString,
    pub tip_kernel: SharedString,
    pub later: SharedString,
    pub menu_graph: SharedString,
    pub menu_kernel_times: SharedString,
    pub utilization: SharedString,
    pub total: SharedString,
    pub kernel: SharedString,
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
        self.modes = mode_items().map(|i| t(i.label));
        self.kernel_on = t(KEY_KERNEL_ON);
        self.kernel_off = t(KEY_KERNEL_OFF);
        self.tip_mode = t(KEY_TIP_MODE);
        self.tip_kernel = t(KEY_TIP_KERNEL);
        self.later = t(KEY_LATER);
        self.menu_graph = t(KEY_MENU_GRAPH);
        self.menu_kernel_times = t(KEY_MENU_KERNEL_TIMES);
        self.utilization = t(KEY_UTILIZATION);
        self.total = t(KEY_TOTAL);
        self.kernel = t(KEY_KERNEL);
        true
    }

    pub fn mode(&self, mode: CpuGraphMode) -> &SharedString {
        &self.modes[mode_index(mode)]
    }

    pub fn kernel_toggle(&self, kernel: bool) -> &SharedString {
        if kernel {
            &self.kernel_on
        } else {
            &self.kernel_off
        }
    }
}

/// Adds the four mode radios; a mode not yet available is a disabled item with its tooltip.
fn mode_menu(
    mut menu: TlMenu,
    current: CpuGraphMode,
    labels: &[SharedString; 4],
    later: &SharedString,
) -> TlMenu {
    for (item, label) in mode_items().into_iter().zip(labels) {
        menu = if item.enabled {
            menu.radio_item(
                label.clone(),
                item.mode == current,
                Box::new(PerfSetCpuGraphMode { mode: item.mode }),
            )
        } else {
            menu.item_disabled(label.clone(), Some(later.clone()))
        };
    }
    menu
}

/// The chart settings the overlaid and stacked specs read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSettingsView {
    pub range_ns: u64,
    pub smooth: bool,
    pub rounded: bool,
}

/// Debug selectors of the two areas during a cross-fade.
pub const FADE_OLD_ID: &str = "perf-cpu-fade-old";
pub const FADE_NEW_ID: &str = "perf-cpu-fade-new";

/// A mode change cross-fading over `m_base` (UI-PRF-023).
#[derive(Clone, Copy, Debug)]
struct ModeFade {
    from: CpuGraphMode,
    /// Bumped on each change, so a new change restarts the animation.
    generation: u32,
}

/// The settings the page draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSettings {
    mode: CpuGraphMode,
    kernel: bool,
    range_ns: u64,
    smooth: bool,
    rounded: bool,
}

impl PageSettings {
    fn view(&self) -> PageSettingsView {
        PageSettingsView {
            range_ns: self.range_ns,
            smooth: self.smooth,
            rounded: self.rounded,
        }
    }

    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            mode: s.performance.cpu_graph_mode,
            kernel: s.performance.cpu_kernel,
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
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

/// The Overall chart: total with fill, kernel as a line when `kernel`, 0..100.
fn overall_spec(device: DeviceIdx, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let mut series = smallvec![ChartSeries {
        id: SeriesId {
            device,
            metric: MetricId::CPU_TOTAL,
        },
        fill: Some(theme.graph.cpu_fill),
        line: theme.graph.cpu_line,
        ..ChartSeries::default()
    }];
    if s.kernel {
        series.push(ChartSeries {
            id: SeriesId {
                device,
                metric: MetricId::CPU_KERNEL,
            },
            fill: None,
            line: theme.graph.kernel_line,
            ..ChartSeries::default()
        });
    }
    ChartSpec {
        series,
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
        },
        smooth: s.smooth,
        background: ChartBg::Window,
        ..ChartSpec::default()
    }
}

fn overall_labels(kernel: bool, words: &Words) -> ChartLabels {
    let mut legend = smallvec![(words.total.clone(), LegendStyle::Solid)];
    if kernel {
        legend.push((words.kernel.clone(), LegendStyle::Dashed));
    }
    ChartLabels {
        heading: Some(words.utilization.clone()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: words.utilization.clone(),
        legend,
        show_axis_max: false,
    }
}

/// The logical CPUs that have a catalog entry, ascending by id.
fn present(logical: &[DeviceIdx]) -> impl Iterator<Item = DeviceIdx> + '_ {
    logical.iter().copied().filter(|d| d.0 != u16::MAX)
}

/// The overlaid chart: one line per logical CPU at `max(1/n, 0.08)` alpha, no fill, 0..100.
pub fn overlaid_spec(logical: &[DeviceIdx], s: &PageSettingsView, theme: &TlTheme) -> ChartSpec {
    let n = present(logical).count().max(1);
    let line = theme.graph.cpu_line;
    let line = Hsla {
        a: (1.0 / n as f32).max(OVERLAID_MIN_ALPHA),
        ..line
    };
    ChartSpec {
        series: present(logical)
            .map(|device| ChartSeries {
                id: SeriesId {
                    device,
                    metric: MetricId::CPU_TOTAL,
                },
                fill: None,
                line,
                ..ChartSeries::default()
            })
            .collect(),
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        ..base_spec(s)
    }
}

/// The stacked chart: one filled band per logical CPU, 0..100 × n.
pub fn stacked_spec(logical: &[DeviceIdx], s: &PageSettingsView, theme: &TlTheme) -> ChartSpec {
    let n = present(logical).count().max(1);
    let fill = theme.graph.cpu_fill;
    ChartSpec {
        series: present(logical)
            .enumerate()
            .map(|(k, device)| ChartSeries {
                id: SeriesId {
                    device,
                    metric: MetricId::CPU_TOTAL,
                },
                fill: Some(Hsla {
                    a: STACKED_ALPHAS[k % 2],
                    ..fill
                }),
                line: theme.graph.cpu_line,
                ..ChartSeries::default()
            })
            .collect(),
        y: YScale::Stacked(u16::try_from(n).unwrap_or(u16::MAX)),
        stack: true,
        ..base_spec(s)
    }
}

/// The range, smoothing and grid every CPU chart shares.
fn base_spec(s: &PageSettingsView) -> ChartSpec {
    ChartSpec {
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
        },
        smooth: s.smooth,
        background: ChartBg::Window,
        ..ChartSpec::default()
    }
}

/// The overlaid and stacked labels: the Utilization heading, no legend; the stacked chart
/// shows its axis max ("1600%").
fn all_cores_labels(stacked: bool, words: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(words.utilization.clone()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: words.utilization.clone(),
        legend: SmallVec::new(),
        show_axis_max: stacked,
    }
}

/// `DeviceIdx` per logical CPU id from the catalog. An id the catalog skips gets
/// `DeviceIdx(u16::MAX)`, which matches no series.
fn logical_devices(catalog: &DeviceCatalog, out: &mut Vec<DeviceIdx>) {
    out.clear();
    for e in catalog.entries.iter() {
        if let DeviceIdentity::CpuLogical { id } = e.identity {
            let i = usize::from(id);
            if out.len() <= i {
                out.resize(i + 1, DeviceIdx(u16::MAX));
            }
            out[i] = e.idx;
        }
    }
}

/// The chart state the owner sets from the CPU source health: Failed with its error, else Live
/// (the view derives WarmingUp and Stale itself).
fn health_state(snap: &Snapshot) -> ChartState {
    let h = snap.health.get(SourceId::Cpu);
    match h.state {
        SourceState::Failed => ChartState::Failed(
            h.last_error
                .as_deref()
                .unwrap_or_default()
                .to_owned()
                .into(),
        ),
        _ => ChartState::Live,
    }
}

/// The CPU device page.
pub struct CpuPage {
    key: DeviceKey,
    row: DeviceRow,
    overall: Entity<ChartView>,
    grid: CpuGrid,
    /// Created the first time the overlaid mode is shown.
    overlaid: Option<Entity<ChartView>>,
    /// Created the first time the stacked mode is shown.
    stacked: Option<Entity<ChartView>>,
    /// The mode change cross-fading now, if any.
    fade: Option<ModeFade>,
    fade_generation: u32,
    /// The graph area width of the last paint, for the per-core grid height during a fade.
    area_width: Rc<Cell<Pixels>>,
    /// E24-021.
    #[allow(dead_code, reason = "E24-021 creates it")]
    secondary: Option<Entity<ChartView>>,
    details: KeyValueModel,
    mode: CpuGraphMode,
    kernel: bool,
    /// Bench only (UI-PRF-053, R-160 OI-5): the Overall chart above the grid.
    #[cfg(feature = "bench")]
    pub bench_show_both: bool,
    mode_button_focus: FocusHandle,
    kernel_button_focus: FocusHandle,
    /// The page root; actions from the toggles and the mode menu are dispatched here.
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
    /// `UiMetrics.style_version` the grid and chart colours were built for.
    style_version: u64,
    words: Words,
    /// The mode button's box of the last paint; the mode menu opens below it.
    mode_button_bounds: Rc<Cell<Bounds<Pixels>>>,
    catalog_version: u64,
    topology: Arc<CpuTopology>,
    logical: Vec<DeviceIdx>,
    static_info: Arc<CpuStaticInfo>,
    temp_sensors: Vec<CpuSensor>,
    /// The per-CPU clocks last handed to the grid.
    clocks: Arc<[Metric<u32>]>,
    /// Reused for `CpuGrid::update_clocks`.
    clock_buf: Vec<(u16, f32, Validity)>,
    /// Overall CPU utilization of the newest sample, the value the all-cores charts show.
    total: Metric<f32>,
    /// Reused buffer for that value's text.
    total_text: String,
    _subs: SmallVec<[Subscription; 3]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl CpuPage {
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

        let mut logical = Vec::new();
        logical_devices(&snap.devices, &mut logical);
        let topology = Arc::clone(&snap.system.cpu_topology);
        let grid = CpuGrid::new(
            &hub,
            Arc::clone(&topology),
            &logical,
            settings.kernel,
            settings.range_ns,
            settings.smooth,
            &theme,
            cx,
        );
        let mut words = Words::default();
        words.refresh(cx);
        let spec = overall_spec(row.idx, &settings, &theme);
        let labels = overall_labels(settings.kernel, &words);
        let overall = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));

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
            overall,
            grid,
            overlaid: None,
            stacked: None,
            fade: None,
            fade_generation: 0,
            area_width: Rc::new(Cell::new(px(0.))),
            secondary: None,
            details: cpu_details_model(),
            mode: settings.mode,
            kernel: settings.kernel,
            #[cfg(feature = "bench")]
            bench_show_both: false,
            mode_button_focus: cx.focus_handle(),
            kernel_button_focus: cx.focus_handle(),
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
            mode_button_bounds: Rc::default(),
            catalog_version: snap.devices.version,
            topology,
            logical,
            static_info: Arc::default(),
            temp_sensors: Vec::new(),
            clocks: Arc::default(),
            clock_buf: Vec::new(),
            total: Metric::default(),
            total_text: String::new(),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        cpu_temp_sensors(&snap.devices, &mut this.temp_sensors);
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this.ensure_mode_chart(cx);
        this
    }

    pub fn overlaid(&self) -> Option<&Entity<ChartView>> {
        self.overlaid.as_ref()
    }

    pub fn stacked(&self) -> Option<&Entity<ChartView>> {
        self.stacked.as_ref()
    }

    /// The mode the graph area is fading from, while a mode change cross-fades.
    pub fn fading_from(&self) -> Option<CpuGraphMode> {
        self.fade.map(|f| f.from)
    }

    /// The kernel toggle does nothing in the all-cores modes (UI-PRF-023).
    pub fn kernel_enabled(&self) -> bool {
        !matches!(
            self.mode,
            CpuGraphMode::AllCoresOverlaid | CpuGraphMode::AllCoresStacked
        )
    }

    /// Creates the overlaid or stacked chart the first time its mode is shown.
    fn ensure_mode_chart(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let s = self.settings.view();
        match self.mode {
            CpuGraphMode::AllCoresOverlaid if self.overlaid.is_none() => {
                let (spec, labels) = (
                    overlaid_spec(&self.logical, &s, &theme),
                    all_cores_labels(false, &self.words),
                );
                let hub = self.hub.clone();
                self.overlaid = Some(cx.new(|cx| ChartView::new(spec, labels, &hub, cx)));
                self.sync_total_text(cx);
            }
            CpuGraphMode::AllCoresStacked if self.stacked.is_none() => {
                let (spec, labels) = (
                    stacked_spec(&self.logical, &s, &theme),
                    all_cores_labels(true, &self.words),
                );
                let hub = self.hub.clone();
                self.stacked = Some(cx.new(|cx| ChartView::new(spec, labels, &hub, cx)));
                self.sync_total_text(cx);
            }
            _ => {}
        }
    }

    /// The overlaid and stacked charts show overall CPU utilization as their value, the number
    /// the Overall chart shows, not CPU 0's. A chart gets new text only when it differs.
    fn sync_total_text(&mut self, cx: &mut App) {
        let text = match self.total.validity {
            Validity::Valid | Validity::Stale => {
                self.total_text.clear();
                format_value(
                    ValueFormat::Percent,
                    self.total.value,
                    NetworkUnit::default(),
                    &mut self.total_text,
                );
                Some(self.total_text.as_str())
            }
            _ => None,
        };
        for view in self.overlaid.iter().chain(&self.stacked) {
            if view.read(cx).value_text().map(|t| t.as_ref()) != text {
                let text = text.map(|t| SharedString::from(t.to_owned()));
                view.update(cx, |v, cx| v.set_value_text(text, cx));
            }
        }
    }

    /// New specs for the overlaid and stacked charts that exist (hotplug, settings, theme).
    fn rebuild_mode_specs(&self, cx: &mut App) {
        let theme = current(cx).clone();
        let s = self.settings.view();
        if let Some(v) = &self.overlaid {
            let spec = overlaid_spec(&self.logical, &s, &theme);
            v.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        if let Some(v) = &self.stacked {
            let spec = stacked_spec(&self.logical, &s, &theme);
            v.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }

    /// Starts the cross-fade from `from` to the current mode, or nothing when animations are
    /// off. A timer ends it after `m_base`, so no frame is drawn for it afterwards.
    fn start_fade(&mut self, from: CpuGraphMode, cx: &mut Context<Self>) {
        self.fade_generation = self.fade_generation.wrapping_add(1);
        if !animations_on(cx) {
            self.fade = None;
            return;
        }
        let generation = self.fade_generation;
        self.fade = Some(ModeFade { from, generation });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(M_BASE).await;
            let _ = this.update(cx, |this, cx| {
                if this.fade_generation == generation {
                    this.fade = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The graph area height of `mode`: 300 for the single charts, the grid's height for the
    /// width of the last paint in per-core mode.
    fn area_height(&self, mode: CpuGraphMode, scale: f32, cx: &App) -> Pixels {
        match mode {
            CpuGraphMode::PerCore => {
                let spec = &self.grid.view.read(cx).spec;
                let w = self.area_width.get();
                let w = if w > px(0.) { w } else { px(1.0e6) };
                let layout = grid_layout(spec.cells.len(), w, spec.cell_min_w, spec.gap);
                grid_height(&layout, spec.cell_h, spec.gap)
            }
            _ => px(GRAPH_H_OVERALL * scale),
        }
    }

    /// The graph area of `mode`.
    fn mode_element(&self, mode: CpuGraphMode, window: &mut Window, cx: &mut App) -> AnyElement {
        let scale = metrics(cx).ui_scale;
        let chart = |id: &'static str, view: Option<&Entity<ChartView>>| {
            view.map(|v| Self::chart_element(id, v, scale))
        };
        let el = match mode {
            CpuGraphMode::PerCore => return self.grid.element(window, cx),
            CpuGraphMode::Overall => None,
            CpuGraphMode::AllCoresOverlaid => chart(OVERLAID_ID, self.overlaid.as_ref()),
            CpuGraphMode::AllCoresStacked => chart(STACKED_ID, self.stacked.as_ref()),
        };
        el.unwrap_or_else(|| self.overall_element(scale))
    }

    /// `view` in a full-width box of `GRAPH_H_OVERALL`.
    fn chart_element(id: &'static str, view: &Entity<ChartView>, scale: f32) -> AnyElement {
        div()
            .debug_selector(move || id.into())
            .w_full()
            .h(px(GRAPH_H_OVERALL * scale))
            .child(view.clone().cached(StyleRefinement::default().size_full()))
            .into_any_element()
    }

    pub fn mode(&self) -> CpuGraphMode {
        self.mode
    }

    pub fn kernel(&self) -> bool {
        self.kernel
    }

    pub fn overall(&self) -> &Entity<ChartView> {
        &self.overall
    }

    pub fn grid(&self) -> &CpuGrid {
        &self.grid
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// This page's demand part while it is active: `PerCore` in every mode but Overall, and
    /// `cpu_fast` whatever `smooth` is (R-126).
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            cpu: match self.mode {
                CpuGraphMode::Overall => CpuDemand::Total,
                _ => CpuDemand::PerCore,
            },
            cpu_fast: true,
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

    /// Brings the grid topology, Details rows, chart state and header up to date with the
    /// components in `mask`. Notifies only when something the page draws changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let mut changed = false;
        let devices = mask.contains(ComponentMask::DEVICES);
        let system = mask.contains(ComponentMask::SYSTEM);

        if devices && snap.devices.version != self.catalog_version {
            self.catalog_version = snap.devices.version;
            cpu_temp_sensors(&snap.devices, &mut self.temp_sensors);
            logical_devices(&snap.devices, &mut self.logical);
        }
        let new_cells =
            devices || (system && !Arc::ptr_eq(&self.topology, &snap.system.cpu_topology));
        if new_cells {
            self.topology = Arc::clone(&snap.system.cpu_topology);
            self.grid
                .set_topology(Arc::clone(&self.topology), &self.logical, cx);
            self.rebuild_mode_specs(cx);
        }
        if system || new_cells {
            self.sync_clocks(&snap.system.cpu.per_cpu_mhz, new_cells, cx);
        }
        if system {
            let inputs = CpuLiveInputs {
                now_boot_ns: now_boot_ns(),
                cpu_error: snap.health.get(SourceId::Cpu).last_error.as_deref(),
                temp_sensors: &self.temp_sensors,
            };
            changed |= update_live(&mut self.details, &snap.system, &inputs);
            if snap.system.cpu.util_pct != self.total {
                self.total = snap.system.cpu.util_pct;
                self.sync_total_text(cx);
            }
        }
        if mask.contains(ComponentMask::DEVICE_DETAILS)
            && !Arc::ptr_eq(&self.static_info, &snap.device_details.cpu)
        {
            self.static_info = Arc::clone(&snap.device_details.cpu);
            changed |= update_static(&mut self.details, &self.static_info);
        }
        if mask.contains(ComponentMask::HEALTH) {
            let state = health_state(snap);
            self.overall.update(cx, |v, cx| v.set_state(state, cx));
        }
        if (devices || mask.contains(ComponentMask::INVENTORY))
            && let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx)
        {
            let header = header_text(page_words(cx), DeviceKind::Cpu, 0, entry, snap);
            if header != self.header {
                self.header = header;
                changed = true;
            }
        }
        if changed {
            cx.notify();
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
        let old = self.settings;
        if new == old {
            return;
        }
        self.settings = new;
        if new.kernel != old.kernel {
            self.kernel = new.kernel;
            self.grid.set_kernel(new.kernel, cx);
            let labels = overall_labels(new.kernel, &self.words);
            self.overall.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        self.grid.set_range(new.range_ns, cx);
        self.grid.set_smooth(new.smooth, cx);
        let theme = current(cx).clone();
        let spec = overall_spec(self.row.idx, &new, &theme);
        self.overall.update(cx, |v, cx| v.set_spec(spec, cx));
        if new.view() != old.view() {
            self.rebuild_mode_specs(cx);
        }
        if new.mode != old.mode {
            self.mode = new.mode;
            self.ensure_mode_chart(cx);
            self.start_fade(old.mode, cx);
            self.send_demand(cx);
        }
        cx.notify();
    }

    /// A new theme or UI scale: the grid reads both when it is built, so it is built again, and
    /// the Overall chart takes the new colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        let theme = current(cx).clone();
        let s = self.settings;
        self.grid = CpuGrid::new(
            &self.hub,
            Arc::clone(&self.topology),
            &self.logical,
            s.kernel,
            s.range_ns,
            s.smooth,
            &theme,
            cx,
        );
        let clocks = Arc::clone(&self.clocks);
        self.sync_clocks(&clocks, true, cx);
        let spec = overall_spec(self.row.idx, &s, &theme);
        self.overall.update(cx, |v, cx| v.set_spec(spec, cx));
        self.rebuild_mode_specs(cx);
        cx.notify();
    }

    /// Hands the per-CPU clocks to the grid when the snapshot has a new `Arc`, or always with
    /// `force` (new grid cells). The grid re-formats only the tooltips whose MHz changed.
    fn sync_clocks(&mut self, clocks: &Arc<[Metric<u32>]>, force: bool, cx: &mut App) {
        if !force && Arc::ptr_eq(&self.clocks, clocks) {
            return;
        }
        self.clocks = Arc::clone(clocks);
        self.clock_buf.clear();
        self.clock_buf
            .extend(clocks.iter().enumerate().map(|(id, m)| {
                (
                    u16::try_from(id).unwrap_or(u16::MAX),
                    m.value as f32,
                    m.validity,
                )
            }));
        self.grid.update_clocks(&self.clock_buf, cx);
    }

    fn on_set_mode(&mut self, a: &PerfSetCpuGraphMode, _: &mut Window, cx: &mut Context<Self>) {
        let allowed = mode_items().iter().any(|i| i.mode == a.mode && i.enabled);
        if !allowed {
            return;
        }
        let mode = a.mode;
        TlSettings::update(cx, |d| d.settings.performance.cpu_graph_mode = mode);
        self.sync_settings(cx);
    }

    fn on_toggle_kernel(
        &mut self,
        _: &PerfToggleCpuKernel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        TlSettings::update(cx, |d| {
            d.settings.performance.cpu_kernel = !d.settings.performance.cpu_kernel;
        });
        self.sync_settings(cx);
    }

    /// The Overall chart in a full-width box of `GRAPH_H_OVERALL`.
    fn overall_element(&self, scale: f32) -> AnyElement {
        div()
            .debug_selector(|| OVERALL_ID.into())
            .w_full()
            .h(px(GRAPH_H_OVERALL * scale))
            .child(
                self.overall
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .into_any_element()
    }
}

impl DevicePage for CpuPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 2]> {
        let m = *metrics(cx);
        let w = &self.words;
        let mode = self.mode;
        let menu_focus = self.focus.clone();
        let (labels, later) = (w.modes.clone(), w.later.clone());
        let bounds = Rc::clone(&self.mode_button_bounds);
        // A dropdown trigger has no action of its own: the click opens the menu below the
        // button, and its items dispatch `PerfSetCpuGraphMode` on the page.
        let mode_button = TlButton::new(MODE_BUTTON_ID, TlButtonKind::Secondary)
            .size(TlButtonSize::Sm)
            .trailing_icon(TlIcon::ChevronDown)
            .icon_size(m.icon.sm)
            .label(w.mode(mode).clone())
            .tooltip(w.tip_mode.clone())
            .focus_handle(self.mode_button_focus.clone())
            .on_click(move |ev, window, cx| {
                let b = bounds.get();
                let at = if b.size.width > px(0.) {
                    b.bottom_left()
                } else {
                    ev.position()
                };
                let (labels, later) = (labels.clone(), later.clone());
                let menu = TlMenu::build(window, cx, menu_focus.clone(), move |menu, _, _| {
                    mode_menu(menu, mode, &labels, &later)
                });
                open_context_menu_at(at, window, cx, menu);
            });
        let measure = {
            let bounds = Rc::clone(&self.mode_button_bounds);
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .inset_0()
        };
        // Column direction keeps the button stretched to the frame's toggle width.
        let mode_button = v_flex().relative().child(mode_button).child(measure);
        // Dispatched on the page, so it works wherever focus is.
        let page_focus = self.focus.clone();
        let kernel_button =
            TlButton::new(KERNEL_BUTTON_ID, TlButtonKind::Segment { on: self.kernel })
                .label(w.kernel_toggle(self.kernel).clone())
                .tooltip(w.tip_kernel.clone())
                .disabled(!self.kernel_enabled())
                .focus_handle(self.kernel_button_focus.clone())
                .on_click(move |_, window, cx| {
                    page_focus.dispatch_action(&PerfToggleCpuKernel, window, cx);
                });
        smallvec![
            mode_button.into_any_element(),
            kernel_button.into_any_element()
        ]
    }

    fn graphs(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        #[cfg(feature = "bench")]
        if self.bench_show_both {
            return v_flex()
                .w_full()
                .child(self.overall_element(m.ui_scale))
                .child(div().h(m.sp[4]))
                .child(self.grid.element(window, cx))
                .into_any_element();
        }
        let measure = {
            let cell = Rc::clone(&self.area_width);
            canvas(move |b, _, _| cell.set(b.size.width), |_, _, _, _| {})
                .absolute()
                .inset_0()
        };
        let Some(ModeFade { from, generation }) = self.fade else {
            return div()
                .relative()
                .w_full()
                .child(self.mode_element(self.mode, window, cx))
                .child(measure)
                .into_any_element();
        };
        // Cross-fade: both areas in one box whose height moves from the old to the new one.
        let (h0, h1) = (
            self.area_height(from, m.ui_scale, cx),
            self.area_height(self.mode, m.ui_scale, cx),
        );
        let anim = || Animation::new(M_BASE).with_easing(ease_out_cubic);
        let id = |part: &'static str| (part, generation as usize);
        let old = div()
            .debug_selector(|| FADE_OLD_ID.into())
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .child(self.mode_element(from, window, cx))
            .with_animation(id("perf-cpu-fade-old"), anim(), |el, t| el.opacity(1. - t));
        let new = div()
            .debug_selector(|| FADE_NEW_ID.into())
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .child(self.mode_element(self.mode, window, cx))
            .with_animation(id("perf-cpu-fade-new"), anim(), |el, t| el.opacity(t));
        div()
            .relative()
            .w_full()
            .overflow_hidden()
            .child(old)
            .child(new)
            .child(measure)
            .with_animation(id("perf-cpu-fade-h"), anim(), move |el, t| {
                el.h(h0 + (h1 - h0) * t)
            })
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    fn menu_items(&self, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
        let w = &self.words;
        let mode = self.mode;
        let (labels, later) = (w.modes.clone(), w.later.clone());
        // E24-021: the "Secondary graph >" submenu.
        menu.submenu(w.menu_graph.clone(), window, cx, move |menu, _, _| {
            mode_menu(menu, mode, &labels, &later)
        })
        .check_item(
            w.menu_kernel_times.clone(),
            self.kernel,
            Box::new(PerfToggleCpuKernel),
        )
    }

    /// E24-016 replaces this with the full CPU copy text.
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
        smallvec![
            self.mode_button_focus.clone(),
            self.kernel_button_focus.clone()
        ]
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

impl Focusable for CpuPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CpuPage {
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
        self.grid.refresh_strings(cx);
        if self.words.refresh(cx) {
            let labels = overall_labels(self.kernel, &self.words);
            self.overall.update(cx, |v, cx| v.set_labels(labels, cx));
            let snap = Arc::clone(self.hub.read(cx).current());
            self.apply_snapshot(&snap, ComponentMask::INVENTORY, cx);
        }
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_set_mode))
            .on_action(cx.listener(Self::on_toggle_kernel))
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
