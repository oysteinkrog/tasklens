//! The Memory page: the Physical memory, Commit and Swap charts stacked above the composition
//! bar and the Details grid, the two page-menu check items and the page's demand
//! (docs/ui/performance.md UI-PRF-026, UI-PRF-020; PERF-018, PERF-026, PERF-041; ADR-008).
//!
//! The charts are cached views that re-render for their own series. The page re-renders only
//! when a chart appears or goes, or its header or Details rows change. The specs are built again
//! only when a total, a chart setting or the theme changed.
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, AppContext as _, AsyncApp,
    Context, ElementId, Entity, EntityId, FocusHandle, Focusable, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StyleRefinement, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::Snapshot;

use super::actions::{PerfToggleMemoryComposition, PerfToggleMemorySwapGraph};
use super::details::KeyValueModel;
use super::memory_bar::{Composition, CompositionBarView};
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_STACKED, page_words};
use super::{DeviceKey, DeviceRow};
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartState, ChartView, GridStyle, LegendStyle,
    ValueAlign, ValueFormat, YScale,
};
use crate::components::menu::TlMenu;
use crate::components::motion::{M_BASE, ease_out_cubic};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, TlSettings, animations_on};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Debug selectors of the chart boxes.
pub const PHYSICAL_ID: &str = "perf-mem-physical";
pub const COMMIT_ID: &str = "perf-mem-commit";
pub const SWAP_ID: &str = "perf-mem-swap";

const KEY_PHYSICAL: &str = "perf-mem-heading-physical";
const KEY_COMMIT: &str = "perf-mem-heading-commit";
const KEY_SWAP: &str = "perf-mem-heading-swap";
const KEY_USED: &str = "perf-mem-legend-used";
const KEY_COMMITTED: &str = "perf-mem-legend-committed";
const KEY_COMMIT_TIP: &str = "perf-mem-commit-tooltip";
const KEY_MENU_COMPOSITION: &str = "perf-mem-menu-composition";
const KEY_MENU_SWAP: &str = "perf-mem-menu-swap";
const KEY_RANGE_MEMORY: &str = "perf-mem-range-memory";
const KEY_RANGE_SWAP: &str = "perf-mem-range-swap";

const NS_PER_S: u64 = 1_000_000_000;
/// Alpha of the dirty-bytes line against `mem_line`.
const DIRTY_ALPHA: f32 = 0.6;

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    pub physical: SharedString,
    pub commit: SharedString,
    pub swap: SharedString,
    pub used: SharedString,
    pub committed: SharedString,
    pub commit_tip: SharedString,
    pub menu_composition: SharedString,
    pub menu_swap: SharedString,
    pub range_memory: SharedString,
    pub range_swap: SharedString,
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
        self.physical = t(KEY_PHYSICAL);
        self.commit = t(KEY_COMMIT);
        self.swap = t(KEY_SWAP);
        self.used = t(KEY_USED);
        self.committed = t(KEY_COMMITTED);
        self.commit_tip = t(KEY_COMMIT_TIP);
        self.menu_composition = t(KEY_MENU_COMPOSITION);
        self.menu_swap = t(KEY_MENU_SWAP);
        self.range_memory = t(KEY_RANGE_MEMORY);
        self.range_swap = t(KEY_RANGE_SWAP);
        true
    }
}

/// The settings the page draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSettings {
    range_ns: u64,
    smooth: bool,
    rounded: bool,
    composition: bool,
    swap: bool,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
            composition: s.performance.memory_composition,
            swap: s.performance.memory_swap_graph,
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

/// `MemTotal`, `CommitLimit` and `SwapTotal` in bytes, from `Snapshot.system`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Totals {
    mem: u64,
    commit_limit: u64,
    swap: u64,
}

impl Totals {
    fn of(snap: &Snapshot) -> Self {
        let m = &snap.system.memory;
        Self {
            mem: m.total,
            commit_limit: m.commit_limit,
            swap: m.swap_total,
        }
    }
}

