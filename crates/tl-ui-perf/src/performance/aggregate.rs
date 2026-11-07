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
//!
//! Both pages have a widget container as their body (ADR-019 phase 3, `tasklens-hzire.3` and
//! `tasklens-hzire.9`).

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use smallvec::SmallVec;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{InvField, Snapshot};
use tl_model::system::Metric;
use tl_model::units;
use tl_model::validity::Validity;
use tl_ui_settings::layout::{
    ContainerLayout, FixedWidget, WidgetSetting, container_layout, reset_to_default,
};
use tl_ui_settings::layout_boxes::DeviceKind as KeyKind;
use tl_ui_settings::layout_pages::{page_container, page_key};

use super::actions::{PerfWidgetEditStart, PerfWidgetResetPage};
use super::details::{Fallback, KeyValueModel};
use super::disk::{self, KIND_KEYS, PageSettings, kind_key, section_versions};
use super::gpu;
use super::page::{
    DevicePage, DevicePageFrame, PerfSidebar, details_draw, header_text, page_history_request,
    page_words,
};
use super::page_flow::{column_flow, default_part};
use super::widget::container::WidgetContainer;
use super::widget::details_widget::{DetailsResized, DetailsShown, DetailsView};
use super::widget::edit::{WidgetEditor, any_editing, default_layout, saved_layout};
use super::widget::fixed_widget::FixedContent;
use super::widget::menu::confirm_reset_page;
use super::widget::remap::{find_present, kind_rows, remap_kind};
use super::widget::source::SourceStore;
use super::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use tl_ui_base::i18n::Strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::theme::{current, metrics};
use tl_ui_kit::components::menu::TlMenu;

pub const ROW_DISKS: &str = "perf-agg-row-disks";
pub const ROW_ADAPTERS: &str = "perf-agg-row-adapters";
pub const ROW_TYPE: &str = "perf-agg-row-type";

const KEY_MULTIPLE: &str = "perf-card-multiple";

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

/// The Aggregate storage page. Its body is one widget container (ADR-019 phase 3,
/// `tasklens-hzire.3`): by default the Active time and Transfer rate charts and the Details
/// widget, in the 1.3.0 places (`page_flow::column_flow`). Edit Widgets changes them; the page
/// keeps the header and the Details model, and hands the model's rows to its `DetailsView`.
///
/// Render contract: the page renders on a header change, a range change and a layout change,
/// never because of a sample. A Details value change notifies only the `DetailsView`.
pub struct AggregateDiskPage {
    key: DeviceKey,
    row: DeviceRow,
    /// The container id, `page.aggregate:disk`.
    container_id: SharedString,
    details: KeyValueModel,
    details_view: Entity<DetailsView>,
    container: Entity<WidgetContainer>,
    editor: Entity<WidgetEditor>,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    is_active: bool,
    window: Option<AnyWindowHandle>,
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 8]>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

/// The strings version, `None` without strings (tests).
pub(super) fn strings_version(cx: &App) -> Option<u64> {
    cx.try_global::<Strings>().map(|s| s.strings_version)
}

/// The layout `container` shows now: its saved entry, else its built-in default.
pub(super) fn page_layout(container: &str, cx: &App) -> ContainerLayout {
    cx.has_global::<TlSettings>()
        .then(|| container_layout(&TlSettings::settings(cx).performance, container))
        .flatten()
        .unwrap_or_else(|| default_layout(container))
}

/// The layout a device page shows on the host its hub reads, and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageLayout {
    pub layout: ContainerLayout,
    /// The local page container a remote page borrowed its layout from. Edit Widgets and Reset
    /// Page save there, so the layout stays one shared layout.
    pub borrowed: Option<SharedString>,
    /// The local key at the page's position and the page's own key (the page pin of `remap`).
    pub pin: Option<(Arc<str>, Arc<str>)>,
}

