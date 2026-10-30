//! The CPU page's core board (crw.104): the Overview CPU box's core rows ([`core_board`]) with
//! each logical CPU's load and clock, under a heading with a sort toggle. The Core board graph
//! mode puts it right of the total chart.
//!
//! The board is its own view, so the page does not render for it. Like the Overview box it shows
//! each core's moving average of `CPU_TOTAL` (`performance.overview_core_smoothing`, UI-PRF-018)
//! and notifies only when a 1% bucket, a clock label or the row order changed. The clocks are
//! the system snapshot's `per_cpu_mhz`, read only when the snapshot has a new `Arc`. The sort
//! toggle saves `performance.cpu_board_sort`: by CPU number, or by load with the busiest first.

use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size, Styled as _, Subscription,
    Window, div, px, size,
};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::{CoreSmoothing, CoreSort};
use tl_model::snapshot::Snapshot;
use tl_model::system::Metric;
use tl_model::validity::Validity;

use super::cpu::logical_devices;
use super::overview::cpu_box::{CoreAverage, KEY_CORE, core_spec, newest};
use super::overview::parts::{
    ChartSettings, CoreBarsLayout, CoreBarsModel, core_bars_layout, core_board, spark_labels,
};
use crate::chart::view::ChangedRuns;
use crate::chart::{ChartBg, ChartSpec, ChartView, ValueFormat};
use crate::components::button::{TlButton, TlButtonKind};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::{Section, TlSettings};
use crate::text::style::t_sm;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The board's debug selector.
pub const BOARD_ID: &str = "perf-cpu-board";
/// Element id of the sort toggle.
pub const SORT_BUTTON_ID: &str = "perf-cpu-board-sort";

const KEY_HEADING: &str = "perf-cpu-board-heading";
const KEY_SORT: &str = "perf-cpu-board-sort";
const KEY_TIP_SORT: &str = "perf-cpu-board-tip-sort";

const NS_PER_S: u64 = 1_000_000_000;

/// The settings the board draws from.
#[derive(Clone, Copy, PartialEq, Debug)]
struct BoardSettings {
    sort: CoreSort,
    smoothing: CoreSmoothing,
    chart: ChartSettings,
}

impl BoardSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            sort: s.performance.cpu_board_sort,
            smoothing: s.performance.overview_core_smoothing,
            chart: ChartSettings {
                kernel: s.performance.cpu_kernel,
                range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
                // The sparklines step at the sample rate, as on the Overview: a sliding chart
                // notifies every animation frame.
                smooth: false,
                rounded: s.appearance.rounded_corners,
            },
        }
    }
}

/// The Overview's core sparkline of logical CPU `device`, on the page background.
fn spark_spec(device: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        background: ChartBg::Window,
        ..core_spec(device, s, theme)
    }
}

/// The versions of the sections [`BoardSettings`] reads.
fn section_versions(cx: &App) -> [u64; 2] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Appearance),
    ]
}

/// The CPU page's core board.
pub struct CpuBoard {
    hub: Entity<SnapshotHub>,
    bars: Rc<RefCell<CoreBarsModel>>,
    /// One sparkline per entry of `logical`.
    sparks: Vec<Entity<ChartView>>,
    /// Logical CPU id to device; `DeviceIdx(u16::MAX)` for an id with no catalog entry.
    logical: Vec<DeviceIdx>,
    /// `CPU_TOTAL` of each entry of `logical`.
    logical_ids: Vec<SeriesId>,
    /// The moving average of each entry of `logical`.
    averages: Vec<CoreAverage>,
    /// The per-CPU clocks last handed to the rows.
    clocks: Arc<[Metric<u32>]>,
    catalog_version: u64,
    /// The rows' size at the last paint, and the layout the last render used.
    grid_size: Rc<Cell<Size<Pixels>>>,
    grid: CoreBarsLayout,
    settings: BoardSettings,
    settings_versions: [u64; 2],
    style_version: u64,
    strings_version: u64,
    heading: SharedString,
    sort_label: SharedString,
    sort_tip: SharedString,
    sort_focus: FocusHandle,
    _subs: [Subscription; 4],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl CpuBoard {
    /// The board, its sort toggle focused through `sort_focus`.
    pub fn new(hub: &Entity<SnapshotHub>, sort_focus: FocusHandle, cx: &mut Context<Self>) -> Self {
        let settings = BoardSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let grid_size = size(px(0.), px(0.));
        let mut bars = CoreBarsModel::default();
        bars.set_by_load(settings.sort == CoreSort::Load);
        let mut this = Self {
            hub: hub.clone(),
            bars: Rc::new(RefCell::new(bars)),
            sparks: Vec::new(),
            logical: Vec::new(),
            logical_ids: Vec::new(),
            averages: Vec::new(),
            clocks: Arc::default(),
            catalog_version: u64::MAX,
            grid_size: Rc::new(Cell::new(grid_size)),
            grid: core_bars_layout(0, grid_size.width, grid_size.height, 1.),
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            strings_version: cx.global::<Strings>().strings_version,
            heading: tr(cx, KEY_HEADING),
            sort_label: tr(cx, KEY_SORT),
            sort_tip: tr(cx, KEY_TIP_SORT),
            sort_focus,
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<TlSettings>(Self::sync_settings),
                cx.observe_global::<UiMetrics>(Self::sync_style),
                cx.observe_global::<Strings>(Self::sync_strings),
            ],
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.sync_devices(&snap, cx);
        this.sync_clocks(&snap, true);
        this
    }

    pub fn bars(&self) -> Ref<'_, CoreBarsModel> {
        self.bars.borrow()
    }

