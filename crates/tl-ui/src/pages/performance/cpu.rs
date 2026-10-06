//! The CPU page: the Overall chart or the per-core grid, the mode and kernel toggles, the Details
//! grid, and the demand that switches the engine to 10 Hz per-core sampling (docs/ui/performance.md
//! UI-PRF-023, UI-PRF-053; PERF-027, PERF-028, PERF-029, PERF-033; ADR-008, ADR-014; R-126).
//!
//! The page re-renders only when its header, Details rows or toggles change. Both charts are
//! cached views that re-render for their own series. The settings observer reads the page's
//! settings only when the Performance, Motion or Appearance section version moved.
//!
//! The words below match `perf-cpu-*` and `perf-later-version` in `en-US.ftl`, like the fallback
//! words in `details.rs`.

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::devices::{DeviceCatalog, DeviceIdentity};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::CpuGraphMode;
use tl_model::snapshot::{CpuStaticInfo, Snapshot};
use tl_model::system::CpuTopology;
use tl_model::time::now_boot_ns;

use super::actions::{PerfSetCpuGraphMode, PerfToggleCpuKernel};
use super::cpu_details::{
    CpuLiveInputs, CpuSensor, cpu_details_model, cpu_temp_sensors, update_live, update_static,
};
use super::cpu_grid::CpuGrid;
use super::details::KeyValueModel;
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_OVERALL, header_text};
use super::{DeviceKey, DeviceKind, DeviceRow};
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartState, ChartView, GridStyle, LegendStyle,
    ValueAlign, ValueFormat, YScale,
};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::settings_global::{Section, TlSettings};
use crate::theme::{TlTheme, current, metrics};

/// The overlaid and stacked modes. E24-018 sets it to true.
pub const CPU_EXTRA_MODES_ENABLED: bool = false;

/// Element id of the mode button.
pub const MODE_BUTTON_ID: &str = "perf-cpu-mode";
/// Element id of the kernel toggle.
pub const KERNEL_BUTTON_ID: &str = "perf-cpu-kernel";
/// Debug selector of the box around the Overall chart.
pub const OVERALL_ID: &str = "perf-cpu-overall";

const WORD_OVERALL: &str = "Overall";
const WORD_PER_CORE: &str = "Per core";
const WORD_OVERLAID: &str = "All cores overlaid";
const WORD_STACKED: &str = "All cores stacked";
const WORD_KERNEL_ON: &str = "Kernel on";
const WORD_KERNEL_OFF: &str = "Kernel off";
const TIP_MODE: &str = "Graph mode";
const TIP_KERNEL: &str = "Show kernel time";
const TIP_LATER: &str = "Available in a later version";
const MENU_GRAPH: &str = "Graph";
const MENU_KERNEL_TIMES: &str = "Kernel times";
const HEADING_UTILIZATION: &str = "Utilization";
const WORD_TOTAL: &str = "Total";
const WORD_KERNEL: &str = "Kernel";

const NS_PER_S: u64 = 1_000_000_000;

/// One entry of the mode menu.
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
            label: mode_label(mode),
            enabled,
            tooltip: if enabled { None } else { Some(TIP_LATER) },
        }
    }
    [
        item(CpuGraphMode::Overall, false),
        item(CpuGraphMode::PerCore, false),
        item(CpuGraphMode::AllCoresOverlaid, true),
        item(CpuGraphMode::AllCoresStacked, true),
    ]
}

/// The mode button's label for `mode`.
pub const fn mode_label(mode: CpuGraphMode) -> &'static str {
    match mode {
        CpuGraphMode::Overall => WORD_OVERALL,
        CpuGraphMode::PerCore => WORD_PER_CORE,
        CpuGraphMode::AllCoresOverlaid => WORD_OVERLAID,
        CpuGraphMode::AllCoresStacked => WORD_STACKED,
    }
}

/// The kernel toggle's label.
pub const fn kernel_label(kernel: bool) -> &'static str {
    if kernel {
        WORD_KERNEL_ON
    } else {
        WORD_KERNEL_OFF
    }
}