/// The layout of device page `container` on the host of `hub` (tasklens-jmc36, leader decision
/// 2026-11-07). On the local host, and for a page whose key is the same on every host, it is
/// [`page_layout`]. On a remote host a page whose key has no saved entry and is not a local
/// device borrows the local page of the same-kind same-order device, with that local key
/// pinned to the page's own device. A saved entry under the remote key wins. Never saves.
pub(super) fn page_layout_on(container: &str, hub: &Entity<SnapshotHub>, cx: &App) -> PageLayout {
    let own = || PageLayout {
        layout: page_layout(container, cx),
        borrowed: None,
        pin: None,
    };
    if !hub.read(cx).clock().is_remote() || saved_layout(container, cx).is_some() {
        return own();
    }
    let Some(key) = page_key(container) else {
        return own();
    };
    let Some(kind) = remap_kind(KeyKind::of_key(key)) else {
        return own();
    };
    let Some(local) = HostContext::local_hub(cx).map(|h| Arc::clone(h.read(cx).current())) else {
        return own();
    };
    if find_present(&local, key).is_some() {
        return own();
    }
    let perf = cx
        .try_global::<TlSettings>()
        .map(|_| &TlSettings::settings(cx).performance);
    let shown = Arc::clone(hub.read(cx).current());
    let Some(n) = kind_rows(&shown, perf, kind)
        .iter()
        .position(|row| *row.key.0 == *key)
    else {
        return own();
    };
    let Some(row) = kind_rows(&local, perf, kind).into_iter().nth(n) else {
        return own();
    };
    let from = page_container(&row.key.0);
    PageLayout {
        layout: page_layout(&from, cx),
        borrowed: Some(SharedString::from(from)),
        pin: Some((row.key.0, Arc::from(key))),
    }
}

/// Shows `page` in a device page's `container` and points its `editor` at the container it
/// saves to. Call it only while the page is not in Edit Widgets.
pub(super) fn show_page_layout(
    container: &Entity<WidgetContainer>,
    editor: &Entity<WidgetEditor>,
    page: PageLayout,
    cx: &mut App,
) {
    editor.update(cx, |e, _| e.set_save_container(page.borrowed));
    container.update(cx, |c, cx| {
        c.set_pin(page.pin, cx);
        c.set_layout(page.layout, cx);
    });
}

/// A device page's source store, with the page pin of `page` set.
pub(super) fn page_store(
    hub: &Entity<SnapshotHub>,
    page: &PageLayout,
    cx: &mut App,
) -> Rc<RefCell<SourceStore>> {
    let mut store = SourceStore::for_page(hub);
    store.set_pin(page.pin.clone(), cx);
    Rc::new(RefCell::new(store))
}

