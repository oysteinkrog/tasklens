//! The CPU page: the per-core grid, the Overall chart, the all-cores charts or the total chart
//! beside the core board (crw.104), the secondary chart, the mode and kernel toggles, the
//! Details grid, and the demand that switches the engine to 10 Hz per-core sampling
//! (docs/ui/performance.md UI-PRF-023, UI-PRF-025, UI-PRF-053; PERF-027 to PERF-033,
//! PERF-038 to PERF-040; ADR-008, ADR-014; R-126).
//!
//! The page body is one widget container (ADR-019 phase 3, `tasklens-hzire.5`). By default it
//! holds the 1.3.0 parts in their 1.3.0 places (`page_flow::cpu`): the per-core grid
//! ([`CpuCoreGridView`] as the fixed widget `cpu_core_grid`), the secondary chart (Power draw
//! with its two fallbacks, Temperature and Clock speed) and the Details widget. The mode button
//! and "Graph >" pick the page's preset: `overall` (the total chart), `per_core`,
//! `all_cores_overlaid`, `all_cores_stacked` and `board` (the total chart beside the core board,
//! [`CpuBoard`] as the fixed widget `cpu_core_board`). "Secondary graph >" replaces the
//! secondary widgets. Edit Widgets adds the total, the kernel time, each core or all cores, the
//! clock speed, the temperature and the power as chart, bar, value, stats or any mix.
//!
//! `page.cpu` converts `cpu_graph_mode` and `cpu_secondary_graph` once, at load
//! (`Settings::clamp`). The header buttons and the page menu edit the page's flow layout and
//! never write those keys; a page without an entry gets one. With a grid layout the mode items
//! ask "Replace your widgets?" first and "Secondary graph >" is hidden. The kernel toggle stays
//! `performance.cpu_kernel`, which every CPU chart follows.
//!
//! Render contract: the page renders on a header change, a header button change, a range
//! change and a layout change, never because of a sample. The grid, the board, the charts and
//! Details are views of their own that notify only for their own changes. The grid is one
//! `ChartGridView` (ADR-009 rule 7).
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, EntityId,
    FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Subscription, Window, canvas, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::health::SourceId;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{CpuStaticInfo, Snapshot};
use tl_model::system::{CpuTopology, Metric};
use tl_model::validity::Validity;
use tl_ui_settings::layout::{
    ContainerLayout, FixedWidget, FormSet, GroupKind, METRIC_TEMP, WidgetContent, WidgetOptions,
    WidgetSetting, container_layout,
};
use tl_ui_settings::layout_boxes::{
    DeviceKind as SourceKind, METRIC_CORE_CLOCKS, METRIC_CPU_TOTAL, METRIC_POWER_DRAW,
};
use tl_ui_settings::layout_pages::{
    METRIC_CLOCK, METRIC_CORES_OVERLAID, METRIC_CORES_STACKED, PRESET_ALL_CORES_OVERLAID,
    PRESET_ALL_CORES_STACKED, PRESET_BOARD, PRESET_OVERALL, PRESET_PER_CORE, convert_cpu_page,
    cpu_mode_widgets, cpu_preset, cpu_secondary_widgets, page_container,
};
use tl_ui_settings::settings::{CpuGraphMode, CpuSecondaryGraph, PerformanceSettings};

use super::actions::{
    PerfSetCpuGraphMode, PerfSetCpuSecondaryGraph, PerfToggleCpuKernel, PerfWidgetEditStart,
    PerfWidgetResetPage,
};
use super::aggregate::{add_demand, page_layout, reset_page, start_page_edit, strings_version};
pub(crate) use super::core::logical_devices;
// Moved to `core/` (tasklens-zljr5.22). These keep the old paths working.
pub use super::core::cpu::{PowerState, SecondaryAvail};
pub(crate) use super::core::look::{OVERLAID_MIN_ALPHA, STACKED_ALPHAS};
use super::core_clocks::core_lines;
use super::cpu_board::CpuBoard;
use super::cpu_details::{
    CpuLiveInputs, CpuSensor, cpu_details_model, cpu_temp_sensors, update_live, update_static,
};
use super::cpu_grid::{CpuCoreGridView, CpuGrid};
use super::details::KeyValueModel;
use super::gpu_views::PartResized;
use super::page::{
    DevicePage, DevicePageFrame, details_draw, header_text, page_history_request, page_range_ns,
    page_words,
};
use super::page_flow::cpu::{CpuParts, cpu_flow, secondary_metric, shown_secondary};
use super::widget::container::WidgetContainer;
use super::widget::details_widget::{DetailsResized, DetailsShown, DetailsView};
use super::widget::edit::{WidgetEditor, default_layout};
use super::widget::fixed_widget::FixedContent;
use super::widget::menu::confirm_preset_page;
use super::widget::source::SourceStore;
use super::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use tl_ui_base::i18n::{Strings, tr, tr_args};
use tl_ui_base::settings_global::{Section, TlSettings};
use tl_ui_base::theme::{TlTheme, UiMetrics, current, metrics};
use tl_ui_chart::chart::{ChartBg, ChartSeries, ChartSpec, ChartView, GridStyle, YScale};
use tl_ui_kit::actions::OpenAdditionalSetup;
use tl_ui_kit::components::button::{TlButton, TlButtonKind, TlButtonSize};
use tl_ui_kit::components::icons::TlIcon;
use tl_ui_kit::components::menu::{TlMenu, open_context_menu_at};