    /// One sparkline per logical CPU.
    pub fn core_sparks(&self) -> &[Entity<ChartView>] {
        &self.sparks
    }

    /// The row layout of the last render.
    pub fn core_layout(&self) -> CoreBarsLayout {
        self.grid
    }

    /// The saved order of the rows.
    pub fn sort(&self) -> CoreSort {
        self.settings.sort
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let devices = ev.mask.contains(ComponentMask::DEVICES);
        let series = ev.mask.contains(ComponentMask::SERIES);
        let system = ev.mask.contains(ComponentMask::SYSTEM);
        if !(devices || series || system) {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        let new_cpus = devices && self.sync_devices(&snap, cx);
        let mut changed = new_cpus;
        if series && !new_cpus {
            let runs = ChangedRuns::new(&ev.changed_series);
            if self.logical_ids.iter().any(|id| runs.contains(*id)) {
                changed |= self.sync_values(&snap);
            }
        }
        // New rows take the clocks again.
        if system || new_cpus {
            changed |= self.sync_clocks(&snap, new_cpus);
        }
        if changed {
            self.notify(cx);
        }
    }

    fn notify(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// Rebuilds the logical CPU list, the row names and the sparklines when the catalog changed.
    /// True when it did.
    fn sync_devices(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        if snap.devices.version == self.catalog_version {
            return false;
        }
        self.catalog_version = snap.devices.version;
        let old = std::mem::take(&mut self.logical);
        logical_devices(&snap.devices, &mut self.logical);
        self.logical_ids.clear();
        self.logical_ids
            .extend(self.logical.iter().map(|&device| SeriesId {
                device,
                metric: MetricId::CPU_TOTAL,
            }));
        self.averages.clear();
        self.averages
            .resize(self.logical.len(), CoreAverage::default());
        self.sync_sparks(&old, false, cx);
        self.sync_names(cx);
        self.sync_values(snap);
        true
    }

    /// One sparkline per logical CPU. Keeps the charts of CPUs whose device did not change,
    /// unless `all` (new settings or theme).
    fn sync_sparks(&mut self, old: &[DeviceIdx], all: bool, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        self.sparks.truncate(self.logical.len());
        for (i, &device) in self.logical.iter().enumerate() {
            let spec = spark_spec(device, &self.settings.chart, &theme);
            if let Some(chart) = self.sparks.get(i) {
                if all || old.get(i) != Some(&device) {
                    chart.update(cx, |v, cx| v.set_spec(spec, cx));
                }
            } else {
                let hub = self.hub.clone();
                let labels = spark_labels(ValueFormat::Percent);
                let chart = cx.new(|cx| ChartView::new(spec, labels, &hub, cx).with_newest_marks());
                self.sparks.push(chart);
            }
        }
    }

    /// "C0", "C1", … for the logical CPUs. The rows keep their values and clocks.
    fn sync_names(&mut self, cx: &mut Context<Self>) {
        let names = (0..self.logical.len())
            .map(|n| tr_args(cx, KEY_CORE, &[("n", FluentValue::from(n))]))
            .collect::<Vec<_>>();
        let mut bars = self.bars.borrow_mut();
        let (values, freqs) = (bars.values.clone(), bars.freqs.clone());
        bars.set_cpus(names.into_iter());
        bars.set_values(values.into_iter());
        bars.set_freqs(freqs.into_iter());
    }

    /// The smoothed `CPU_TOTAL` of each logical CPU (the newest valid sample with smoothing off)
    /// to the rows. True when a 1% bucket or the row order changed.
    fn sync_values(&mut self, snap: &Snapshot) -> bool {
        let tau_s = self.settings.smoothing.tau_s();
        let mut bars = self.bars.borrow_mut();
        if tau_s > 0.0 {
            for (avg, id) in self.averages.iter_mut().zip(&self.logical_ids) {
                if let Some(view) = snap.series.get(*id) {
                    avg.advance(&view.samples, tau_s);
                }
            }
            bars.set_values(self.averages.iter().map(|avg| avg.value))
        } else {
            bars.set_values(self.logical_ids.iter().map(|id| newest(snap, *id)))
        }
    }

    /// The snapshot's `per_cpu_mhz` and BogoMIPS flag to the rows, when the clocks are a new
    /// `Arc` or with `force`. A clock that is not Valid or Stale shows "…". True when a clock
    /// label changed.
    fn sync_clocks(&mut self, snap: &Snapshot, force: bool) -> bool {
        let cpu = &snap.system.cpu;
        let mut bars = self.bars.borrow_mut();
        let mut changed = bars.set_clock_mark(cpu.freq_is_bogomips);
        if !force && Arc::ptr_eq(&self.clocks, &cpu.per_cpu_mhz) {
            return changed;
        }
        self.clocks = Arc::clone(&cpu.per_cpu_mhz);
        let clocks = &self.clocks;
        let shown = |m: &Metric<u32>| matches!(m.validity, Validity::Valid | Validity::Stale);
        changed |= bars.set_freqs(
            (0..self.logical.len())
                .map(|id| clocks.get(id).filter(|m| shown(m)).map(|m| m.value as f32)),
        );
        changed
    }

    /// Applies the settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = BoardSettings::read(cx);
        let old = std::mem::replace(&mut self.settings, new);
        if new == old {
            return;
        }
        let mut changed = self
            .bars
            .borrow_mut()
            .set_by_load(new.sort == CoreSort::Load);
        // The toggle shows the saved order.
        changed |= new.sort != old.sort;
        if new.smoothing != old.smoothing {
            self.averages.fill(CoreAverage::default());
            let snap = Arc::clone(self.hub.read(cx).current());
            changed |= self.sync_values(&snap);
        }
        if new.chart != old.chart {
            self.sync_sparks(&[], true, cx);
        }
        if changed {
            self.notify(cx);
        }
    }

    /// A new theme or UI scale: new sparkline colours, and the rows draw in the new style.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.sync_sparks(&[], true, cx);
        cx.notify();
    }

