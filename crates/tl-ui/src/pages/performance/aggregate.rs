//! The Aggregate storage and Aggregate graphics pages: one page that sums over every disk or
//! every GPU, with two charts above the Details grid (docs/ui/performance.md UI-PRF-036;
//! PERF-008, PERF-009; E24-024).
//!
//! The series come from the engine's aggregate device (`row.idx`): disk active time is the
//! busiest disk, read and write are sums; GPU utilization is the busiest adapter, memory is a
//! sum. The Details rows come from the member devices, because `Snapshot.system` has no record
//! for the aggregate itself. A page exists only with two or more members (see `build_page`).
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version.

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Div, Entity, EntityId, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::devices::{DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId};
use tl_model::snapshot::{InvField, Snapshot};
use tl_model::system::Metric;
use tl_model::units;
use tl_model::validity::Validity;

use super::details::{Fallback, KeyValueModel};
use super::disk::{
    self, KIND_KEYS, PageSettings, health_state, kind_key, section_versions, transfer_spec,
};
use super::gpu;
use super::page::{
    DevicePage, DevicePageFrame, GRAPH_H_STACKED, header_text, page_history_request, page_words,
};
use super::{DeviceKey, DeviceKind, DeviceRow};
use crate::chart::{ChartLabels, ChartSeries, ChartView, ValueAlign, ValueFormat, YScale};
use crate::components::menu::TlMenu;
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::TlSettings;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Debug selectors of the chart boxes.
pub const DISK_ACTIVE_ID: &str = "perf-agg-disk-active";
pub const DISK_TRANSFER_ID: &str = "perf-agg-disk-transfer";
pub const GPU_UTILIZATION_ID: &str = "perf-agg-gpu-utilization";
pub const GPU_MEMORY_ID: &str = "perf-agg-gpu-memory";

pub const ROW_DISKS: &str = "perf-agg-row-disks";
pub const ROW_ADAPTERS: &str = "perf-agg-row-adapters";
pub const ROW_TYPE: &str = "perf-agg-row-type";

const KEY_UTILIZATION: &str = "perf-agg-heading-utilization";
const KEY_MULTIPLE: &str = "perf-card-multiple";
const KEY_MEMORY: &str = "perf-gpu-heading-memory";
const KEY_MEMORY_USAGE: &str = "perf-gpu-range-memory";
const KEY_DEDICATED: &str = "perf-gpu-legend-dedicated";
const KEY_SHARED: &str = "perf-gpu-legend-shared";

/// The Details rows of the Aggregate storage page, in grid order.
pub const DISK_ROWS: [&str; 10] = [
    disk::ROW_ACTIVE,
    disk::ROW_READ,
    disk::ROW_WRITE,
    disk::ROW_TOTAL_READ,
    disk::ROW_TOTAL_WRITTEN,
    disk::ROW_CAPACITY,
    disk::ROW_USED,
    disk::ROW_FREE,
    ROW_DISKS,
    ROW_TYPE,
];

/// The Details rows of the Aggregate graphics page, in grid order.
pub const GPU_ROWS: [&str; 4] = [
    gpu::ROW_UTILIZATION,
    gpu::ROW_DEDICATED,
    ROW_ADAPTERS,
    ROW_TYPE,
];

// ---- members ----

/// The present whole disks (no partitions), in catalog order.
fn member_disks(snap: &Snapshot) -> impl Iterator<Item = &DeviceEntry> {
    snap.devices.entries.iter().filter(|e| {
        e.present && e.parent.is_none() && matches!(e.identity, DeviceIdentity::Disk { .. })
    })
}

/// The present GPUs, in catalog order.
fn member_gpus(snap: &Snapshot) -> impl Iterator<Item = &DeviceEntry> {
    snap.devices
        .entries
        .iter()
        .filter(|e| e.present && matches!(e.identity, DeviceIdentity::Gpu { .. }))
}

fn disk_name(e: &DeviceEntry) -> &str {
    match &e.identity {
        DeviceIdentity::Disk { name, .. } => name,
        _ => &e.display_name,
    }
}