/// The overlaid and stacked modes are in the mode menu (E24-018).
pub const CPU_EXTRA_MODES_ENABLED: bool = true;

/// Element id of the mode button.
pub const MODE_BUTTON_ID: &str = "perf-cpu-mode";
/// Element id of the kernel toggle.
pub const KERNEL_BUTTON_ID: &str = "perf-cpu-kernel";

const KEY_OVERALL: &str = "perf-cpu-mode-overall";
const KEY_PER_CORE: &str = "perf-cpu-mode-per-core";
const KEY_OVERLAID: &str = "perf-cpu-mode-overlaid";
const KEY_STACKED: &str = "perf-cpu-mode-stacked";
const KEY_BOARD: &str = "perf-cpu-mode-board";
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
const KEY_MENU_SECONDARY: &str = "perf-cpu-menu-secondary";
const KEY_SECONDARY_NONE: &str = "perf-cpu-secondary-none";
const KEY_SECONDARY_POWER: &str = "perf-cpu-secondary-power";
const KEY_SECONDARY_CLOCK: &str = "perf-cpu-secondary-clock";
const KEY_SECONDARY_CLOCKS: &str = "perf-cpu-secondary-clocks";
const KEY_SECONDARY_TEMP: &str = "perf-cpu-secondary-temp";
/// `{$shown}`: the name of the chart shown in place of Power draw.
const KEY_SECONDARY_FALLBACK: &str = "perf-cpu-secondary-fallback";
const KEY_SETUP_POWER: &str = "perf-cpu-setup-power";
const KEY_NEEDS_SETUP: &str = "perf-tip-needs-setup";

/// One entry of the mode menu. `label` and `tooltip` are `en-US.ftl` keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeItem {
    pub mode: CpuGraphMode,
    pub label: &'static str,
    pub enabled: bool,
    /// Why the item is disabled.
    pub tooltip: Option<&'static str>,
}

/// The five mode items in menu order. Overlaid and stacked are disabled until
/// [`CPU_EXTRA_MODES_ENABLED`].
pub const fn mode_items() -> [ModeItem; 5] {
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
        item(CpuGraphMode::Board, false),
    ]
}

/// The `en-US.ftl` key of `mode`'s name.
pub const fn mode_key(mode: CpuGraphMode) -> &'static str {
    match mode {
        CpuGraphMode::Overall => KEY_OVERALL,
        CpuGraphMode::PerCore => KEY_PER_CORE,
        CpuGraphMode::AllCoresOverlaid => KEY_OVERLAID,
        CpuGraphMode::AllCoresStacked => KEY_STACKED,
        CpuGraphMode::Board => KEY_BOARD,
    }
}

/// The index of `mode` in [`mode_items`].
const fn mode_index(mode: CpuGraphMode) -> usize {
    match mode {
        CpuGraphMode::Overall => 0,
        CpuGraphMode::PerCore => 1,
        CpuGraphMode::AllCoresOverlaid => 2,
        CpuGraphMode::AllCoresStacked => 3,
        CpuGraphMode::Board => 4,
    }
}

/// The chart shown for the saved choice `saved`, and whether it stands in for Power draw
/// (PERF-032). Power falls back to Temperature, then Clock speed. A choice that is not
/// available, and None, show no chart.
pub fn effective_secondary(
    saved: CpuSecondaryGraph,
    avail: SecondaryAvail,
) -> Option<(CpuSecondaryGraph, bool)> {
    use CpuSecondaryGraph as G;
    match saved {
        G::None => None,
        G::Power if avail.power == PowerState::Readable => Some((G::Power, false)),
        G::Power if avail.temperature => Some((G::Temperature, true)),
        G::Power if avail.clock => Some((G::Clock, true)),
        G::Power => None,
        G::Clock => avail.clock.then_some((G::Clock, false)),
        G::Clocks => avail.clock.then_some((G::Clocks, false)),
        G::Temperature => avail.temperature.then_some((G::Temperature, false)),
    }
}

/// One radio item of the "Secondary graph" submenu. `label` and `tooltip` are `en-US.ftl` keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecondaryItem {
    pub choice: CpuSecondaryGraph,
    pub label: &'static str,
    pub enabled: bool,
    /// Why the item is disabled.
    pub tooltip: Option<&'static str>,
    /// "Set up CPU power draw…" follows the item (UI-PRF-025).
    pub setup_after: bool,
}

/// The five items in menu order. Power draw is disabled with "Needs setup" when RAPL is
/// unreadable, and the setup item follows it; Temperature is disabled without a CPU hwmon.
pub fn secondary_items(avail: SecondaryAvail) -> [SecondaryItem; 5] {
    use CpuSecondaryGraph as G;
    let needs_setup = avail.power == PowerState::NeedsSetup;
    let item = |choice, enabled| SecondaryItem {
        choice,
        label: secondary_key(choice),
        enabled,
        tooltip: None,
        setup_after: false,
    };
    [
        item(G::None, true),
        SecondaryItem {
            tooltip: needs_setup.then_some(KEY_NEEDS_SETUP),
            setup_after: needs_setup,
            ..item(G::Power, !needs_setup)
        },
        item(G::Clock, true),
        item(G::Clocks, true),
        item(G::Temperature, avail.temperature),
    ]
}