fn series(device: DeviceIdx, metric: MetricId) -> SeriesId {
    SeriesId { device, metric }
}

fn spec(series: SmallVec<[ChartSeries; 4]>, y: YScale, s: &PageSettings) -> ChartSpec {
    ChartSpec {
        series,
        y,
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

/// Used with fill, committed as a dashed line and dirty bytes as a faint line, 0..MemTotal.
fn physical_spec(device: DeviceIdx, total: u64, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let line = theme.graph.mem_line;
    let series = smallvec![
        ChartSeries {
            id: series(device, MetricId::MEM_USED),
            fill: Some(theme.graph.mem_fill),
            line,
            ..ChartSeries::default()
        },
        ChartSeries {
            id: series(device, MetricId::MEM_COMMIT),
            line,
            dashed: true,
            ..ChartSeries::default()
        },
        ChartSeries {
            id: series(device, MetricId::MEM_DIRTY),
            line: Hsla {
                a: line.a * DIRTY_ALPHA,
                ..line
            },
            line_width: px(1.),
            ..ChartSeries::default()
        },
    ];
    spec(series, YScale::FixedTotal(total as f32), s)
}

/// Committed bytes with fill, 0..max(CommitLimit, peak).
fn commit_spec(device: DeviceIdx, limit: u64, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let series = smallvec![ChartSeries {
        id: series(device, MetricId::MEM_COMMIT),
        fill: Some(theme.graph.mem_fill),
        line: theme.graph.mem_line,
        ..ChartSeries::default()
    }];
    spec(series, YScale::Commit(limit as f32), s)
}

/// Swap used with fill, 0..SwapTotal.
fn swap_spec(device: DeviceIdx, total: u64, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let series = smallvec![ChartSeries {
        id: series(device, MetricId::SWAP_USED),
        fill: Some(theme.graph.mem_fill),
        line: theme.graph.mem_line,
        ..ChartSeries::default()
    }];
    spec(series, YScale::FixedTotal(total as f32), s)
}

fn physical_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.physical.clone()),
        value_fmt: ValueFormat::Bytes,
        value_align: ValueAlign::Right,
        range_word: w.range_memory.clone(),
        // The dirty line has no legend entry.
        legend: smallvec![
            (w.used.clone(), LegendStyle::Solid),
            (w.committed.clone(), LegendStyle::Dashed)
        ],
        show_axis_max: false,
    }
}

/// The value is `Committed_AS / CommitLimit`; the axis text shows when the peak passes 100%.
fn commit_labels(w: &Words, limit: u64) -> ChartLabels {
    ChartLabels {
        heading: Some(w.commit.clone()),
        value_fmt: ValueFormat::PercentOf { total: limit },
        value_align: ValueAlign::Right,
        range_word: w.commit.clone(),
        legend: SmallVec::new(),
        show_axis_max: true,
    }
}