/// The value of a metric when it is shown.
fn shown<T: Copy + Default>(m: Metric<T>) -> Option<T> {
    matches!(m.validity, Validity::Valid | Validity::Stale).then_some(m.value)
}

fn shown_inv(f: InvField<u64>) -> Option<u64> {
    matches!(f.validity, Validity::Valid | Validity::Stale).then_some(f.value)
}

/// One row from an optional value: formats on a bits change, `None` reads "Collecting…".
fn value_row<T: Copy>(
    model: &mut KeyValueModel,
    label: &'static str,
    v: Option<T>,
    bits: impl Fn(T) -> u64,
    write: impl FnOnce(&mut String, T),
) -> bool {
    match v {
        Some(v) => model.set_valid(label, bits(v), |o| write(o, v)),
        None => model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None),
    }
}

/// `Some(max)` of the values, `None` for no value.
fn max_f32(it: impl Iterator<Item = f32>) -> Option<f32> {
    it.fold(None, |m, v| Some(m.map_or(v, |m| m.max(v))))
}

/// The sum of the values, `None` for no value.
fn sum_opt<T: std::ops::Add<Output = T> + Copy>(it: impl Iterator<Item = T>) -> Option<T> {
    it.fold(None, |s, v| Some(s.map_or(v, |s| s + v)))
}

/// The common word of `words`; `multiple` when they differ; `None` when there is none.
fn common<'a>(mut words: impl Iterator<Item = &'a str>) -> Option<Common<'a>> {
    let first = words.next()?;
    Some(if words.all(|w| w == first) {
        Common::One(first)
    } else {
        Common::Many
    })
}

enum Common<'a> {
    One(&'a str),
    Many,
}

// ---- Aggregate storage rows ----

