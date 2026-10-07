//! The Disk page: one page per physical disk with the Active time and Transfer rate charts
//! above the Details grid, and the page's demand (docs/ui/performance.md UI-PRF-029,
//! UI-PRF-020; PERF-047, PERF-048; ADR-008).
//!
//! The charts are cached views that re-render for their own series. The page re-renders only
//! when its header or Details rows change. The specs are built again only when a chart setting
//! or the theme changed.
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{SmartKind, Snapshot};

use super::details::KeyValueModel;
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_STACKED, header_text, page_words};
use super::{
    DeviceKey, DeviceKind, DeviceRow, PerformanceSettingsView, sidebar_order, type_number,
    visible_rows,
};
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartState, ChartView, GridStyle, LegendStyle,
    ValueAlign, ValueFormat, YScale,
};
use crate::components::menu::TlMenu;
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, TlSettings};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Debug selectors of the chart boxes.
pub const ACTIVE_ID: &str = "perf-disk-active";
pub const TRANSFER_ID: &str = "perf-disk-transfer";

const KEY_ACTIVE: &str = "perf-disk-heading-active";
const KEY_TRANSFER: &str = "perf-disk-heading-transfer";
const KEY_READ: &str = "perf-disk-legend-read";
const KEY_WRITE: &str = "perf-disk-legend-write";
const KEY_THROUGHPUT: &str = "perf-disk-range-throughput";

const NS_PER_S: u64 = 1_000_000_000;

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    pub active: SharedString,
    pub transfer: SharedString,
    pub read: SharedString,
    pub write: SharedString,
    pub throughput: SharedString,
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
        self.active = t(KEY_ACTIVE);
        self.transfer = t(KEY_TRANSFER);
        self.read = t(KEY_READ);
        self.write = t(KEY_WRITE);
        self.throughput = t(KEY_THROUGHPUT);
        true
    }
}

/// The settings the charts draw from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSettings {
    range_ns: u64,
    smooth: bool,
    rounded: bool,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
        }
    }
}

/// The versions of the sections [`PageSettings`] and the disk number read.
fn section_versions(cx: &App) -> [u64; 3] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Appearance),
    ]
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

/// Active time with fill, 0..100.
fn active_spec(device: DeviceIdx, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let series = smallvec![ChartSeries {
        id: series(device, MetricId::DISK_ACTIVE),
        fill: Some(theme.graph.disk_fill),
        line: theme.graph.disk_line,
        ..ChartSeries::default()
    }];
    let y = YScale::Fixed {
        min: 0.0,
        max: 100.0,
    };
    spec(series, y, s)
}

/// Read with fill and write as a dashed line, on a power-of-two scale.
fn transfer_spec(device: DeviceIdx, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let line = theme.graph.disk_line;
    let series = smallvec![
        ChartSeries {
            id: series(device, MetricId::DISK_READ),
            fill: Some(theme.graph.disk_fill),
            line,
            ..ChartSeries::default()
        },
        ChartSeries {
            id: series(device, MetricId::DISK_WRITE),
            line,
            dashed: true,
            ..ChartSeries::default()
        },
    ];
    spec(series, YScale::AutoRate { iec: true }, s)
}

fn active_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.active.clone()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: w.active.clone(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

fn transfer_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.transfer.clone()),
        value_fmt: ValueFormat::RateIec,
        value_align: ValueAlign::Right,
        range_word: w.throughput.clone(),
        legend: smallvec![
            (w.read.clone(), LegendStyle::Solid),
            (w.write.clone(), LegendStyle::Dashed)
        ],
        show_axis_max: true,
    }
}