impl AggregateDiskPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = PageSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let container_id = SharedString::from(page_container(&key.0));

        let mut details = disk_model();
        details.refresh_strings(cx);
        update_disk_rows(&mut details, &snap);
        let details_view = {
            let shown = DetailsShown::of(&details, strings_version(cx));
            let draw = details_draw(cx.weak_entity());
            let id = container_id.clone();
            cx.new(|_| DetailsView::new(id, shown, draw))
        };
        let natural = details_view.read(cx).natural_height();
        let content: FixedContent = {
            let view: AnyView = details_view.clone().into();
            Rc::new(move |kind: &FixedWidget, _: &mut App| {
                matches!(kind, FixedWidget::Details).then(|| view.clone())
            })
        };
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::for_page(&hub)));
            let layout = page_layout(&container_id, cx);
            let id = container_id.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut c = WidgetContainer::new(id, layout, &hub, store, cx);
                let place = column_flow(Rc::new(move |w: &WidgetSetting| {
                    Some(default_part(w, &natural))
                }));
                c.set_flow_placer(Some(place), cx);
                c.set_fixed_content(Some(content), cx);
                c
            })
        };
        let editor = {
            let container = container.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut e = WidgetEditor::new(container, &hub, cx);
                e.set_page_parts(vec![FixedWidget::Details]);
                e
            })
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        // The Details grid got taller or shorter: the parts under it move.
        subs.push(
            cx.subscribe(&details_view, |this, _, _: &DetailsResized, cx| {
                this.container.update(cx, |c, cx| {
                    c.place_again(cx);
                });
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
            container_id,
            details,
            details_view,
            container,
            editor,
            header: (SharedString::default(), SharedString::default()),
            settings,
            settings_versions: section_versions(cx),
            hub,
            demand,
            id,
            is_active: false,
            window: None,
            focus: cx.focus_handle(),
            _subs: subs,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
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

    /// The container id, `page.aggregate:disk`.
    pub fn container_id(&self) -> &SharedString {
        &self.container_id
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// This page's demand part while it is active: disks, the device metric history at 1 h,
    /// 24 h and 7 d, and what the placed widgets read.
    pub fn demand_part(&self, cx: &App) -> Demand {
        let mut demand = Demand {
            any_surface_visible: true,
            disks: true,
            device_history_range: page_history_request(self.settings.range_ns()),
            ..Demand::default()
        };
        add_demand(&mut demand, &self.container.read(cx).demand_part());
        demand
    }

    fn send_demand(&self, cx: &mut App) {
        if self.is_active {
            let part = self.demand_part(cx);
            send_demand(&self.demand, self.id, self.window, part, cx);
        }
    }

    fn notify(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
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

    /// Brings the header and the Details rows up to date with the components in `mask`. Only
    /// a header change renders the page; a row change goes to the Details view. A failed disk
    /// collector reaches the chart widgets through their sources (`SourceData::chart_state`),
    /// so a chart added in Edit Widgets shows "Failed" too.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        if mask.intersects(
            ComponentMask::SYSTEM
                | ComponentMask::HEALTH
                | ComponentMask::DEVICE_DETAILS
                | ComponentMask::DEVICES,
        ) && update_disk_rows(&mut self.details, snap)
        {
            self.sync_details(cx);
        }
        if mask.intersects(ComponentMask::DEVICES.union(ComponentMask::INVENTORY))
            && self.sync_header(snap, cx)
        {
            self.notify(cx);
        }
        // The aggregate went away: an edit ends as Cancel does.
        if mask.contains(ComponentMask::DEVICES)
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

    /// The header from the catalog. True when it changed.
    fn sync_header(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let Some(header) = header_of(DeviceKind::AggregateDisk, self.row.idx, snap, cx) else {
            return false;
        };
        if header == self.header {
            return false;
        }
        let word = header.0.clone();
        self.editor.update(cx, |e, _| e.set_page_name(word));
        self.header = header;
        true
    }

    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        // A saved layout (Apply, Reset Page) shows when the page is not in Edit Widgets.
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
        if new != self.settings {
            let range_moved = new.range_ns() != self.settings.range_ns();
            self.settings = new;
            if range_moved {
                // The header switch shows the new range.
                self.send_demand(cx);
                self.notify(cx);
            }
        }
    }

    /// New strings: the Details words and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        self.details.refresh_strings(cx);
        self.sync_details(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_header(&snap, cx);
        self.notify(cx);
    }

    /// "Edit Widgets…" in the page menu: Select Pages ends as "Done" ends it, then the page
    /// starts Edit Widgets. Nothing while another container of the window is in it.
    fn on_edit_start(
        &mut self,
        _: &PerfWidgetEditStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        start_page_edit(&self.editor, &self.hub, window, cx);
    }

    /// "Reset Page" in the page menu (ADR-019 phase 3 "Reset").
    fn on_reset_page(
        &mut self,
        _: &PerfWidgetResetPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        reset_page(&self.container_id, self.header.0.clone(), window, cx);
    }
}

/// Starts Edit Widgets on a device page's `editor`: the sidebar of the same window leaves
/// Select Pages first (ADR-019 phase 3 "Select Pages and Edit Layout").
pub fn start_page_edit(
    editor: &Entity<WidgetEditor>,
    hub: &Entity<SnapshotHub>,
    window: &mut Window,
    cx: &mut App,
) {
    if any_editing(hub, cx) {
        return;
    }
    let sidebar = cx
        .try_global::<PerfSidebar>()
        .and_then(|s| s.0.upgrade())
        .filter(|s| s.read(cx).hub() == hub);
    if let Some(sidebar) = sidebar {
        sidebar.update(cx, |s, cx| s.end_select_pages(cx));
    }
    editor.update(cx, |e, cx| e.start(window, cx));
}

/// "Reset Page": the default of page `container` is saved at once. With a grid layout it asks
/// first, since the user's widgets cannot come back. Nothing while the page has no entry.
pub fn reset_page(container: &str, page: SharedString, window: &mut Window, cx: &mut App) {
    let Some(saved) = saved_layout(container, cx) else {
        return;
    };
    let container: Arc<str> = Arc::from(container);
    let save = move |_: &mut Window, cx: &mut App| {
        if cx.has_global::<TlSettings>() {
            TlSettings::update(cx, |d| {
                reset_to_default(&mut d.settings.performance, &container);
            });
        }
    };
    if saved.preset.is_some() {
        save(window, cx);
    } else {
        confirm_reset_page(page, true, window, cx, save);
    }
}

/// Adds the records `part` asks for to `demand`.
pub(super) fn add_demand(demand: &mut Demand, part: &Demand) {
    demand.memory |= part.memory;
    demand.disks |= part.disks;
    demand.nets |= part.nets;
    demand.gpus |= part.gpus;
    demand.sensors |= part.sensors;
    demand.cpu_fast |= part.cpu_fast;
    demand.process_top |= part.process_top;
    demand.device_timeline |= part.device_timeline;
    if !demand.cpu.wants_total() {
        demand.cpu = part.cpu;
    }
}

impl DevicePage for AggregateDiskPage {
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
        // The charts are widgets of the container.
        div().into_any_element()
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

    fn widget_editor(&self) -> Option<&Entity<WidgetEditor>> {
        Some(&self.editor)
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

// ---- Aggregate graphics page ----

/// The Aggregate graphics page. Its body is one widget container (ADR-019 phase 3,
/// `tasklens-hzire.9`): by default the Utilization and Memory charts and the Details widget, in
/// the 1.3.0 places (`page_flow::column_flow`). Edit Widgets changes them; the page keeps the
/// header and the Details model, and hands the model's rows to its `DetailsView`.
///
/// Render contract: the page renders on a header change, a range change and a layout change,
/// never because of a sample. A Details value change notifies only the `DetailsView`.
pub struct AggregateGpuPage {
    key: DeviceKey,
    row: DeviceRow,
    /// The container id, `page.aggregate:gpu`.
    container_id: SharedString,
    details: KeyValueModel,
    details_view: Entity<DetailsView>,
    container: Entity<WidgetContainer>,
    editor: Entity<WidgetEditor>,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    is_active: bool,
    window: Option<AnyWindowHandle>,
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 8]>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl AggregateGpuPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = PageSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let container_id = SharedString::from(page_container(&key.0));

        let mut details = gpu_model();
        details.refresh_strings(cx);
        update_gpu_rows(&mut details, &snap);
        let details_view = {
            let shown = DetailsShown::of(&details, strings_version(cx));
            let draw = details_draw(cx.weak_entity());
            let id = container_id.clone();
            cx.new(|_| DetailsView::new(id, shown, draw))
        };
        let natural = details_view.read(cx).natural_height();
        let content: FixedContent = {
            let view: AnyView = details_view.clone().into();
            Rc::new(move |kind: &FixedWidget, _: &mut App| {
                matches!(kind, FixedWidget::Details).then(|| view.clone())
            })
        };
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::for_page(&hub)));
            let layout = page_layout(&container_id, cx);
            let id = container_id.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut c = WidgetContainer::new(id, layout, &hub, store, cx);
                let place = column_flow(Rc::new(move |w: &WidgetSetting| {
                    Some(default_part(w, &natural))
                }));
                c.set_flow_placer(Some(place), cx);
                c.set_fixed_content(Some(content), cx);
                c
            })
        };
        let editor = {
            let container = container.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut e = WidgetEditor::new(container, &hub, cx);
                e.set_page_parts(vec![FixedWidget::Details]);
                e
            })
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        // The Details grid got taller or shorter: the parts under it move.
        subs.push(
            cx.subscribe(&details_view, |this, _, _: &DetailsResized, cx| {
                this.container.update(cx, |c, cx| {
                    c.place_again(cx);
                });
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
            container_id,
            details,
            details_view,
            container,
            editor,
            header: (SharedString::default(), SharedString::default()),
            settings,
            settings_versions: section_versions(cx),
            hub,
            demand,
            id,
            is_active: false,
            window: None,
            focus: cx.focus_handle(),
            _subs: subs,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
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

    /// The container id, `page.aggregate:gpu`.
    pub fn container_id(&self) -> &SharedString {
        &self.container_id
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// This page's demand part while it is active: the GPU component, the inventory for the
    /// adapters' VRAM sizes, the device metric history at 1 h, 24 h and 7 d, and what the placed
    /// widgets read.
    pub fn demand_part(&self, cx: &App) -> Demand {
        let mut demand = Demand {
            any_surface_visible: true,
            gpus: true,
            inventory: true,
            device_history_range: page_history_request(self.settings.range_ns()),
            ..Demand::default()
        };
        add_demand(&mut demand, &self.container.read(cx).demand_part());
        demand
    }

    fn send_demand(&self, cx: &mut App) {
        if self.is_active {
            let part = self.demand_part(cx);
            send_demand(&self.demand, self.id, self.window, part, cx);
        }
    }

    fn notify(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
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

    /// Brings the header and the Details rows up to date with the components in `mask`. Only a
    /// header change renders the page; a row change goes to the Details view. The Memory
    /// chart's scale (the summed VRAM) is the widget's own.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        if mask.intersects(
            ComponentMask::SYSTEM
                | ComponentMask::INVENTORY
                | ComponentMask::DEVICES
                | ComponentMask::HEALTH,
        ) && update_gpu_rows(&mut self.details, snap)
        {
            self.sync_details(cx);
        }
        if mask.intersects(ComponentMask::DEVICES.union(ComponentMask::INVENTORY))
            && self.sync_header(snap, cx)
        {
            self.notify(cx);
        }
        // The aggregate went away: an edit ends as Cancel does.
        if mask.contains(ComponentMask::DEVICES)
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

    /// The header from the catalog. True when it changed.
    fn sync_header(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let Some(header) = header_of(DeviceKind::AggregateGpu, self.row.idx, snap, cx) else {
            return false;
        };
        if header == self.header {
            return false;
        }
        let word = header.0.clone();
        self.editor.update(cx, |e, _| e.set_page_name(word));
        self.header = header;
        true
    }

    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        // A saved layout (Apply, Reset Page) shows when the page is not in Edit Widgets.
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
        if new != self.settings {
            let range_moved = new.range_ns() != self.settings.range_ns();
            self.settings = new;
            if range_moved {
                // The header switch shows the new range.
                self.send_demand(cx);
                self.notify(cx);
            }
        }
    }

    /// New strings: the Details words and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        self.details.refresh_strings(cx);
        self.sync_details(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_header(&snap, cx);
        self.notify(cx);
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

    /// "Reset Page" in the page menu (ADR-019 phase 3 "Reset").
    fn on_reset_page(
        &mut self,
        _: &PerfWidgetResetPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        reset_page(&self.container_id, self.header.0.clone(), window, cx);
    }
}

impl DevicePage for AggregateGpuPage {
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
        // The charts are widgets of the container.
        div().into_any_element()
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

    fn widget_editor(&self) -> Option<&Entity<WidgetEditor>> {
        Some(&self.editor)
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