/// The disk model: ten rows in order, with the kind words the Type row uses.
pub fn disk_model() -> KeyValueModel {
    let mut texts: Vec<(&'static str, &'static [&'static str])> = vec![(KEY_MULTIPLE, &[])];
    texts.extend(KIND_KEYS.iter().map(|(_, key)| (*key, &[][..])));
    KeyValueModel::new(&DISK_ROWS).with_texts(&texts)
}

/// Fills the storage rows from the member disks. True when any row changed.
pub fn update_disk_rows(model: &mut KeyValueModel, snap: &Snapshot) -> bool {
    let totals: Vec<_> = member_disks(snap)
        .filter_map(|e| {
            snap.system
                .disks
                .iter()
                .find(|(i, _)| *i == e.idx)
                .map(|(_, t)| t)
        })
        .collect();
    let details = |idx: DeviceIdx| {
        snap.device_details
            .disks
            .iter()
            .find(|(i, _)| *i == idx)
            .map(|(_, d)| d)
    };
    let f32_bits = |v: f32| u64::from(v.to_bits());
    let mut changed = false;
    changed |= value_row(
        model,
        disk::ROW_ACTIVE,
        max_f32(totals.iter().filter_map(|t| shown(t.active_pct))),
        f32_bits,
        units::percent,
    );
    changed |= value_row(
        model,
        disk::ROW_READ,
        sum_opt(totals.iter().filter_map(|t| shown(t.read_bps))),
        f64::to_bits,
        units::rate_iec_auto,
    );
    changed |= value_row(
        model,
        disk::ROW_WRITE,
        sum_opt(totals.iter().filter_map(|t| shown(t.write_bps))),
        f64::to_bits,
        units::rate_iec_auto,
    );
    let bytes = |model: &mut KeyValueModel, label, v: Option<u64>| {
        value_row(model, label, v, |n| n, units::bytes)
    };
    changed |= bytes(
        model,
        disk::ROW_TOTAL_READ,
        sum_opt(totals.iter().map(|t| t.read_total)),
    );
    changed |= bytes(
        model,
        disk::ROW_TOTAL_WRITTEN,
        sum_opt(totals.iter().map(|t| t.written_total)),
    );
    changed |= bytes(
        model,
        disk::ROW_CAPACITY,
        sum_opt(totals.iter().map(|t| t.capacity)),
    );
    let used = sum_opt(member_disks(snap).filter_map(|e| shown_inv(details(e.idx)?.used_bytes)));
    let free = sum_opt(member_disks(snap).filter_map(|e| shown_inv(details(e.idx)?.free_bytes)));
    for (label, v) in [(disk::ROW_USED, used), (disk::ROW_FREE, free)] {
        changed |= match v {
            Some(n) => model.set_valid(label, n, |o| units::bytes(o, n)),
            None => model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None),
        };
    }

    let mut names = String::new();
    for e in member_disks(snap) {
        if !names.is_empty() {
            names.push_str(", ");
        }
        names.push_str(disk_name(e));
    }
    changed |= model.set_text(ROW_DISKS, &names);

    let kinds = member_disks(snap)
        .filter_map(|e| details(e.idx).map(|d| &*d.kind).filter(|k| !k.is_empty()));
    changed |= match common(kinds) {
        Some(Common::One(kind)) => match kind_key(kind) {
            Some(key) => {
                let word = model.text(key);
                model.set_text(ROW_TYPE, word.text())
            }
            None => {
                model.set_fallback(ROW_TYPE, Validity::Unsupported, Fallback::NotReported, None)
            }
        },
        Some(Common::Many) => {
            let word = model.text(KEY_MULTIPLE);
            model.set_text(ROW_TYPE, word.text())
        }
        None => model.set_fallback(ROW_TYPE, Validity::Unsupported, Fallback::NotReported, None),
    };
    changed
}

// ---- Aggregate graphics rows ----

/// The GPU model: four rows in order, with the Multiple word.
pub fn gpu_model() -> KeyValueModel {
    KeyValueModel::new(&GPU_ROWS).with_texts(&[(KEY_MULTIPLE, &[])])
}

/// The summed VRAM of the member GPUs: the adapter's dedicated size, else the NVML total.
fn gpu_memory_total(snap: &Snapshot) -> u64 {
    member_gpus(snap)
        .filter_map(|e| {
            gpu::adapter(snap, e.idx)
                .and_then(|a| shown_inv(a.dedicated_bytes))
                .or_else(|| gpu::gpu_totals(snap, e.idx).and_then(|t| shown(t.mem_total)))
        })
        .sum()
}

/// Fills the graphics rows from the member GPUs. True when any row changed.
pub fn update_gpu_rows(model: &mut KeyValueModel, snap: &Snapshot) -> bool {
    let totals: Vec<_> = member_gpus(snap)
        .filter_map(|e| gpu::gpu_totals(snap, e.idx))
        .collect();
    let mut changed = value_row(
        model,
        gpu::ROW_UTILIZATION,
        max_f32(totals.iter().filter_map(|t| shown(t.util_pct))),
        |v| u64::from(v.to_bits()),
        units::percent,
    );
    let used = sum_opt(totals.iter().filter_map(|t| shown(t.mem_used)));
    let total = gpu_memory_total(snap);
    changed |= match used {
        Some(used) if total > 0 => model.set_valid(
            gpu::ROW_DEDICATED,
            used.wrapping_mul(31).wrapping_add(total),
            |o| units::bytes_of(o, used, total),
        ),
        Some(_) => model.set_fallback(
            gpu::ROW_DEDICATED,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        ),
        None => model.set_fallback(
            gpu::ROW_DEDICATED,
            Validity::WarmingUp,
            Fallback::Unavailable,
            None,
        ),
    };

    let mut names = String::new();
    for e in member_gpus(snap) {
        if !names.is_empty() {
            names.push_str(", ");
        }
        names.push_str(&e.display_name);
    }
    changed |= model.set_text(ROW_ADAPTERS, &names);

    // The vendor is the first word of the adapter name, as on the sidebar card.
    let vendors = member_gpus(snap).filter_map(|e| e.display_name.split_whitespace().next());
    changed |= match common(vendors) {
        Some(Common::One(vendor)) => model.set_text(ROW_TYPE, vendor),
        Some(Common::Many) => {
            let word = model.text(KEY_MULTIPLE);
            model.set_text(ROW_TYPE, word.text())
        }
        None => model.set_fallback(ROW_TYPE, Validity::Unsupported, Fallback::NotReported, None),
    };
    changed
}

// ---- shared page parts ----

/// The header of `kind` from the catalog entry of `idx`. `None` without the entry.
fn header_of(
    kind: DeviceKind,
    idx: DeviceIdx,
    snap: &Snapshot,
    cx: &mut App,
) -> Option<(SharedString, SharedString)> {
    let entry = snap.devices.entries.iter().find(|e| e.idx == idx)?;
    Some(header_text(page_words(cx), kind, 0, entry, snap))
}

/// `chart` in a full-width box of `height`.
fn chart_box(id: &'static str, chart: &Entity<ChartView>, height: f32) -> Div {
    div()
        .debug_selector(move || id.into())
        .flex_none()
        .w_full()
        .h(px(height))
        .child(chart.clone().cached(StyleRefinement::default().size_full()))
}

fn copy_header(header: &(SharedString, SharedString), details: &KeyValueModel, out: &mut String) {
    let (word, model) = header;
    out.push_str(word);
    if !model.is_empty() {
        out.push(' ');
        out.push_str(model);
    }
    out.push('\n');
    details.copy_into(out);
}

fn send_demand(
    demand: &Entity<DemandAggregator>,
    id: EntityId,
    window: Option<AnyWindowHandle>,
    part: Demand,
    cx: &mut App,
) {
    demand.update(cx, |d, cx| d.set(id, window, part, cx));
}

// ---- Aggregate storage page ----

/// The Aggregate storage page.
pub struct AggregateDiskPage {
    key: DeviceKey,
    row: DeviceRow,
    active: Entity<ChartView>,
    transfer: Entity<ChartView>,
    details: KeyValueModel,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    words: disk::Words,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    is_active: bool,
    window: Option<AnyWindowHandle>,
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl AggregateDiskPage {
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
        let mut words = disk::Words::default();
        words.refresh(cx);

        let idx = row.idx;
        let active = {
            let (spec, labels) = (
                disk::active_spec(idx, &settings, &theme),
                disk::active_labels(&words),
            );
            cx.new(|cx| {
                ChartView::new(spec, labels, &hub, cx)
                    .with_newest_marks()
                    .with_device_history(&snap)
            })
        };
        let transfer = {
            let (spec, labels) = (
                transfer_spec(idx, &settings, &theme),
                disk::transfer_labels(&words),
            );
            cx.new(|cx| {
                let mut view = ChartView::new(spec, labels, &hub, cx)
                    .with_newest_marks()
                    .with_device_history(&snap);
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
            details: disk_model(),
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

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// This page's demand part while it is active: disks, and the device metric history at
    /// 1 h, 24 h and 7 d.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            disks: true,
            device_history_range: page_history_request(self.settings.range_ns()),
            ..Demand::default()
        }
    }

    fn send_demand(&self, cx: &mut App) {
        if self.is_active {
            send_demand(&self.demand, self.id, self.window, self.demand_part(), cx);
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

    /// Brings the header, the rows and the chart state up to date with the components in
    /// `mask`. Notifies only when something the page draws changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let mut changed = false;
        if mask.contains(ComponentMask::HEALTH) {
            let state = health_state(snap);
            for chart in [&self.active, &self.transfer] {
                let state = state.clone();
                chart.update(cx, |v, cx| v.set_state(state, cx));
            }
        }
        if mask.intersects(
            ComponentMask::SYSTEM
                | ComponentMask::HEALTH
                | ComponentMask::DEVICE_DETAILS
                | ComponentMask::DEVICES,
        ) {
            changed |= update_disk_rows(&mut self.details, snap);
        }
        if mask.intersects(ComponentMask::DEVICES.union(ComponentMask::INVENTORY)) {
            changed |= self.sync_header(snap, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// The header from the catalog. True when it changed.
    fn sync_header(&mut self, snap: &Snapshot, cx: &mut App) -> bool {
        let Some(header) = header_of(DeviceKind::AggregateDisk, self.row.idx, snap, cx) else {
            return false;
        };
        if header == self.header {
            return false;
        }
        self.header = header;
        true
    }

    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        if new != self.settings {
            let range_moved = new.range_ns() != self.settings.range_ns();
            self.settings = new;
            self.rebuild_specs(cx);
            if range_moved {
                // The header switch shows the new range.
                self.send_demand(cx);
                cx.notify();
            }
        }
    }

    fn rebuild_specs(&self, cx: &mut App) {
        let (idx, s) = (self.row.idx, self.settings);
        let theme = current(cx).clone();
        let active = disk::active_spec(idx, &s, &theme);
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
        let (active, transfer) = (
            disk::active_labels(&self.words),
            disk::transfer_labels(&self.words),
        );
        self.active.update(cx, |v, cx| v.set_labels(active, cx));
        self.transfer.update(cx, |v, cx| v.set_labels(transfer, cx));
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_header(&snap, cx);
        cx.notify();
    }
}

impl DevicePage for AggregateDiskPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        let h = GRAPH_H_STACKED * m.ui_scale;
        v_flex()
            .w_full()
            .gap(m.sp[4])
            .child(chart_box(DISK_ACTIVE_ID, &self.active, h))
            .child(chart_box(DISK_TRANSFER_ID, &self.transfer, h))
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
    }

    fn copy_text(&self, out: &mut String) {
        copy_header(&self.header, &self.details, out);
    }

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

impl Focusable for AggregateDiskPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AggregateDiskPage {
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

// ---- Aggregate graphics page ----

/// The words of the Aggregate graphics charts.
#[derive(Clone, Debug, Default)]
struct GpuWords {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    utilization: SharedString,
    /// The Memory chart's words, in the GPU page's type.
    memory: gpu::Words,
}

impl GpuWords {
    /// Translates every word when the strings changed since the last call. True when it did.
    fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return false;
        }
        self.version = version;
        self.utilization = tr(cx, KEY_UTILIZATION);
        self.memory.memory = tr(cx, KEY_MEMORY);
        self.memory.memory_usage = tr(cx, KEY_MEMORY_USAGE);
        self.memory.dedicated = tr(cx, KEY_DEDICATED);
        self.memory.shared = tr(cx, KEY_SHARED);
        true
    }
}

/// Utilization of the busiest adapter with fill, 0..100.
fn utilization_spec(
    device: DeviceIdx,
    s: &PageSettings,
    theme: &TlTheme,
) -> crate::chart::ChartSpec {
    let series = smallvec![ChartSeries {
        id: tl_model::ids::SeriesId {
            device,
            metric: MetricId::GPU_UTIL,
        },
        fill: Some(theme.graph.gpu_fill),
        line: theme.graph.gpu_line,
        ..ChartSeries::default()
    }];
    let y = YScale::Fixed {
        min: 0.0,
        max: 100.0,
    };
    disk::spec(series, y, s)
}

/// The summed VRAM chart over the shared range.
fn aggregate_memory_spec(
    device: DeviceIdx,
    total: u64,
    s: &PageSettings,
    theme: &TlTheme,
) -> crate::chart::ChartSpec {
    let mut spec = gpu::memory_spec(device, false, total, theme);
    spec.range_ns = s.range_ns();
    spec
}

fn utilization_labels(w: &GpuWords) -> ChartLabels {
    ChartLabels {
        heading: Some(w.utilization.clone()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: w.utilization.clone(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The Aggregate graphics page.
pub struct AggregateGpuPage {
    key: DeviceKey,
    row: DeviceRow,
    utilization: Entity<ChartView>,
    memory: Entity<ChartView>,
    /// The summed VRAM the Memory chart is scaled to.
    memory_total: u64,
    details: KeyValueModel,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    words: GpuWords,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    is_active: bool,
    window: Option<AnyWindowHandle>,
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl AggregateGpuPage {
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
        let mut words = GpuWords::default();
        words.refresh(cx);

        let idx = row.idx;
        let memory_total = gpu_memory_total(&snap);
        let utilization = {
            let (spec, labels) = (
                utilization_spec(idx, &settings, &theme),
                utilization_labels(&words),
            );
            cx.new(|cx| {
                ChartView::new(spec, labels, &hub, cx)
                    .with_newest_marks()
                    .with_device_history(&snap)
            })
        };
        let memory = {
            let spec = aggregate_memory_spec(idx, memory_total, &settings, &theme);
            let labels = gpu::memory_labels(false, &words.memory);
            cx.new(|cx| {
                ChartView::new(spec, labels, &hub, cx)
                    .with_newest_marks()
                    .with_device_history(&snap)
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
            utilization,
            memory,
            memory_total,
            details: gpu_model(),
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

    pub fn utilization_chart(&self) -> &Entity<ChartView> {
        &self.utilization
    }

    pub fn memory_chart(&self) -> &Entity<ChartView> {
        &self.memory
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// This page's demand part while it is active: the GPU component, the inventory for the
    /// adapters' VRAM sizes, and the device metric history at 1 h, 24 h and 7 d.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            gpus: true,
            inventory: true,
            device_history_range: page_history_request(self.settings.range_ns()),
            ..Demand::default()
        }
    }

    fn send_demand(&self, cx: &mut App) {
        if self.is_active {
            send_demand(&self.demand, self.id, self.window, self.demand_part(), cx);
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

    /// Brings the header, the rows and the Memory scale up to date with the components in
    /// `mask`. Notifies only when something the page draws changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let mut changed = false;
        if mask.intersects(
            ComponentMask::SYSTEM
                | ComponentMask::INVENTORY
                | ComponentMask::DEVICES
                | ComponentMask::HEALTH,
        ) {
            changed |= update_gpu_rows(&mut self.details, snap);
            let total = gpu_memory_total(snap);
            if total != self.memory_total {
                self.memory_total = total;
                self.rebuild_memory(cx);
            }
        }
        if mask.intersects(ComponentMask::DEVICES.union(ComponentMask::INVENTORY)) {
            changed |= self.sync_header(snap, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// The header from the catalog. True when it changed.
    fn sync_header(&mut self, snap: &Snapshot, cx: &mut App) -> bool {
        let Some(header) = header_of(DeviceKind::AggregateGpu, self.row.idx, snap, cx) else {
            return false;
        };
        if header == self.header {
            return false;
        }
        self.header = header;
        true
    }

    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        if new != self.settings {
            let range_moved = new.range_ns() != self.settings.range_ns();
            self.settings = new;
            self.rebuild_specs(cx);
            if range_moved {
                // The header switch shows the new range.
                self.send_demand(cx);
                cx.notify();
            }
        }
    }

    fn rebuild_memory(&self, cx: &mut App) {
        let theme = current(cx).clone();
        let spec = aggregate_memory_spec(self.row.idx, self.memory_total, &self.settings, &theme);
        self.memory.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    fn rebuild_specs(&self, cx: &mut App) {
        let theme = current(cx).clone();
        let spec = utilization_spec(self.row.idx, &self.settings, &theme);
        self.utilization.update(cx, |v, cx| v.set_spec(spec, cx));
        self.rebuild_memory(cx);
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
        let utilization = utilization_labels(&self.words);
        let memory = gpu::memory_labels(false, &self.words.memory);
        self.utilization
            .update(cx, |v, cx| v.set_labels(utilization, cx));
        self.memory.update(cx, |v, cx| v.set_labels(memory, cx));
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_header(&snap, cx);
        cx.notify();
    }
}

impl DevicePage for AggregateGpuPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        let h = GRAPH_H_STACKED * m.ui_scale;
        v_flex()
            .w_full()
            .gap(m.sp[4])
            .child(chart_box(GPU_UTILIZATION_ID, &self.utilization, h))
            .child(chart_box(GPU_MEMORY_ID, &self.memory, h))
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
    }

    fn copy_text(&self, out: &mut String) {
        copy_header(&self.header, &self.details, out);
    }

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

impl Focusable for AggregateGpuPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AggregateGpuPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
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