fn swap_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.swap.clone()),
        value_fmt: ValueFormat::Bytes,
        value_align: ValueAlign::Right,
        range_word: w.range_swap.clone(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The chart state from the memory source health: Failed with its error, else Live (the views
/// derive WarmingUp and Stale themselves).
fn health_state(snap: &Snapshot) -> ChartState {
    let h = snap.health.get(SourceId::Memory);
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

/// The Swap chart's height animation (UI-PRF-026).
#[derive(Clone, Copy, Debug)]
struct SwapAnim {
    /// True while the chart opens, false while it closes.
    showing: bool,
    /// Bumped on each change, so a new change restarts the animation.
    generation: u32,
}

/// The Memory device page.
pub struct MemoryPage {
    key: DeviceKey,
    row: DeviceRow,
    physical: Entity<ChartView>,
    commit: Entity<ChartView>,
    swap: Entity<ChartView>,
    /// The composition bar view, which keeps the `CompositionBarState`. Created the first time
    /// the bar is shown; a cached view, so a memory sample repaints only the bar.
    bar: Option<Entity<CompositionBarView>>,
    /// The composition of the newest memory sample.
    composition: Option<Composition>,
    /// E25-003 fills the rows.
    details: KeyValueModel,
    /// `performance.memory_composition`.
    show_composition: bool,
    /// `performance.memory_swap_graph`.
    show_swap: bool,
    swap_total: u64,
    totals: Totals,
    /// The Swap chart is in the graph area (its end state while it animates).
    swap_shown: bool,
    swap_anim: Option<SwapAnim>,
    anim_generation: u32,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    /// `UiMetrics.style_version` the chart colours were built for.
    style_version: u64,
    words: Words,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The page is the shown page (`set_active`).
    active: bool,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    /// The page root; the menu items dispatch their actions here.
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl MemoryPage {
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
        let totals = Totals::of(&snap);
        let mut words = Words::default();
        words.refresh(cx);

        let idx = row.idx;
        let physical = {
            let (spec, labels) = (
                physical_spec(idx, totals.mem, &settings, &theme),
                physical_labels(&words),
            );
            cx.new(|cx| ChartView::new(spec, labels, &hub, cx))
        };
        let commit = {
            let (spec, labels) = (
                commit_spec(idx, totals.commit_limit, &settings, &theme),
                commit_labels(&words, totals.commit_limit),
            );
            cx.new(|cx| ChartView::new(spec, labels, &hub, cx))
        };
        let swap = {
            let (spec, labels) = (
                swap_spec(idx, totals.swap, &settings, &theme),
                swap_labels(&words),
            );
            cx.new(|cx| ChartView::new(spec, labels, &hub, cx))
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<UiMetrics>(Self::sync_style));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let header = (page_words(cx).memory.clone(), SharedString::default());
        let composition = Composition::of(&snap.system.memory);
        let mut this = Self {
            key,
            row,
            physical,
            commit,
            swap,
            bar: None,
            composition,
            details: KeyValueModel::default(),
            show_composition: settings.composition,
            show_swap: settings.swap,
            swap_total: totals.swap,
            totals,
            swap_shown: settings.swap && totals.swap > 0,
            swap_anim: None,
            anim_generation: 0,
            header,
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            words,
            demand,
            id,
            active: false,
            window: None,
            focus: cx.focus_handle(),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        let state = health_state(&snap);
        this.set_chart_state(state, cx);
        this.sync_bar(cx);
        this
    }

    /// The composition bar view, once it has been shown.
    pub fn bar(&self) -> Option<&Entity<CompositionBarView>> {
        self.bar.as_ref()
    }

    /// Creates the bar view when the bar is shown for the first time, and hands it the newest
    /// composition. The bar view notifies itself only when the composition changed.
    fn sync_bar(&mut self, cx: &mut Context<Self>) {
        if !self.show_composition {
            return;
        }
        let c = self.composition;
        match &self.bar {
            Some(bar) => bar.update(cx, |b, cx| b.set_composition(c, cx)),
            None => self.bar = Some(cx.new(|cx| CompositionBarView::new(c, cx))),
        }
    }

    pub fn physical(&self) -> &Entity<ChartView> {
        &self.physical
    }

    pub fn commit(&self) -> &Entity<ChartView> {
        &self.commit
    }

    pub fn swap(&self) -> &Entity<ChartView> {
        &self.swap
    }

    pub fn show_composition(&self) -> bool {
        self.show_composition
    }

    pub fn show_swap(&self) -> bool {
        self.show_swap
    }

    /// The Swap chart is in the graph area, or will be when its animation ends.
    pub fn swap_shown(&self) -> bool {
        self.swap_shown
    }

    /// The Swap chart's height is animating.
    pub fn swap_animating(&self) -> bool {
        self.swap_anim.is_some()
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// This page's demand part while it is active: memory only.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            memory: true,
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

    fn set_chart_state(&self, state: ChartState, cx: &mut App) {
        for chart in [&self.physical, &self.commit, &self.swap] {
            let state = state.clone();
            chart.update(cx, |v, cx| v.set_state(state, cx));
        }
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let snap = Arc::clone(hub.read(cx).current());
        if ev.mask.contains(ComponentMask::SYSTEM) {
            self.composition = Composition::of(&snap.system.memory);
            self.sync_bar(cx);
            let totals = Totals::of(&snap);
            if totals != self.totals {
                let old = self.totals;
                self.totals = totals;
                self.swap_total = totals.swap;
                self.rebuild_specs(old, cx);
                self.sync_swap_shown(cx);
            }
        }
        if ev.mask.contains(ComponentMask::HEALTH) {
            self.set_chart_state(health_state(&snap), cx);
        }
    }

    /// Hands each chart whose total changed since `old` its new spec, and the Commit chart its
    /// new value format.
    fn rebuild_specs(&self, old: Totals, cx: &mut App) {
        let (idx, s, t) = (self.row.idx, self.settings, self.totals);
        let theme = current(cx).clone();
        if t.mem != old.mem {
            let spec = physical_spec(idx, t.mem, &s, &theme);
            self.physical.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        if t.commit_limit != old.commit_limit {
            let spec = commit_spec(idx, t.commit_limit, &s, &theme);
            let labels = commit_labels(&self.words, t.commit_limit);
            self.commit.update(cx, |v, cx| {
                v.set_spec(spec, cx);
                v.set_labels(labels, cx);
            });
        }
        if t.swap != old.swap {
            let spec = swap_spec(idx, t.swap, &s, &theme);
            self.swap.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }

    /// Every chart's spec from the current settings, totals and theme.
    fn rebuild_all_specs(&self, cx: &mut App) {
        let (idx, s, t) = (self.row.idx, self.settings, self.totals);
        let theme = current(cx).clone();
        let physical = physical_spec(idx, t.mem, &s, &theme);
        let commit = commit_spec(idx, t.commit_limit, &s, &theme);
        let swap = swap_spec(idx, t.swap, &s, &theme);
        self.physical.update(cx, |v, cx| v.set_spec(physical, cx));
        self.commit.update(cx, |v, cx| v.set_spec(commit, cx));
        self.swap.update(cx, |v, cx| v.set_spec(swap, cx));
    }

    /// Shows or hides the Swap chart when `show_swap` or `SwapTotal` moved. The height animates
    /// over `m_base` unless animations are off.
    fn sync_swap_shown(&mut self, cx: &mut Context<Self>) {
        let wanted = self.show_swap && self.swap_total > 0;
        if wanted == self.swap_shown {
            return;
        }
        self.swap_shown = wanted;
        self.anim_generation = self.anim_generation.wrapping_add(1);
        if animations_on(cx) {
            let generation = self.anim_generation;
            self.swap_anim = Some(SwapAnim {
                showing: wanted,
                generation,
            });
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(M_BASE).await;
                let _ = this.update(cx, |this, cx| {
                    if this.anim_generation == generation {
                        this.swap_anim = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        } else {
            self.swap_anim = None;
        }
        cx.notify();
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
        if (new.range_ns, new.smooth, new.rounded) != (old.range_ns, old.smooth, old.rounded) {
            self.rebuild_all_specs(cx);
        }
        if new.composition != old.composition {
            self.show_composition = new.composition;
            self.sync_bar(cx);
            cx.notify();
        }
        if new.swap != old.swap {
            self.show_swap = new.swap;
            self.sync_swap_shown(cx);
        }
    }

    /// A new theme: the charts take the new colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_all_specs(cx);
        cx.notify();
    }

    /// New strings: the chart labels, the header and the menu words.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        if !self.words.refresh(cx) {
            return;
        }
        let (physical, commit, swap) = (
            physical_labels(&self.words),
            commit_labels(&self.words, self.totals.commit_limit),
            swap_labels(&self.words),
        );
        self.physical.update(cx, |v, cx| v.set_labels(physical, cx));
        self.commit.update(cx, |v, cx| v.set_labels(commit, cx));
        self.swap.update(cx, |v, cx| v.set_labels(swap, cx));
        self.header.0 = page_words(cx).memory.clone();
        cx.notify();
    }

    fn on_toggle_composition(
        &mut self,
        _: &PerfToggleMemoryComposition,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        TlSettings::update(cx, |d| {
            let p = &mut d.settings.performance;
            p.memory_composition = !p.memory_composition;
        });
        self.sync_settings(cx);
    }

    fn on_toggle_swap(
        &mut self,
        _: &PerfToggleMemorySwapGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        TlSettings::update(cx, |d| {
            let p = &mut d.settings.performance;
            p.memory_swap_graph = !p.memory_swap_graph;
        });
        self.sync_settings(cx);
    }

    /// `chart` in a full-width box of `height`.
    fn chart_box(id: &'static str, chart: &Entity<ChartView>, height: f32) -> gpui_kit::Div {
        div()
            .debug_selector(move || id.into())
            .flex_none()
            .w_full()
            .h(px(height))
            .child(chart.clone().cached(StyleRefinement::default().size_full()))
    }
}

impl DevicePage for MemoryPage {
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
        let h = GRAPH_H_STACKED * m.ui_scale;

        // The heading tooltip covers the Commit chart box.
        let commit = div()
            .id(ElementId::NamedChild(
                Arc::new(ElementId::Name(SharedString::from(self.key.0.clone()))),
                COMMIT_ID.into(),
            ))
            .w_full()
            .child(Self::chart_box(COMMIT_ID, &self.commit, h));
        let tip = HoverTooltip::state((COMMIT_ID, self.id.as_u64() as usize), window, cx);
        let commit = HoverTooltip::attach(
            &tip,
            commit,
            DELAY_CONTROL,
            TlTooltip::text(self.words.commit_tip.clone()).build(),
            window,
            cx,
        );

        let swap = match self.swap_anim {
            None => self
                .swap_shown
                .then(|| Self::chart_box(SWAP_ID, &self.swap, h).into_any_element()),
            Some(SwapAnim {
                showing,
                generation,
            }) => Some(
                div()
                    .flex_none()
                    .w_full()
                    .overflow_hidden()
                    .child(Self::chart_box(SWAP_ID, &self.swap, h))
                    .with_animation(
                        ("perf-mem-swap-anim", generation as usize),
                        Animation::new(M_BASE).with_easing(ease_out_cubic),
                        move |el, t| el.h(px(h * if showing { t } else { 1. - t })),
                    )
                    .into_any_element(),
            ),
        };

        // The composition bar after `sp6` when `show_composition`.
        let bar = self
            .bar
            .as_ref()
            .filter(|_| self.show_composition)
            .map(|bar| {
                div()
                    .w_full()
                    .pt(m.sp[6] - m.sp[4])
                    .child(bar.clone().cached(StyleRefinement::default().w_full()))
            });
        v_flex()
            .w_full()
            .gap(m.sp[4])
            .child(Self::chart_box(PHYSICAL_ID, &self.physical, h))
            .child(commit)
            .children(swap)
            .children(bar)
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        let w = &self.words;
        menu.check_item(
            w.menu_composition.clone(),
            self.show_composition,
            Box::new(PerfToggleMemoryComposition),
        )
        .check_item(
            w.menu_swap.clone(),
            self.show_swap,
            Box::new(PerfToggleMemorySwapGraph),
        )
    }

    /// E25-029 replaces this with the full Memory copy text.
    fn copy_text(&self, out: &mut String) {
        out.push_str(&self.header.0);
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

impl Focusable for MemoryPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MemoryPage {
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
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_toggle_composition))
            .on_action(cx.listener(Self::on_toggle_swap))
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