    /// A new language: new heading, toggle and core names.
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.heading = tr(cx, KEY_HEADING);
        self.sort_label = tr(cx, KEY_SORT);
        self.sort_tip = tr(cx, KEY_TIP_SORT);
        self.sync_names(cx);
        cx.notify();
    }
}

impl Render for CpuBoard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let (rows, grid) = core_board(
            &self.bars,
            &self.sparks,
            &self.grid_size,
            &m,
            cx.entity_id(),
        );
        self.grid = grid;
        let by_load = self.settings.sort == CoreSort::Load;
        let sort = TlButton::new(SORT_BUTTON_ID, TlButtonKind::Segment { on: by_load })
            .label(self.sort_label.clone())
            .tooltip(self.sort_tip.clone())
            .focus_handle(self.sort_focus.clone())
            .on_click(move |_, _, cx| {
                let sort = if by_load {
                    CoreSort::Number
                } else {
                    CoreSort::Load
                };
                TlSettings::update(cx, |d| d.settings.performance.cpu_board_sort = sort);
            });
        let mut heading = div();
        *heading.text_style() = t_sm(&m);
        v_flex()
            .debug_selector(|| BOARD_ID.into())
            .size_full()
            .gap(m.sp[2])
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        heading
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(self.heading.clone()),
                    )
                    .child(sort),
            )
            .child(div().relative().flex_1().min_h_0().w_full().child(rows))
    }
}