/// Adds the four mode radios; a mode not yet available is a disabled item with its tooltip.
fn mode_menu(mut menu: TlMenu, current: CpuGraphMode) -> TlMenu {
    for item in mode_items() {
        let label = SharedString::new_static(item.label);
        menu = if item.enabled {
            menu.radio_item(
                label,
                item.mode == current,
                Box::new(PerfSetCpuGraphMode { mode: item.mode }),
            )
        } else {
            menu.item_disabled(label, item.tooltip.map(SharedString::new_static))
        };
    }
    menu
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

fn overall_labels(kernel: bool) -> ChartLabels {
    let mut legend = smallvec![(SharedString::new_static(WORD_TOTAL), LegendStyle::Solid)];
    if kernel {
        legend.push((SharedString::new_static(WORD_KERNEL), LegendStyle::Dashed));
    }
    ChartLabels {
        heading: Some(SharedString::new_static(HEADING_UTILIZATION)),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: HEADING_UTILIZATION,
        legend,
        show_axis_max: false,
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
    /// E24-018.
    #[allow(dead_code, reason = "E24-018 creates it on first use")]
    overlaid: Option<Entity<ChartView>>,
    /// E24-018.
    #[allow(dead_code, reason = "E24-018 creates it on first use")]
    stacked: Option<Entity<ChartView>>,
    /// E24-021.
    #[allow(dead_code, reason = "E24-021 creates it")]
    secondary: Option<Entity<ChartView>>,
    details: KeyValueModel,
    mode: CpuGraphMode,
    kernel: bool,
    /// Bench only (UI-PRF-053): the Overall chart above the grid.
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
    catalog_version: u64,
    topology: Arc<CpuTopology>,
    logical: Vec<DeviceIdx>,
    static_info: Arc<CpuStaticInfo>,
    temp_sensors: Vec<CpuSensor>,
    _subs: SmallVec<[Subscription; 2]>,
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
        let spec = overall_spec(row.idx, &settings, &theme);
        let labels = overall_labels(settings.kernel);
        let overall = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
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
            secondary: None,
            details: cpu_details_model(),
            mode: settings.mode,
            kernel: settings.kernel,
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
            catalog_version: snap.devices.version,
            topology,
            logical,
            static_info: Arc::default(),
            temp_sensors: Vec::new(),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        cpu_temp_sensors(&snap.devices, &mut this.temp_sensors);
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
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
        if devices || (system && !Arc::ptr_eq(&self.topology, &snap.system.cpu_topology)) {
            self.topology = Arc::clone(&snap.system.cpu_topology);
            self.grid
                .set_topology(Arc::clone(&self.topology), &self.logical, cx);
        }
        if system {
            let inputs = CpuLiveInputs {
                now_boot_ns: now_boot_ns(),
                cpu_error: snap.health.get(SourceId::Cpu).last_error.as_deref(),
                temp_sensors: &self.temp_sensors,
            };
            changed |= update_live(&mut self.details, &snap.system, &inputs);
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
            let header = header_text(DeviceKind::Cpu, 0, entry, snap);
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
            let labels = overall_labels(new.kernel);
            self.overall.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        self.grid.set_range(new.range_ns, cx);
        self.grid.set_smooth(new.smooth, cx);
        let theme = current(cx).clone();
        let spec = overall_spec(self.row.idx, &new, &theme);
        self.overall.update(cx, |v, cx| v.set_spec(spec, cx));
        if new.mode != old.mode {
            self.mode = new.mode;
            self.send_demand(cx);
        }
        cx.notify();
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
        let mode = self.mode;
        let menu_focus = self.focus.clone();
        // A dropdown trigger has no action of its own: the click opens the menu, whose items
        // dispatch `PerfSetCpuGraphMode` on the page.
        let mode_button = TlButton::new(MODE_BUTTON_ID, TlButtonKind::Secondary)
            .size(TlButtonSize::Sm)
            .icon(TlIcon::ChevronDown)
            .icon_size(m.icon.sm)
            .label(SharedString::new_static(mode_label(mode)))
            .tooltip(SharedString::new_static(TIP_MODE))
            .on_click(move |ev, window, cx| {
                let menu = TlMenu::build(window, cx, menu_focus.clone(), move |menu, _, _| {
                    mode_menu(menu, mode)
                });
                open_context_menu_at(ev.position(), window, cx, menu);
            });
        // Dispatched on the page, so it works wherever focus is.
        let page_focus = self.focus.clone();
        let kernel_button =
            TlButton::new(KERNEL_BUTTON_ID, TlButtonKind::Segment { on: self.kernel })
                .label(SharedString::new_static(kernel_label(self.kernel)))
                .tooltip(SharedString::new_static(TIP_KERNEL))
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
        if self.bench_show_both {
            return v_flex()
                .w_full()
                .child(self.overall_element(m.ui_scale))
                .child(div().h(m.sp[4]))
                .child(self.grid.element(window, cx))
                .into_any_element();
        }
        match self.mode {
            CpuGraphMode::PerCore => self.grid.element(window, cx),
            // The overlaid and stacked modes show the Overall chart until E24-018.
            _ => self.overall_element(m.ui_scale),
        }
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    fn menu_items(&self, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
        let mode = self.mode;
        // E24-021: the "Secondary graph >" submenu.
        menu.submenu(MENU_GRAPH, window, cx, move |menu, _, _| {
            mode_menu(menu, mode)
        })
        .check_item(
            MENU_KERNEL_TIMES,
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