/// The chart state from the disk source health: Failed with its error, else Live (the views
/// derive WarmingUp and Stale themselves).
fn health_state(snap: &Snapshot) -> ChartState {
    let h = snap.health.get(SourceId::Disk);
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

/// "Disk N": the index of `key` among the visible disks in sidebar order, counted as the
/// sidebar counts it (PERF-008). Runs on catalog and settings changes only.
fn disk_number(key: &DeviceKey, snap: &Snapshot, cx: &App) -> u16 {
    let p = &TlSettings::settings(cx).performance;
    let k = |s: &str| DeviceKey(Arc::from(s));
    let order: Vec<DeviceKey> = p.device_order.iter().map(|s| k(s)).collect();
    let overrides: HashMap<_, _> = p.device_overrides.iter().map(|(s, v)| (k(s), *v)).collect();
    let view = PerformanceSettingsView {
        order: &order,
        overrides: &overrides,
        show_disks: p.show_disks,
        show_gpus: p.show_gpus,
        show_fans: p.show_fans,
        show_batteries: p.show_batteries,
        show_network_wired: p.show_network_wired,
        show_network_wireless: p.show_network_wireless,
        show_network_vpn: p.show_network_vpn,
        show_network_virtual: p.show_network_virtual,
        show_network_other: p.show_network_other,
    };
    let mut rows = Vec::new();
    sidebar_order(&snap.devices, &view, &mut rows);
    let visible: Vec<DeviceRow> = visible_rows(&rows, &view).cloned().collect();
    type_number(&visible, key)
}

/// The Disk device page.
pub struct DiskPage {
    key: DeviceKey,
    row: DeviceRow,
    active: Entity<ChartView>,
    transfer: Entity<ChartView>,
    /// E25-005 fills the rows.
    details: KeyValueModel,
    // E25-016: partitions: Option<PartitionsBlock> (added by o55.17 as a chain edit, R-262)
    /// E25-017 renders the SMART button from it.
    smart_available: bool,
    /// E25-017 renders the Eject button from it.
    ejectable: bool,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    /// `UiMetrics.style_version` the chart colours were built for.
    style_version: u64,
    words: Words,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The page is the shown page (`set_active`).
    is_active: bool,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    /// The page root.
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl DiskPage {
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
        let mut words = Words::default();
        words.refresh(cx);

        let idx = row.idx;
        let active = {
            let (spec, labels) = (active_spec(idx, &settings, &theme), active_labels(&words));
            cx.new(|cx| ChartView::new(spec, labels, &hub, cx))
        };
        let transfer = {
            let (spec, labels) = (
                transfer_spec(idx, &settings, &theme),
                transfer_labels(&words),
            );
            cx.new(|cx| {
                let mut view = ChartView::new(spec, labels, &hub, cx);
                // The value is read plus write.
                view.set_value_sum(true, cx);
                view
            })
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

        let mut this = Self {
            key,
            row,
            active,
            transfer,
            details: KeyValueModel::default(),
            smart_available: false,
            ejectable: false,
            header: (SharedString::default(), SharedString::default()),
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            words,
            hub,
            demand,
            id,
            is_active: false,
            window: None,
            focus: cx.focus_handle(),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    pub fn active_chart(&self) -> &Entity<ChartView> {
        &self.active
    }

    pub fn transfer_chart(&self) -> &Entity<ChartView> {
        &self.transfer
    }

    pub fn smart_available(&self) -> bool {
        self.smart_available
    }

    pub fn ejectable(&self) -> bool {
        self.ejectable
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// This page's demand part while it is active: disks only.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            disks: true,
            ..Demand::default()
        }
    }

    fn send_demand(&self, cx: &mut App) {
        if !self.is_active {
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

    /// Brings the header, the button flags and the chart state up to date with the components
    /// in `mask`. Notifies only when something the page draws changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let mut changed = false;
        let details = mask.contains(ComponentMask::DEVICE_DETAILS);
        if details {
            let d = snap
                .device_details
                .disks
                .iter()
                .find(|(i, _)| *i == self.row.idx)
                .map(|(_, d)| d);
            let smart = d.is_some_and(|d| d.smart != SmartKind::None);
            let eject = d.is_some_and(|d| d.ejectable);
            if (smart, eject) != (self.smart_available, self.ejectable) {
                (self.smart_available, self.ejectable) = (smart, eject);
                changed = true;
            }
        }
        if mask.contains(ComponentMask::HEALTH) {
            let state = health_state(snap);
            for chart in [&self.active, &self.transfer] {
                let state = state.clone();
                chart.update(cx, |v, cx| v.set_state(state, cx));
            }
        }
        // The model comes from the disk details or the inventory.
        if details || mask.intersects(ComponentMask::DEVICES.union(ComponentMask::INVENTORY)) {
            changed |= self.sync_header(snap, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// The header from the catalog entry and the disk model. True when it changed.
    fn sync_header(&mut self, snap: &Snapshot, cx: &mut App) -> bool {
        let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx) else {
            return false;
        };
        let number = disk_number(&self.key, snap, cx);
        let header = header_text(page_words(cx), DeviceKind::Disk, number, entry, snap);
        if header == self.header {
            return false;
        }
        self.header = header;
        true
    }

    /// Applies the chart settings and the disk number when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        let performance_moved = versions[0] != self.settings_versions[0];
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        if new != self.settings {
            self.settings = new;
            self.rebuild_specs(cx);
        }
        // `device_order` and the visibility settings move the number in "Disk N".
        if performance_moved {
            let snap = Arc::clone(self.hub.read(cx).current());
            if self.sync_header(&snap, cx) {
                cx.notify();
            }
        }
    }

    fn rebuild_specs(&self, cx: &mut App) {
        let (idx, s) = (self.row.idx, self.settings);
        let theme = current(cx).clone();
        let active = active_spec(idx, &s, &theme);
        let transfer = transfer_spec(idx, &s, &theme);
        self.active.update(cx, |v, cx| v.set_spec(active, cx));
        self.transfer.update(cx, |v, cx| v.set_spec(transfer, cx));
    }

    /// A new theme: the charts take the new colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_specs(cx);
        cx.notify();
    }

    /// New strings: the chart labels and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        if !self.words.refresh(cx) {
            return;
        }
        let (active, transfer) = (active_labels(&self.words), transfer_labels(&self.words));
        self.active.update(cx, |v, cx| v.set_labels(active, cx));
        self.transfer.update(cx, |v, cx| v.set_labels(transfer, cx));
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_header(&snap, cx);
        cx.notify();
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

impl DevicePage for DiskPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    /// E25-017 adds the SMART and Eject buttons.
    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        let h = GRAPH_H_STACKED * m.ui_scale;
        v_flex()
            .w_full()
            .gap(m.sp[4])
            .child(Self::chart_box(ACTIVE_ID, &self.active, h))
            .child(Self::chart_box(TRANSFER_ID, &self.transfer, h))
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    /// E25-016 adds the partitions table.
    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    /// No page items in P1.
    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
    }

    /// E25-029 replaces this with the full Disk copy text.
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

    /// E25-017 adds the header buttons.
    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        SmallVec::new()
    }

    fn set_active(&mut self, active: bool, cx: &mut App) {
        if self.is_active == active {
            return;
        }
        self.is_active = active;
        if active {
            self.send_demand(cx);
        } else {
            let id = self.id;
            self.demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }
}

impl Focusable for DiskPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DiskPage {
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