/// The `en-US.ftl` key of a secondary chart's name, also its heading.
pub const fn secondary_key(choice: CpuSecondaryGraph) -> &'static str {
    match choice {
        CpuSecondaryGraph::None => KEY_SECONDARY_NONE,
        CpuSecondaryGraph::Power => KEY_SECONDARY_POWER,
        CpuSecondaryGraph::Clock => KEY_SECONDARY_CLOCK,
        CpuSecondaryGraph::Clocks => KEY_SECONDARY_CLOCKS,
        CpuSecondaryGraph::Temperature => KEY_SECONDARY_TEMP,
    }
}

const fn secondary_index(choice: CpuSecondaryGraph) -> usize {
    match choice {
        CpuSecondaryGraph::None => 0,
        CpuSecondaryGraph::Power => 1,
        CpuSecondaryGraph::Clock => 2,
        CpuSecondaryGraph::Clocks => 3,
        CpuSecondaryGraph::Temperature => 4,
    }
}

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    /// In [`mode_items`] order.
    pub modes: [SharedString; 5],
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
    pub menu_secondary: SharedString,
    /// In [`secondary_index`] order: None, Power draw, Clock speed, Core clocks, Temperature.
    pub secondary: [SharedString; 5],
    pub setup_power: SharedString,
    pub needs_setup: SharedString,
    /// "Power draw is not available. Showing Temperature." and "… Showing Clock speed."
    pub fallback_temp: SharedString,
    pub fallback_clock: SharedString,
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
        self.menu_secondary = t(KEY_MENU_SECONDARY);
        self.secondary = [
            CpuSecondaryGraph::None,
            CpuSecondaryGraph::Power,
            CpuSecondaryGraph::Clock,
            CpuSecondaryGraph::Clocks,
            CpuSecondaryGraph::Temperature,
        ]
        .map(|c| t(secondary_key(c)));
        self.setup_power = t(KEY_SETUP_POWER);
        self.needs_setup = t(KEY_NEEDS_SETUP);
        let fallback = |shown: &SharedString| {
            tr_args(
                cx,
                KEY_SECONDARY_FALLBACK,
                &[("shown", shown.to_string().into())],
            )
        };
        self.fallback_temp = fallback(&self.secondary[4]);
        self.fallback_clock = fallback(&self.secondary[2]);
        true
    }

    pub fn secondary(&self, choice: CpuSecondaryGraph) -> &SharedString {
        &self.secondary[secondary_index(choice)]
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

/// Adds the five mode radios, `current` checked (none with a grid layout); a mode not yet
/// available is a disabled item with its tooltip.
fn mode_menu(
    mut menu: TlMenu,
    current: Option<CpuGraphMode>,
    labels: &[SharedString; 5],
    later: &SharedString,
) -> TlMenu {
    for (item, label) in mode_items().into_iter().zip(labels) {
        menu = if item.enabled {
            menu.radio_item(
                label.clone(),
                Some(item.mode) == current,
                Box::new(PerfSetCpuGraphMode { mode: item.mode }),
            )
        } else {
            menu.item_disabled(label.clone(), Some(later.clone()))
        };
    }
    menu
}

/// Adds the four secondary chart radios and, under a Power draw that needs setup, the item that
/// opens Additional Setup.
fn secondary_menu(
    mut menu: TlMenu,
    saved: CpuSecondaryGraph,
    avail: SecondaryAvail,
    words: &Words,
) -> TlMenu {
    for item in secondary_items(avail) {
        let label = words.secondary(item.choice).clone();
        menu = if item.enabled {
            menu.radio_item(
                label,
                item.choice == saved,
                Box::new(PerfSetCpuSecondaryGraph {
                    choice: item.choice,
                }),
            )
        } else {
            let tip = item.tooltip.map(|_| words.needs_setup.clone());
            menu.item_disabled(label, tip)
        };
        if item.setup_after {
            menu = menu.item(
                words.setup_power.clone(),
                Box::new(OpenAdditionalSetup { focus_row: None }),
            );
        }
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

// ---- The page's flow layout (ADR-019 phase 3 "Header buttons and page menu") ----

/// The graph mode of the CPU page preset `id`; `None` for an id that is not one.
pub fn preset_mode(id: &str) -> Option<CpuGraphMode> {
    Some(match id {
        PRESET_OVERALL => CpuGraphMode::Overall,
        PRESET_PER_CORE => CpuGraphMode::PerCore,
        PRESET_ALL_CORES_OVERLAID => CpuGraphMode::AllCoresOverlaid,
        PRESET_ALL_CORES_STACKED => CpuGraphMode::AllCoresStacked,
        PRESET_BOARD => CpuGraphMode::Board,
        _ => return None,
    })
}

/// The graph mode of `layout`: its preset's, `None` for a grid layout.
pub fn layout_mode(layout: &ContainerLayout) -> Option<CpuGraphMode> {
    layout.preset.as_ref().and_then(|p| preset_mode(&p.id))
}

/// Whether `w` is a metric widget of `metric`.
fn is_metric(w: &WidgetSetting, metric: &str) -> bool {
    matches!(&w.content, WidgetContent::Metric(s) if s.metric.as_str() == metric)
}

/// Whether `w` is one of a graph mode's widgets: the grid, the board, the total chart or an
/// all-cores chart.
fn is_mode_widget(w: &WidgetSetting) -> bool {
    match &w.content {
        WidgetContent::Fixed(f) => {
            matches!(f, FixedWidget::CpuCoreGrid | FixedWidget::CpuCoreBoard)
        }
        WidgetContent::Metric(_) => [
            METRIC_CPU_TOTAL,
            METRIC_CORES_OVERLAID,
            METRIC_CORES_STACKED,
        ]
        .iter()
        .any(|m| is_metric(w, m)),
        WidgetContent::Group(_) => false,
    }
}

/// Inserts `widgets` into `layout` at `at`, with new ids.
fn insert_widgets(layout: &mut ContainerLayout, at: usize, widgets: Vec<(WidgetContent, FormSet)>) {
    let at = at.min(layout.widgets.len());
    for (i, (content, forms)) in widgets.into_iter().enumerate() {
        let id = layout.next_id;
        layout.next_id += 1;
        layout.widgets.insert(
            at + i,
            WidgetSetting {
                id,
                content,
                forms,
                cell: None,
                options: WidgetOptions::default(),
                device_name: None,
            },
        );
    }
}

/// Sets the preset of the flow layout `layout` to `mode`'s and replaces the mode's widgets with
/// the new preset's, first in the list. The secondary widgets, Details and the other widgets
/// keep their ids. Nothing for a grid layout.
pub fn set_cpu_preset(layout: &mut ContainerLayout, mode: CpuGraphMode) {
    let Some(preset) = layout.preset.as_mut() else {
        return;
    };
    let id = cpu_preset(mode);
    if preset.id == id {
        return;
    }
    preset.id = id.to_owned();
    layout.widgets.retain(|w| !is_mode_widget(w));
    insert_widgets(layout, 0, cpu_mode_widgets(id));
}

/// Replaces the secondary widgets of the flow layout `layout` with those of `graph`, where the
/// old ones were, else before Details. Nothing for a grid layout.
pub fn set_cpu_secondary(layout: &mut ContainerLayout, graph: CpuSecondaryGraph) {
    if layout.preset.is_none() || layout_secondary(layout) == graph {
        return;
    }
    let first = layout
        .widgets
        .iter()
        .position(|w| secondary_metric(w).is_some());
    layout.widgets.retain(|w| secondary_metric(w).is_none());
    let at = first.unwrap_or_else(|| {
        layout
            .widgets
            .iter()
            .position(|w| matches!(w.content, WidgetContent::Fixed(FixedWidget::Details)))
            .unwrap_or(layout.widgets.len())
    });
    insert_widgets(layout, at, cpu_secondary_widgets(graph));
}

/// The secondary graph `layout` lists: Power draw (with its fallbacks), Core clocks, Clock
/// speed, Temperature, or None.
pub fn layout_secondary(layout: &ContainerLayout) -> CpuSecondaryGraph {
    let has = |m| {
        layout
            .widgets
            .iter()
            .any(|w| secondary_metric(w) == Some(m))
    };
    if has(METRIC_POWER_DRAW) {
        CpuSecondaryGraph::Power
    } else if has(METRIC_CORE_CLOCKS) {
        CpuSecondaryGraph::Clocks
    } else if has(METRIC_CLOCK) {
        CpuSecondaryGraph::Clock
    } else if has(METRIC_TEMP) {
        CpuSecondaryGraph::Temperature
    } else {
        CpuSecondaryGraph::None
    }
}

/// The secondary graph a secondary widget of `metric` stands for.
fn secondary_of(metric: &str) -> CpuSecondaryGraph {
    match metric {
        METRIC_POWER_DRAW => CpuSecondaryGraph::Power,
        METRIC_TEMP => CpuSecondaryGraph::Temperature,
        METRIC_CORE_CLOCKS => CpuSecondaryGraph::Clocks,
        _ => CpuSecondaryGraph::Clock,
    }
}

/// `layout` with the total chart first (bench only, UI-PRF-053, R-160 OI-5: the Overall chart
/// above the grid).
#[cfg(feature = "bench")]
fn with_total(mut layout: ContainerLayout) -> ContainerLayout {
    if layout.preset.is_some()
        && !layout
            .widgets
            .iter()
            .any(|w| is_metric(w, METRIC_CPU_TOTAL))
    {
        insert_widgets(&mut layout, 0, cpu_mode_widgets(PRESET_OVERALL));
    }
    layout
}

/// The settings the page draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSettings {
    kernel: bool,
    /// The shared range the per-core grid draws.
    range_ns: u64,
    smooth: bool,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            kernel: s.performance.cpu_kernel,
            range_ns: page_range_ns(cx),
            smooth: s.motion.smooth_graphs,
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

/// The core board and its sort toggle's focus, made the first time a layout places the board.
type BoardSlot = Rc<RefCell<Option<(Entity<CpuBoard>, FocusHandle)>>>;

/// The core board of `slot`, made now when it has none. A hovered row highlights its core in
/// the page's Core clocks chart (tasklens-o55.57).
fn board_of(
    slot: &BoardSlot,
    hub: &Entity<SnapshotHub>,
    page: &gpui_kit::WeakEntity<CpuPage>,
    cx: &mut App,
) -> Entity<CpuBoard> {
    if let Some((board, _)) = &*slot.borrow() {
        return board.clone();
    }
    let focus = cx.focus_handle();
    let sort_focus = focus.clone();
    let board = cx.new(|cx| CpuBoard::new(hub, sort_focus, cx));
    let page = page.clone();
    board.read(cx).set_row_hover(move |cpu, cx| {
        let Some(page) = page.upgrade() else {
            return;
        };
        let Some(chart) = page.read(cx).chart_of(METRIC_CORE_CLOCKS, cx) else {
            return;
        };
        let snap = Arc::clone(page.read(cx).hub.read(cx).current());
        let mut logical = Vec::new();
        logical_devices(&snap.devices, &mut logical);
        let lines = core_lines(&snap.system.cpu_topology, &logical);
        let line = cpu.and_then(|c| lines.line_of_cpu.get(c).copied().flatten());
        chart.update(cx, |v, cx| v.set_highlight(line, cx));
    });
    *slot.borrow_mut() = Some((board.clone(), focus));
    board
}

/// The CPU device page.
pub struct CpuPage {
    key: DeviceKey,
    row: DeviceRow,
    /// The container id, `page.cpu`.
    container_id: SharedString,
    grid: CpuGrid,
    /// The per-core grid part (fixed widget `cpu_core_grid`).
    grid_view: Entity<CpuCoreGridView>,
    /// The core board (fixed widget `cpu_core_board`), once a layout placed it.
    board: BoardSlot,
    details: KeyValueModel,
    details_view: Entity<DetailsView>,
    container: Entity<WidgetContainer>,
    editor: Entity<WidgetEditor>,
    avail: SecondaryAvail,
    /// `avail`, shared with the flow placer.
    avail_cell: Rc<Cell<SecondaryAvail>>,
    /// The preset of the layout the page shows; `None` for a grid layout.
    mode: Option<CpuGraphMode>,
    /// The layout places the core board, so its sort toggle joins the focus order.
    board_placed: bool,
    kernel: bool,
    /// Bench only (UI-PRF-053, R-160 OI-5): the Overall chart above the grid.
    #[cfg(feature = "bench")]
    bench_show_both: bool,
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
    /// `UiMetrics.style_version` the grid's colours and sizes were built for.
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
    _subs: SmallVec<[Subscription; 8]>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl CpuPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let container_id = SharedString::from(page_container(&key.0));
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
        let grid_view = {
            let (id, view) = (container_id.clone(), grid.view.clone());
            cx.new(|_| CpuCoreGridView::new(id, view))
        };
        let mut details = cpu_details_model();
        details.refresh_strings(cx);
        let details_view = {
            let shown = DetailsShown::of(&details, strings_version(cx));
            let draw = details_draw(cx.weak_entity());
            let id = container_id.clone();
            cx.new(|_| DetailsView::new(id, shown, draw))
        };
        let avail = SecondaryAvail::read(&snap, &snap.device_details.cpu);
        let avail_cell = Rc::new(Cell::new(avail));
        let parts = CpuParts {
            grid: grid_view.read(cx).natural_height(),
            details: details_view.read(cx).natural_height(),
            avail: Rc::clone(&avail_cell),
        };
        let board: BoardSlot = Rc::default();
        let content: FixedContent = {
            let details: AnyView = details_view.clone().into();
            let grid: AnyView = grid_view.clone().into();
            let (slot, hub, page) = (Rc::clone(&board), hub.clone(), cx.weak_entity());
            Rc::new(move |kind: &FixedWidget, cx: &mut App| match kind {
                FixedWidget::Details => Some(details.clone()),
                FixedWidget::CpuCoreGrid => Some(grid.clone()),
                FixedWidget::CpuCoreBoard => Some(board_of(&slot, &hub, &page, cx).into()),
                _ => None,
            })
        };
        let layout = page_layout(&container_id, cx);
        let mode = layout_mode(&layout);
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::for_page(&hub)));
            let id = container_id.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut c = WidgetContainer::new(id, layout, &hub, store, cx);
                c.set_flow_placer(Some(cpu_flow(parts)), cx);
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
                    FixedWidget::CpuCoreGrid,
                    FixedWidget::CpuCoreBoard,
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
        subs.push(cx.subscribe(&grid_view, |this, _, _: &PartResized, cx| {
            this.place_again(cx);
        }));
        // A layout change can change what the page asks for and which chart stands in for
        // Power draw.
        subs.push(cx.observe(&container, |this, _, cx| this.on_container_changed(cx)));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            container_id,
            grid,
            grid_view,
            board,
            details,
            details_view,
            container,
            editor,
            avail,
            avail_cell,
            mode,
            board_placed: false,
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
            _subs: subs,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        cpu_temp_sensors(&snap.devices, &mut this.temp_sensors);
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this.on_container_changed(cx);
        this
    }

    /// The preset of the layout the page shows: its graph mode, `None` with a grid layout.
    pub fn mode(&self) -> Option<CpuGraphMode> {
        self.mode
    }

    pub fn kernel(&self) -> bool {
        self.kernel
    }

    /// The kernel toggle does nothing in the all-cores modes (UI-PRF-023).
    pub fn kernel_enabled(&self) -> bool {
        !matches!(
            self.mode,
            Some(CpuGraphMode::AllCoresOverlaid | CpuGraphMode::AllCoresStacked)
        )
    }

    pub fn grid(&self) -> &CpuGrid {
        &self.grid
    }

    /// The per-core grid part (fixed widget `cpu_core_grid`).
    pub fn grid_view(&self) -> &Entity<CpuCoreGridView> {
        &self.grid_view
    }

    /// The core board, once a layout has placed it.
    pub fn board(&self) -> Option<Entity<CpuBoard>> {
        self.board.borrow().as_ref().map(|(b, _)| b.clone())
    }

    pub fn words(&self) -> &Words {
        &self.words
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

    /// The container id, `page.cpu`.
    pub fn container_id(&self) -> &SharedString {
        &self.container_id
    }

    /// The id of the first widget of `metric` in the layout the page shows.
    pub fn widget_of(&self, metric: &str, cx: &App) -> Option<u32> {
        let c = self.container.read(cx);
        c.layout()
            .widgets
            .iter()
            .find(|w| is_metric(w, metric))
            .map(|w| w.id)
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

    /// The total chart (the Overall chart, and the chart beside the core board), while the
    /// page shows it.
    pub fn overall(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_CPU_TOTAL, cx)
    }

    /// The all cores overlaid chart, while the page shows it.
    pub fn overlaid(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_CORES_OVERLAID, cx)
    }

    /// The all cores stacked chart, while the page shows it.
    pub fn stacked(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_CORES_STACKED, cx)
    }

    /// The secondary widget that takes space, and whether it stands in for Power draw.
    fn shown_secondary(&self, cx: &App) -> Option<(u32, bool)> {
        let c = self.container.read(cx);
        c.layout().preset.as_ref()?;
        let placed = c.placed_ids();
        shown_secondary(c.layout().widgets.iter(), self.avail, |id| {
            placed.contains(&id)
        })
    }

    /// The secondary chart the page shows, if any.
    pub fn secondary(&self, cx: &App) -> Option<Entity<ChartView>> {
        let (id, _) = self.shown_secondary(cx)?;
        self.container
            .read(cx)
            .widget(id)?
            .read(cx)
            .chart()
            .cloned()
    }

    /// The chart the secondary area shows, if any.
    pub fn secondary_choice(&self, cx: &App) -> Option<CpuSecondaryGraph> {
        let (id, _) = self.shown_secondary(cx)?;
        let c = self.container.read(cx);
        let w = c.layout().widgets.iter().find(|w| w.id == id)?;
        secondary_metric(w).map(secondary_of)
    }

    /// The heading tooltip while the secondary chart stands in for Power draw.
    pub fn secondary_fallback_text(&self, cx: &App) -> Option<&SharedString> {
        let (id, fallback) = self.shown_secondary(cx)?;
        if !fallback {
            return None;
        }
        let c = self.container.read(cx);
        let w = c.layout().widgets.iter().find(|w| w.id == id)?;
        Some(self.fallback_text(secondary_metric(w)?))
    }

    /// "Power draw is not available. Showing …" for the chart of `metric`.
    fn fallback_text(&self, metric: &str) -> &SharedString {
        if metric == METRIC_TEMP {
            &self.words.fallback_temp
        } else {
            &self.words.fallback_clock
        }
    }

    pub fn secondary_avail(&self) -> SecondaryAvail {
        self.avail
    }

    /// Bench only: shows the Overall chart above the grid (`perf-cpu`, UI-PRF-053).
    #[cfg(feature = "bench")]
    pub fn set_bench_show_both(&mut self, on: bool, cx: &mut Context<Self>) {
        self.bench_show_both = on;
        self.sync_layout(cx);
    }

    #[cfg(feature = "bench")]
    pub fn bench_show_both(&self) -> bool {
        self.bench_show_both
    }

    /// This page's demand part while it is active: per-core samples while the grid, the board,
    /// an all-cores chart, Core clocks or a core widget shows, 10 Hz samples while the grid or
    /// the board shows (R-126), the sensors for the temperature, the device metric history at
    /// 1 h, 24 h and 7 d, and what the placed widgets read. With the default layout it equals
    /// 1.3.0's.
    pub fn demand_part(&self, cx: &App) -> Demand {
        let c = self.container.read(cx);
        let (mut per_core, mut fast, mut sensors) = (false, false, false);
        for id in c.placed_ids() {
            let Some(w) = c.layout().widgets.iter().find(|w| w.id == id) else {
                continue;
            };
            match &w.content {
                WidgetContent::Fixed(FixedWidget::CpuCoreGrid | FixedWidget::CpuCoreBoard) => {
                    per_core = true;
                    fast = true;
                }
                WidgetContent::Metric(s) => match SourceKind::of(&s.device) {
                    SourceKind::CpuLogical => per_core = true,
                    SourceKind::Cpu => match s.metric.as_str() {
                        METRIC_CORES_OVERLAID | METRIC_CORES_STACKED | METRIC_CORE_CLOCKS => {
                            per_core = true;
                        }
                        METRIC_TEMP => sensors = true,
                        _ => {}
                    },
                    _ => {}
                },
                WidgetContent::Group(g) if g.members == GroupKind::Cores => per_core = true,
                _ => {}
            }
        }
        let mut demand = Demand {
            any_surface_visible: true,
            cpu: if per_core {
                CpuDemand::PerCore
            } else {
                CpuDemand::Total
            },
            cpu_fast: fast,
            sensors,
            device_history_range: page_history_request(self.settings.range_ns),
            ..Demand::default()
        };
        add_demand(&mut demand, &c.demand_part());
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

    /// A part's height or what the secondary chart can show changed: the flow layout places
    /// the parts again.
    fn place_again(&mut self, cx: &mut Context<Self>) {
        self.container.update(cx, |c, cx| {
            c.place_again(cx);
        });
    }

    /// The container placed its widgets again or took a new layout: the demand, the focus
    /// order and the fallback note follow.
    fn on_container_changed(&mut self, cx: &mut Context<Self>) {
        let board = {
            let c = self.container.read(cx);
            let placed = c.placed_ids();
            c.layout().widgets.iter().any(|w| {
                placed.contains(&w.id)
                    && matches!(w.content, WidgetContent::Fixed(FixedWidget::CpuCoreBoard))
            })
        };
        self.board_placed = board;
        self.send_demand(cx);
        self.sync_fallback(cx);
    }

    /// Puts the 1.3.0 fallback note ("Power draw is not available. Showing Temperature.") on
    /// the heading of the secondary chart that stands in for Power draw, and takes it off the
    /// other secondary charts.
    fn sync_fallback(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown_secondary(cx);
        let updates: SmallVec<[_; 4]> = {
            let c = self.container.read(cx);
            c.layout()
                .widgets
                .iter()
                .filter_map(|w| {
                    let metric = secondary_metric(w)?;
                    let tip =
                        (shown == Some((w.id, true))).then(|| self.fallback_text(metric).clone());
                    Some((c.source(w.id)?.clone(), tip))
                })
                .collect()
        };
        for (source, tip) in updates {
            source.update(cx, |s, cx| s.set_page_tip(tip, cx));
        }
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

    /// Brings the grid topology, Details rows, what the secondary chart can show and the header
    /// up to date with the components in `mask`. Only a header change renders the page; each
    /// part notifies for its own change.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let devices = mask.contains(ComponentMask::DEVICES);
        let system = mask.contains(ComponentMask::SYSTEM);
        let mut details = false;

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
        }
        if system || new_cells {
            self.sync_clocks(&snap.system.cpu.per_cpu_mhz, new_cells, cx);
        }
        if system {
            let inputs = CpuLiveInputs {
                now_boot_ns: self.hub.read(cx).now_ns(),
                cpu_error: snap.health.get(SourceId::Cpu).last_error.as_deref(),
                temp_sensors: &self.temp_sensors,
            };
            details |= update_live(&mut self.details, &snap.system, &inputs);
        }
        if mask.contains(ComponentMask::DEVICE_DETAILS)
            && !Arc::ptr_eq(&self.static_info, &snap.device_details.cpu)
        {
            self.static_info = Arc::clone(&snap.device_details.cpu);
            details |= update_static(&mut self.details, &self.static_info);
            self.grid.set_prefcore(&self.static_info.prefcore_rank, cx);
        }
        if details {
            self.sync_details(cx);
        }
        if system || mask.intersects(ComponentMask::DEVICE_DETAILS | ComponentMask::INVENTORY) {
            let avail = SecondaryAvail::read(snap, &self.static_info);
            if avail != self.avail {
                self.avail = avail;
                self.avail_cell.set(avail);
                self.place_again(cx);
                self.sync_fallback(cx);
            }
        }
        if (devices || mask.contains(ComponentMask::INVENTORY))
            && let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx)
        {
            let header = header_text(page_words(cx), DeviceKind::Cpu, 0, entry, snap);
            if header != self.header {
                let word = header.0.clone();
                self.editor.update(cx, |e, _| e.set_page_name(word));
                self.header = header;
                self.notify(cx);
            }
        }
        // The CPU went away: an edit ends as Cancel does.
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

    /// The layout the page shows now: its saved entry, else its built-in default.
    fn shown_layout(&self, cx: &App) -> ContainerLayout {
        let layout = page_layout(&self.container_id, cx);
        #[cfg(feature = "bench")]
        if self.bench_show_both {
            return with_total(layout);
        }
        layout
    }

    /// Shows the saved layout (Apply, Reset Page, a menu item) when the page is not in Edit
    /// Widgets. The header follows a new preset.
    fn sync_layout(&mut self, cx: &mut Context<Self>) {
        if self.editor.read(cx).is_editing() {
            return;
        }
        let layout = self.shown_layout(cx);
        let mode = layout_mode(&layout);
        self.container.update(cx, |c, cx| c.set_layout(layout, cx));
        if mode != self.mode {
            self.mode = mode;
            self.notify(cx);
        }
    }

    /// Applies the page's settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        self.sync_layout(cx);
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
        self.grid.set_kernel(new.kernel, cx);
        self.grid.set_range(new.range_ns, cx);
        self.grid.set_smooth(new.smooth, cx);
        if new.range_ns != old.range_ns {
            self.send_demand(cx);
        }
        if new.kernel != old.kernel || new.range_ns != old.range_ns {
            // The kernel toggle and the header switch show the new state.
            self.kernel = new.kernel;
            self.notify(cx);
        }
    }

    /// A new theme or UI scale: the grid reads both when it is built, so it is built again.
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
        self.grid.set_prefcore(&self.static_info.prefcore_rank, cx);
        let view = self.grid.view.clone();
        self.grid_view.update(cx, |v, cx| v.set_grid(view, cx));
        self.notify(cx);
    }

    /// New strings: the page's words, the grid's headings, Details, the fallback note and the
    /// header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        let words = self.words.refresh(cx);
        let grid = self.grid.refresh_strings(cx);
        if !words && !grid {
            return;
        }
        self.details.refresh_strings(cx);
        self.sync_details(cx);
        self.sync_fallback(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.apply_snapshot(&snap, ComponentMask::INVENTORY, cx);
        self.notify(cx);
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

    /// `PerfSetCpuGraphMode`: with a flow layout the page takes the mode's preset and widgets;
    /// a page without an entry gets one. With a grid layout it asks "Replace your widgets?"
    /// first, then saves the mode's flow layout with the default secondary graph.
    /// `cpu_graph_mode` is not written.
    fn on_set_mode(
        &mut self,
        a: &PerfSetCpuGraphMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let allowed = mode_items().iter().any(|i| i.mode == a.mode && i.enabled);
        if !allowed || !cx.has_global::<TlSettings>() {
            return;
        }
        let mode = a.mode;
        let container = self.container_id.to_string();
        if page_layout(&container, cx).preset.is_some() {
            TlSettings::update(cx, |d| {
                let perf = &mut d.settings.performance;
                let mut layout = container_layout(perf, &container)
                    .unwrap_or_else(|| default_layout(&container));
                set_cpu_preset(&mut layout, mode);
                perf.layouts.set(layout);
            });
            self.sync_settings(cx);
            return;
        }
        let page = cx.weak_entity();
        confirm_preset_page(
            self.header.0.clone(),
            self.words.mode(mode).clone(),
            window,
            cx,
            move |_, cx| {
                let perf = PerformanceSettings {
                    cpu_graph_mode: mode,
                    ..PerformanceSettings::default()
                };
                let layout = convert_cpu_page(&perf, &container);
                TlSettings::update(cx, |d| d.settings.performance.layouts.set(layout));
                let _ = page.update(cx, |p, cx| p.sync_settings(cx));
            },
        );
    }

    /// `PerfSetCpuSecondaryGraph`: replaces the secondary widgets of the page's flow layout. A
    /// page without an entry gets one. `cpu_secondary_graph` is not written. Nothing with a
    /// grid layout, whose menu hides the item.
    fn on_set_secondary(
        &mut self,
        a: &PerfSetCpuSecondaryGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !cx.has_global::<TlSettings>() {
            return;
        }
        let choice = a.choice;
        let container = self.container_id.to_string();
        TlSettings::update(cx, |d| {
            let perf = &mut d.settings.performance;
            let mut layout =
                container_layout(perf, &container).unwrap_or_else(|| default_layout(&container));
            if layout.preset.is_none() {
                return;
            }
            set_cpu_secondary(&mut layout, choice);
            perf.layouts.set(layout);
        });
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

    /// "Edit Widgets…" in the page menu.
    fn on_edit_start(
        &mut self,
        _: &PerfWidgetEditStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        start_page_edit(&self.editor, &self.hub, window, cx);
    }

    /// "Reset Page" in the page menu (ADR-019 phase 3 "Reset"): the entry goes and
    /// `cpu_graph_mode` and `cpu_secondary_graph` go back to their defaults.
    fn on_reset_page(
        &mut self,
        _: &PerfWidgetResetPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        reset_page(&self.container_id, self.header.0.clone(), window, cx);
    }
}

impl DevicePage for CpuPage {
    fn hub(&self) -> Option<&Entity<SnapshotHub>> {
        Some(&self.hub)
    }

    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    /// The mode button (the preset's name; "Graph" with a grid layout) and the kernel toggle.
    fn toggles(&self, _: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 2]> {
        let m = *metrics(cx);
        let w = &self.words;
        let mode = self.mode;
        let menu_focus = self.focus.clone();
        let (labels, later) = (w.modes.clone(), w.later.clone());
        let bounds = Rc::clone(&self.mode_button_bounds);
        let label = mode.map_or_else(|| w.menu_graph.clone(), |m| w.mode(m).clone());
        // A dropdown trigger has no action of its own: the click opens the menu below the
        // button, and its items dispatch `PerfSetCpuGraphMode` on the page.
        let mode_button = TlButton::new(MODE_BUTTON_ID, TlButtonKind::Secondary)
            .size(TlButtonSize::Sm)
            .trailing_icon(TlIcon::ChevronDown)
            .icon_size(m.icon.sm)
            .label(label)
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

    fn graphs(&self, _: &mut Window, _: &mut App) -> AnyElement {
        // The grid, the charts and the board are widgets of the container.
        div().into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    /// "Graph >" (the presets, the current one checked from the flow layout), "Secondary graph
    /// >" with a flow layout only, and "Kernel times" (UI-PRF-023, ADR-019 phase 3).
    fn menu_items(&self, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
        let layout = page_layout(&self.container_id, cx);
        let w = &self.words;
        let mode = layout_mode(&layout);
        let (labels, later) = (w.modes.clone(), w.later.clone());
        let mut menu = menu.submenu(w.menu_graph.clone(), window, cx, move |menu, _, _| {
            mode_menu(menu, mode, &labels, &later)
        });
        if layout.preset.is_some() {
            let (avail, saved, words) = (self.avail, layout_secondary(&layout), w.clone());
            menu = menu.submenu(w.menu_secondary.clone(), window, cx, move |menu, _, _| {
                secondary_menu(menu, saved, avail, &words)
            });
        }
        menu.check_item(
            w.menu_kernel_times.clone(),
            self.kernel,
            Box::new(PerfToggleCpuKernel),
        )
    }

    /// The CPU copy text (PERF-011): the header word and the model on their own lines, then
    /// every Details row indented, whatever the layout holds. The CPU page has no blocks.
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
    }

    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        let mut order = smallvec![
            self.mode_button_focus.clone(),
            self.kernel_button_focus.clone()
        ];
        // The core board's sort toggle, while the board shows.
        if self.board_placed
            && let Some((_, focus)) = &*self.board.borrow()
        {
            order.push(focus.clone());
        }
        order
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

impl Focusable for CpuPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CpuPage {
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
            .on_action(cx.listener(Self::on_set_mode))
            .on_action(cx.listener(Self::on_toggle_kernel))
            .on_action(cx.listener(Self::on_set_secondary))
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

// ---- The all-cores chart specs (UI-PRF-023) ----

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
            vertical_at: None,
        },
        smooth: s.smooth,
        background: ChartBg::Window,
        hover: true,
        ..ChartSpec::default()
    }
}
