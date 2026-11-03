//! The widget menu, the Add Widget list and the confirmations of Edit Widgets (UI-PRF-061).
//!
//! - **Sources**: the metrics of every GPU the box can show ([`source_groups`]). Per GPU: the
//!   engines it reports, in the GPU page's order and with its names, then VRAM, Graphics clock,
//!   Memory clock, Power draw and Temperature. A metric the GPU does not report is not listed.
//!   The CPU box lists the CPU's metrics instead ([`cpu_source_groups`], UI-PRF-061 phase 2):
//!   Utilization, Kernel time, Speed, Temperature, Package power, Core power, Power draw, Core
//!   clocks and CPU pressure, then a separator and "Core load >" and "Core speed >" with C0 to
//!   Cn; Add Widget adds "All cores >" (Load, Speed). The Memory box lists In use, Cached,
//!   Available, Swap, Commit, Memory pressure and Memory composition ([`mem_source_groups`]).
//!   The Disk box lists one submenu per shown disk (Active time, Read, Write, Read and write,
//!   Temperature) and "All disks >" (the same but Temperature, of the disk total); Add Widget
//!   adds "All disks >" (Active time, Read, Write, Read and write) ([`disk_source_groups`]).
//!   The Network box lists one submenu per counted adapter and "All adapters >" (of the counted
//!   total), each with Download, Upload and Download and upload; Add Widget adds "All adapters >"
//!   with the same three ([`net_source_groups`]).
//! - **Widget menu** ([`widget_menu`]): "Source >" (radio items, one submenu per GPU when the
//!   box shows two), "Show >" (check items for the forms the metric offers; the last checked one
//!   is disabled), "Chart >" while Chart is on (Fill under the line, Dashed line), a separator,
//!   then "Remove Widget".
//! - **Add Widget** ([`add_items`]): the same sources as plain items, then a separator and
//!   "All engines", one item per GPU (a group widget, ADR-019 phase 2), and a separator and the
//!   fixed widget the box allows while it has none ("Memory by app" in the Memory box).
//! - **Group widgets** (UI-PRF-061 phase 2): "Source >" lists only the metrics of the group's
//!   kind ([`group_metrics`]; none for engines), and after "Chart >" come "Items >" (Rows,
//!   Cells), "Arrange >" with Cells (Automatic, Stack vertically, Side by side) and
//!   "Show at most >" (All, 4, 8, 16, 32, and the saved number when it is another).
//!   [`set_group_option`] and [`new_group`] are the edits they make. A fixed widget's menu has
//!   only "Remove Widget".
//! - **Confirmations**: Reset in Edit Widgets ([`confirm_reset`]), Reset Box with a grid layout
//!   ([`confirm_reset_now`]) and a preset over a grid layout ([`confirm_preset`]). A device
//!   page asks "Reset the {$page} page?" ([`confirm_reset_page`]).
//! - **Device pages** (ADR-019 phase 3 "Source lists"): "Source >" and Add Widget list the
//!   metrics of the page's own device ([`page_source_groups`]), then the page parts it does not
//!   hold yet, Details first.
//!
//! The items dispatch the `PerfWidget*` actions to the editor's focus handle.

use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{App, SharedString, Window};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, FormSet, GROUP_LIMIT_RANGE, GroupItems, GroupKind,
    GroupOptions, GroupSelector, METRIC_CLOCK_GFX, METRIC_CLOCK_MEM, METRIC_ENGINE_ANY,
    METRIC_POWER, METRIC_TEMP, METRIC_VRAM, MetricKey, WidgetContent, WidgetOptions, WidgetSetting,
};
use tl_ui_settings::layout_boxes::{
    CPU_LOGICAL_PREFIX, DEVICE_CPU, DEVICE_DISKS, DEVICE_MEMORY, DEVICE_NETS, DeviceKind,
    METRIC_CORE_CLOCKS, METRIC_CPU_KERNEL, METRIC_CPU_TOTAL, METRIC_DISK_ACTIVE, METRIC_DISK_IO,
    METRIC_DISK_READ, METRIC_DISK_WRITE, METRIC_FREQ, METRIC_LOAD, METRIC_MEM_AVAILABLE,
    METRIC_MEM_CACHED, METRIC_MEM_COMMIT, METRIC_MEM_COMPOSITION, METRIC_MEM_SWAP, METRIC_MEM_USED,
    METRIC_NET_RX, METRIC_NET_TRAFFIC, METRIC_NET_TX, METRIC_POWER_CORES, METRIC_POWER_DRAW,
    METRIC_PRESSURE,
};
use tl_ui_settings::layout_pages::{
    METRIC_CLOCK, METRIC_COMMIT_PCT, METRIC_CORES_OVERLAID, METRIC_CORES_STACKED, METRIC_DIRTY,
    METRIC_FAN, METRIC_FREQ_MAX, METRIC_GPU_CLOCKS, METRIC_GPU_MEMORY, METRIC_PHYSICAL,
    METRIC_RESPONSE, METRIC_SIGNAL, METRIC_TOTAL_READ, METRIC_TOTAL_RX, METRIC_TOTAL_TX,
    METRIC_TOTAL_WRITTEN, METRIC_VRAM_SHARED,
};

use super::grid::{self, InnerGrid, SlotDevice};
use super::members::{Members, members};
use super::meta::MetricMeta;
use super::source::{box_reading, reading};
use crate::overview::layout::Arrange;
use tl_ui_base::i18n::{tr, tr_args};
use tl_ui_base::settings_global::TlSettings;
use tl_ui_kit::components::dialog::{DialogResult, TlDialog};
use tl_ui_kit::components::menu::TlMenu;
use tl_ui_perf_core::core::DeviceKey;
use tl_ui_perf_core::core::actions::{
    PerfWidgetAdd, PerfWidgetAddFixed, PerfWidgetAddGroup, PerfWidgetRemove, PerfWidgetSetForm,
    PerfWidgetSetGroup, PerfWidgetSetOption, PerfWidgetSetSource, WidgetSource,
};
use tl_ui_perf_core::core::gpu::{GpuVendor, gpu_totals, grid_engines};

/// The device facts after the engines, in menu order, with their name keys.
const OTHER_METRICS: [(&str, &str); 5] = [
    (METRIC_VRAM, "perf-overview-gpu-vram"),
    (METRIC_CLOCK_GFX, "perf-widget-metric-clock-gfx"),
    (METRIC_CLOCK_MEM, "perf-widget-metric-clock-mem"),
    (METRIC_POWER, "perf-widget-metric-power"),
    (METRIC_TEMP, "perf-widget-metric-temp"),
];

/// The CPU package's metrics in the CPU box's "Source >", in menu order, with their name keys
/// (UI-PRF-061 phase 2).
const CPU_METRICS: [(&str, &str); 9] = [
    (METRIC_CPU_TOTAL, "perf-widget-metric-cpu-total"),
    (METRIC_CPU_KERNEL, "perf-widget-metric-cpu-kernel"),
    (METRIC_FREQ, "perf-widget-metric-speed"),
    (METRIC_TEMP, "perf-widget-metric-temp"),
    (METRIC_POWER, "perf-widget-metric-cpu-power"),
    (METRIC_POWER_CORES, "perf-widget-metric-cpu-power-cores"),
    (METRIC_POWER_DRAW, "perf-widget-metric-power"),
    (METRIC_CORE_CLOCKS, "perf-widget-metric-core-clocks"),
    (METRIC_PRESSURE, "perf-widget-metric-cpu-pressure"),
];

/// The memory metrics in the Memory box's "Source >", in menu order, with their name keys
/// (UI-PRF-061 phase 2).
const MEM_METRICS: [(&str, &str); 7] = [
    (METRIC_MEM_USED, "perf-overview-mem-used"),
    (METRIC_MEM_CACHED, "perf-overview-mem-cached"),
    (METRIC_MEM_AVAILABLE, "perf-overview-mem-available"),
    (METRIC_MEM_SWAP, "perf-overview-mem-swap"),
    (METRIC_MEM_COMMIT, "perf-overview-mem-commit"),
    (METRIC_PRESSURE, "perf-widget-metric-mem-pressure"),
    (METRIC_MEM_COMPOSITION, "perf-widget-metric-mem-composition"),
];

/// The disk metrics in the Disk box's "Source >", in menu order, with their name keys
/// (UI-PRF-061 phase 2). The disk total lists the first four: it has no temperature.
const DISK_METRICS: [(&str, &str); 5] = [
    (METRIC_DISK_ACTIVE, "perf-widget-metric-disk-active"),
    (METRIC_DISK_READ, "perf-widget-metric-disk-read"),
    (METRIC_DISK_WRITE, "perf-widget-metric-disk-write"),
    (METRIC_DISK_IO, "perf-widget-metric-disk-io"),
    (METRIC_TEMP, "perf-widget-metric-temp"),
];

/// The form check items of "Show >", in menu order, with their name keys.
const FORM_ITEMS: [(FormSet, &str, &str); 5] = [
    (FormSet::CHART, "chart", "perf-widget-form-chart"),
    (FormSet::BAR, "bar", "perf-widget-form-bar"),
    (FormSet::VALUE, "value", "perf-widget-form-value"),
    (FormSet::STATS, "stats", "perf-widget-form-stats"),
    (FormSet::LABEL, "label", "perf-widget-form-label"),
];

/// The chart options of "Chart >": the option name and its name key.
pub const OPTION_FILL: &str = "fill";
pub const OPTION_DASHED: &str = "dashed";

/// The group options of [`PerfWidgetSetGroup`].
pub const GROUP_METRIC: &str = "metric";
pub const GROUP_ITEMS: &str = "items";
pub const GROUP_ARRANGE: &str = "arrange";
pub const GROUP_LIMIT: &str = "limit";
/// The `limit` value that shows every member.
pub const LIMIT_ALL: &str = "all";
/// The numbers of "Show at most >", after "All".
pub const LIMIT_CHOICES: [u16; 4] = [4, 8, 16, 32];

/// The metrics a group of `kind` lists in "Source >", in menu order, with their name keys:
/// cores Load and Speed; disks Active time, Read, Write, Read and write; adapters Download,
/// Upload, Download and upload. An engine group has one metric and lists none.
pub fn group_metrics(kind: &GroupKind) -> &'static [(&'static str, &'static str)] {
    match kind {
        GroupKind::Cores => &[
            (METRIC_LOAD, "perf-widget-core-load"),
            (METRIC_FREQ, "perf-widget-core-speed"),
        ],
        GroupKind::Disks => &[
            (METRIC_DISK_ACTIVE, "perf-widget-metric-disk-active"),
            (METRIC_DISK_READ, "perf-widget-metric-disk-read"),
            (METRIC_DISK_WRITE, "perf-widget-metric-disk-write"),
            (METRIC_DISK_IO, "perf-widget-metric-disk-io"),
        ],
        GroupKind::Adapters => &[
            (METRIC_NET_RX, "perf-widget-metric-net-rx"),
            (METRIC_NET_TX, "perf-widget-metric-net-tx"),
            (METRIC_NET_TRAFFIC, "perf-widget-metric-net-traffic"),
        ],
        GroupKind::Engines | GroupKind::Other(_) => &[],
    }
}

/// The kind of device a member of a group of `kind` is.
fn member_kind(kind: &GroupKind) -> DeviceKind {
    match kind {
        GroupKind::Cores => DeviceKind::CpuLogical,
        GroupKind::Disks => DeviceKind::Disk,
        GroupKind::Adapters => DeviceKind::Net,
        GroupKind::Engines => DeviceKind::Gpu,
        GroupKind::Other(_) => DeviceKind::Other,
    }
}

/// The forms `metric` offers on a member of a group of `kind`, from its metadata. An engine
/// group's `engine:*` offers what every engine offers.
pub fn group_offered(kind: &GroupKind, metric: &MetricKey) -> FormSet {
    let metric = if metric.as_str() == METRIC_ENGINE_ANY {
        MetricKey::new("engine:gpu")
    } else {
        metric.clone()
    };
    MetricMeta::of(member_kind(kind), &metric).map_or(FormSet::empty(), |m| m.forms())
}

/// A new group widget (Add Widget, UI-PRF-061 phase 2): Chart, Load bar, Value and Name as Rows
/// for cores; Chart, Value and Name as Cells for the others; without the forms the metric does
/// not offer, and with no limit. `name` is the device's name for an engine group.
pub fn new_group(
    device: DeviceSelector,
    members: GroupKind,
    metric: MetricKey,
    name: Option<Arc<str>>,
) -> WidgetSetting {
    let offered = group_offered(&members, &metric);
    let (want, items) = match members {
        GroupKind::Cores => (
            FormSet::CHART | FormSet::BAR | FormSet::VALUE | FormSet::LABEL,
            GroupItems::Rows,
        ),
        _ => (
            FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
            GroupItems::Cells,
        ),
    };
    let forms = want & offered;
    let forms = if forms.is_empty() {
        default_forms(offered)
    } else {
        forms
    };
    let device_name = matches!(members, GroupKind::Engines)
        .then_some(name)
        .flatten();
    WidgetSetting {
        id: 0,
        content: WidgetContent::Group(GroupSelector {
            device,
            members,
            metric,
            options: GroupOptions {
                limit: None,
                items,
                arrange: None,
            },
        }),
        forms,
        cell: None,
        options: WidgetOptions::default(),
        device_name,
    }
}

/// Sets group option `option` of group widget `id` to `value` ([`PerfWidgetSetGroup`]): a new
/// metric keeps the forms it offers (or takes the default forms when none is left), `items`
/// is `rows` or `cells`, `arrange` an Arrange id (`auto` is no saved value), `limit` [`LIMIT_ALL`]
/// or a number in `GROUP_LIMIT_RANGE`. The widget then spans at least its new minimum on
/// `grid`. Returns whether the layout changed.
pub fn set_group_option(
    layout: &mut ContainerLayout,
    id: u32,
    option: &str,
    value: &str,
    grid: &InnerGrid,
) -> bool {
    let Some(w) = layout.widgets.iter_mut().find(|w| w.id == id) else {
        return false;
    };
    let WidgetContent::Group(g) = &mut w.content else {
        return false;
    };
    let before = (g.clone(), w.forms);
    match option {
        GROUP_METRIC => {
            let metric = MetricKey::new(value);
            let offered = group_offered(&g.members, &metric);
            if offered.is_empty() {
                return false;
            }
            let kept = w.forms & offered;
            w.forms = if kept.is_empty() {
                default_forms(offered)
            } else {
                kept
            };
            g.metric = metric;
        }
        GROUP_ITEMS => match GroupItems::from_name(value) {
            Some(items) => g.options.items = items,
            None => return false,
        },
        GROUP_ARRANGE => match Arrange::from_id(value) {
            Some(Arrange::Auto) => g.options.arrange = None,
            Some(a) => g.options.arrange = Some(a.id().to_owned()),
            None => return false,
        },
        GROUP_LIMIT => {
            g.options.limit = if value == LIMIT_ALL {
                None
            } else {
                match value.parse::<u16>() {
                    Ok(n) if (GROUP_LIMIT_RANGE.0..=GROUP_LIMIT_RANGE.1).contains(&n) => Some(n),
                    _ => return false,
                }
            };
        }
        _ => return false,
    }
    if (g.clone(), w.forms) == before {
        return false;
    }
    if let Some(cell) = w.cell {
        grid::set_cell(layout, id, cell, grid);
    }
    true
}

/// One metric of one device in the source list.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceItem {
    pub metric: MetricKey,
    pub label: SharedString,
    /// The forms the metric offers on this device.
    pub offered: FormSet,
    /// An engine; a separator comes between the engines and the other metrics.
    pub engine: bool,
    /// The item's own device key, when it is not the group's (a logical CPU of the CPU box).
    pub device: Option<Arc<str>>,
}

impl SourceItem {
    /// The device key the item's source names: its own, else `group`'s.
    fn device_key(&self, group: &SourceGroup) -> Arc<str> {
        Arc::clone(self.device.as_ref().unwrap_or(&group.key))
    }
}

/// A submenu of sources after the group's items and a separator: "Core load >" and
/// "Core speed >" of the CPU box, each item on its logical CPU.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceSub {
    pub label: SharedString,
    pub items: Vec<SourceItem>,
}

/// The group widget Add Widget offers for a device other than a GPU: "All cores >" with one
/// item per member metric.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupAdd {
    pub label: SharedString,
    pub kind: GroupKind,
    pub metrics: Vec<(MetricKey, SharedString)>,
}

/// The metrics of one device the box can show.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceGroup {
    /// The device's persistent key.
    pub key: Arc<str>,
    pub name: SharedString,
    pub items: Vec<SourceItem>,
    /// Submenus after the items (the CPU box's core lists).
    pub subs: Vec<SourceSub>,
    /// The group widget Add Widget offers; `None` for a GPU, which offers "All engines".
    pub add_group: Option<GroupAdd>,
}

impl SourceGroup {
    /// The item of `metric` on `device`, if the device reports it.
    pub fn item_on(&self, device: &str, metric: &MetricKey) -> Option<&SourceItem> {
        self.items
            .iter()
            .chain(self.subs.iter().flat_map(|s| &s.items))
            .find(|i| &i.metric == metric && *i.device_key(self) == *device)
    }

    /// The item of `metric` on the group's own device, if it reports it.
    pub fn item(&self, metric: &MetricKey) -> Option<&SourceItem> {
        self.item_on(&self.key.clone(), metric)
    }
}

/// The forms a GPU metric offers: those of its metadata on the GPU `idx` of `snap`, none for
/// an unknown key.
fn offered(metric: &MetricKey, snap: &Snapshot, idx: DeviceIdx) -> FormSet {
    MetricMeta::of(DeviceKind::Gpu, metric)
        .map_or(FormSet::empty(), |m| m.offered_forms(Some((snap, idx))))
}

/// The sources of the GPUs in `slots` (slot 0 first) on `snap`. A GPU that is not present has
/// no group.
pub fn source_groups(slots: &[SlotDevice], snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let mut groups = Vec::with_capacity(slots.len());
    for slot in slots {
        let Some(entry) = snap
            .devices
            .entries
            .iter()
            .filter(|e| e.present)
            .find(|e| DeviceKey::from_identity(&e.identity).is_some_and(|k| *k.0 == *slot.key))
        else {
            continue;
        };
        let vendor = GpuVendor::of(snap, entry.idx);
        let totals = gpu_totals(snap, entry.idx);
        let mut items = Vec::new();
        for (engine, name_key) in grid_engines(vendor, totals, &[] as &[&str]) {
            let metric = MetricKey::engine(engine);
            items.push(SourceItem {
                offered: offered(&metric, snap, entry.idx),
                metric,
                label: tr(cx, name_key),
                engine: true,
                device: None,
            });
        }
        if let Some(t) = totals {
            for (key, name_key) in OTHER_METRICS {
                let metric = MetricKey::new(key);
                if reading(&metric, t).validity == Validity::Unsupported {
                    continue;
                }
                items.push(SourceItem {
                    offered: offered(&metric, snap, entry.idx),
                    metric,
                    label: tr(cx, name_key),
                    engine: false,
                    device: None,
                });
            }
        }
        groups.push(SourceGroup {
            key: Arc::clone(&slot.key),
            name: SharedString::from(entry.display_name.to_string()),
            items,
            subs: Vec::new(),
            add_group: None,
        });
    }
    groups
}

/// The sources of the CPU box on `snap` (UI-PRF-061 phase 2): the CPU package's metrics it
/// reports, then "Core load >" and "Core speed >" with every present logical CPU, and "All
/// cores >" for Add Widget. Empty when the catalog has no CPU package.
pub fn cpu_source_groups(snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let Some(entry) = snap
        .devices
        .entries
        .iter()
        .find(|e| e.present && &*e.persistent_key == DEVICE_CPU)
    else {
        return Vec::new();
    };
    let on = |kind, metric: &MetricKey, idx| {
        MetricMeta::of(kind, metric)
            .map_or(FormSet::empty(), |m| m.offered_forms(Some((snap, idx))))
    };
    let mut items = Vec::new();
    for (key, name_key) in CPU_METRICS {
        let metric = MetricKey::new(key);
        let (value, _) = box_reading(DeviceKind::Cpu, &metric, snap, entry.idx, None);
        if value.validity == Validity::Unsupported {
            continue;
        }
        items.push(SourceItem {
            offered: on(DeviceKind::Cpu, &metric, entry.idx),
            metric,
            label: tr(cx, name_key),
            engine: false,
            device: None,
        });
    }
    // The present logical CPUs in the cores group's order, as their own sources.
    let cores = GroupSelector {
        device: DeviceSelector::Key(Arc::from(DEVICE_CPU)),
        members: GroupKind::Cores,
        metric: MetricKey::new(METRIC_LOAD),
        options: GroupOptions {
            limit: None,
            items: GroupItems::Rows,
            arrange: None,
        },
    };
    let logical: Vec<(Arc<str>, DeviceIdx, i64)> = match members(&cores, snap, None, None, false) {
        Members::List(list) => list
            .into_iter()
            .filter_map(|s| {
                let DeviceSelector::Key(key) = s.device else {
                    return None;
                };
                let n = key.strip_prefix(CPU_LOGICAL_PREFIX)?.parse::<i64>().ok()?;
                let idx = snap
                    .devices
                    .entries
                    .iter()
                    .find(|e| e.present && *e.persistent_key == *key)?
                    .idx;
                Some((key, idx, n))
            })
            .collect(),
        _ => Vec::new(),
    };
    let sub = |metric: &str, label_key: &str, name_key: &str| SourceSub {
        label: tr(cx, label_key),
        items: logical
            .iter()
            .map(|(key, idx, n)| {
                let metric = MetricKey::new(metric);
                SourceItem {
                    offered: on(DeviceKind::CpuLogical, &metric, *idx),
                    metric,
                    label: tr_args(cx, name_key, &[("n", FluentValue::from(*n))]),
                    engine: false,
                    device: Some(Arc::clone(key)),
                }
            })
            .collect(),
    };
    let subs = if logical.is_empty() {
        Vec::new()
    } else {
        vec![
            sub(
                METRIC_LOAD,
                "perf-widget-core-load",
                "perf-widget-metric-core",
            ),
            sub(
                METRIC_FREQ,
                "perf-widget-core-speed",
                "perf-widget-metric-core-speed",
            ),
        ]
    };
    let add_group = GroupAdd {
        label: tr(cx, "perf-widget-group-cores"),
        kind: GroupKind::Cores,
        metrics: group_metrics(&GroupKind::Cores)
            .iter()
            .map(|(key, name)| (MetricKey::new(key), tr(cx, name)))
            .collect(),
    };
    vec![SourceGroup {
        key: Arc::from(DEVICE_CPU),
        name: SharedString::from(entry.display_name.to_string()),
        items,
        subs,
        add_group: Some(add_group),
    }]
}

/// The sources of the Memory box on `snap` (UI-PRF-061 phase 2): the memory metrics it
/// reports. Empty when the catalog has no memory device.
pub fn mem_source_groups(snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let Some(entry) = snap
        .devices
        .entries
        .iter()
        .find(|e| e.present && &*e.persistent_key == DEVICE_MEMORY)
    else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for (key, name_key) in MEM_METRICS {
        let metric = MetricKey::new(key);
        let (value, _) = box_reading(DeviceKind::Memory, &metric, snap, entry.idx, None);
        if value.validity == Validity::Unsupported {
            continue;
        }
        items.push(SourceItem {
            offered: MetricMeta::of(DeviceKind::Memory, &metric).map_or(FormSet::empty(), |m| {
                m.offered_forms(Some((snap, entry.idx)))
            }),
            metric,
            label: tr(cx, name_key),
            engine: false,
            device: None,
        });
    }
    vec![SourceGroup {
        key: Arc::from(DEVICE_MEMORY),
        name: SharedString::from(entry.display_name.to_string()),
        items,
        subs: Vec::new(),
        add_group: None,
    }]
}

/// What an aggregate box lists in "Source >": one submenu per member of its group, then the
/// aggregate device's submenu, which also offers the group in Add Widget.
struct AggregateSources {
    /// The group whose members get a submenu each, and the metrics a member lists.
    members: GroupKind,
    member_kind: DeviceKind,
    member_metrics: &'static [(&'static str, &'static str)],
    /// The aggregate device's persistent key, kind and metrics.
    total_key: &'static str,
    total_kind: DeviceKind,
    total_metrics: &'static [(&'static str, &'static str)],
    /// The name key of the aggregate's submenu and of its Add Widget group.
    all_key: &'static str,
}

/// The sources of the box `spec` describes on `snap`: one group per member the box shows (in
/// sidebar order, named as the catalog names them) with the metrics it reports, then the
/// aggregate with its metrics and its group for Add Widget. Without an aggregate device only
/// the members are listed.
fn aggregate_source_groups(spec: &AggregateSources, snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let perf = cx
        .try_global::<TlSettings>()
        .map(|_| &TlSettings::settings(cx).performance);
    // A member lists the metrics it reports. The aggregate sums the members' series, so it
    // lists its metrics even before the first of its samples arrives.
    let items = |kind: DeviceKind, idx: DeviceIdx, metrics: &[(&str, &str)]| {
        metrics
            .iter()
            .filter_map(|(key, name_key)| {
                let metric = MetricKey::new(key);
                let (value, _) = box_reading(kind, &metric, snap, idx, None);
                if kind == spec.member_kind && value.validity == Validity::Unsupported {
                    return None;
                }
                Some(SourceItem {
                    offered: MetricMeta::of(kind, &metric)
                        .map_or(FormSet::empty(), |m| m.offered_forms(Some((snap, idx)))),
                    metric,
                    label: tr(cx, name_key),
                    engine: false,
                    device: None,
                })
            })
            .collect::<Vec<_>>()
    };
    let group_metrics_of = group_metrics(&spec.members);
    let group = GroupSelector {
        device: DeviceSelector::Key(Arc::from(spec.total_key)),
        members: spec.members.clone(),
        metric: MetricKey::new(group_metrics_of.first().map_or("", |(k, _)| k)),
        options: GroupOptions {
            limit: None,
            items: GroupItems::Cells,
            arrange: None,
        },
    };
    let mut groups = Vec::new();
    for s in members(&group, snap, perf, None, false).list() {
        let DeviceSelector::Key(key) = &s.device else {
            continue;
        };
        let Some(entry) = snap.devices.entries.iter().filter(|e| e.present).find(|e| {
            *e.persistent_key == **key
                || DeviceKey::from_identity(&e.identity).is_some_and(|k| *k.0 == **key)
        }) else {
            continue;
        };
        groups.push(SourceGroup {
            key: Arc::clone(key),
            name: SharedString::from(entry.display_name.to_string()),
            items: items(spec.member_kind, entry.idx, spec.member_metrics),
            subs: Vec::new(),
            add_group: None,
        });
    }
    let Some(total) = snap
        .devices
        .entries
        .iter()
        .find(|e| e.present && &*e.persistent_key == spec.total_key)
    else {
        return groups;
    };
    let all = tr(cx, spec.all_key);
    let add_group = GroupAdd {
        label: all.clone(),
        kind: spec.members.clone(),
        metrics: group_metrics_of
            .iter()
            .map(|(key, name)| (MetricKey::new(key), tr(cx, name)))
            .collect(),
    };
    groups.push(SourceGroup {
        key: Arc::from(spec.total_key),
        name: all,
        items: items(spec.total_kind, total.idx, spec.total_metrics),
        subs: Vec::new(),
        add_group: Some(add_group),
    });
    groups
}

/// The sources of the Disk box on `snap` (UI-PRF-061 phase 2): one group per disk the box
/// shows (the whole disks of the visible sidebar cards, in sidebar order, named as the catalog
/// names them) with the metrics it reports, then "All disks" with the disk total's metrics and
/// "All disks >" for Add Widget. Without a disk-total device only the disks are listed.
pub fn disk_source_groups(snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let spec = AggregateSources {
        members: GroupKind::Disks,
        member_kind: DeviceKind::Disk,
        member_metrics: &DISK_METRICS,
        total_key: DEVICE_DISKS,
        total_kind: DeviceKind::Disks,
        total_metrics: &DISK_METRICS[..4],
        all_key: "perf-widget-group-disks",
    };
    aggregate_source_groups(&spec, snap, cx)
}

/// The sources of the Network box on `snap` (UI-PRF-061 phase 2): one group per adapter the
/// box counts (not `lo`, of a class `show_network_*` shows, in sidebar order, named as the
/// catalog names them) with Download, Upload and Download and upload, then "All adapters" with
/// the same three of the counted total and "All adapters >" for Add Widget. Without a network
/// total device only the adapters are listed.
pub fn net_source_groups(snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let metrics = group_metrics(&GroupKind::Adapters);
    let spec = AggregateSources {
        members: GroupKind::Adapters,
        member_kind: DeviceKind::Net,
        member_metrics: metrics,
        total_key: DEVICE_NETS,
        total_kind: DeviceKind::Nets,
        total_metrics: metrics,
        all_key: "perf-widget-group-adapters",
    };
    aggregate_source_groups(&spec, snap, cx)
}

/// The fixed widget Add Widget offers: its kind's name as written and its menu label.
#[derive(Clone, Debug, PartialEq)]
pub struct FixedAdd {
    pub name: Arc<str>,
    pub label: SharedString,
}

/// The forms a new widget on a metric that offers `offered` shows: Chart, Value and Name with a
/// chart, else Load bar, Value and Name, each only when offered.
pub fn default_forms(offered: FormSet) -> FormSet {
    let want = if offered.contains(FormSet::CHART) {
        FormSet::CHART | FormSet::VALUE | FormSet::LABEL
    } else {
        FormSet::BAR | FormSet::VALUE | FormSet::LABEL
    };
    let forms = want & offered;
    if forms.is_empty() { offered } else { forms }
}

/// The device key of a widget's source; `None` for a slot selector.
fn device_key(w: &WidgetSetting) -> Option<&str> {
    match w.content.device()? {
        DeviceSelector::Key(k) => Some(k.as_ref()),
        DeviceSelector::Slot(_) => None,
    }
}

/// The source items of `group` as radio items (`selected` marks the widget's source), or as
/// plain items when `selected` is `None`.
fn source_items(
    mut menu: TlMenu,
    group: &SourceGroup,
    id: Option<u32>,
    selected: Option<&(Arc<str>, MetricKey)>,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let mut engines = false;
    for item in &group.items {
        if engines && !item.engine {
            menu = menu.separator();
        }
        engines = item.engine;
        menu = source_item(menu, group, item, id, selected);
    }
    if !group.subs.is_empty() {
        menu = menu.separator();
    }
    for (k, sub) in group.subs.iter().enumerate() {
        let group = group.clone();
        let selected = selected.cloned();
        menu = menu.submenu(sub.label.clone(), window, cx, move |mut menu, _, _| {
            for item in &group.subs[k].items {
                menu = source_item(menu, &group, item, id, selected.as_ref());
            }
            menu
        });
    }
    menu
}

/// One source item: a radio item that sets widget `id`'s source (`selected` marks the widget's
/// own), or a plain Add Widget item when `id` is `None`.
fn source_item(
    menu: TlMenu,
    group: &SourceGroup,
    item: &SourceItem,
    id: Option<u32>,
    selected: Option<&(Arc<str>, MetricKey)>,
) -> TlMenu {
    let device = item.device_key(group);
    let checked = selected.is_some_and(|(k, m)| *k == device && *m == item.metric);
    let source = WidgetSource {
        device,
        metric: Arc::from(item.metric.as_str()),
    };
    match id {
        Some(id) => menu.radio_item(
            item.label.clone(),
            checked,
            Box::new(PerfWidgetSetSource { id, source }),
        ),
        None => menu.item(item.label.clone(), Box::new(PerfWidgetAdd { source })),
    }
}

/// The source list: flat with one GPU, one submenu per GPU (named by the GPU) with two.
/// `id` and `selected` as in [`source_items`].
fn sources(
    menu: TlMenu,
    groups: &Rc<[SourceGroup]>,
    id: Option<u32>,
    selected: Option<(Arc<str>, MetricKey)>,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    if groups.len() == 1 {
        return source_items(menu, &groups[0], id, selected.as_ref(), window, cx);
    }
    let mut menu = menu;
    for (i, group) in groups.iter().enumerate() {
        let groups = Rc::clone(groups);
        let selected = selected.clone();
        menu = menu.submenu(group.name.clone(), window, cx, move |menu, window, cx| {
            source_items(menu, &groups[i], id, selected.as_ref(), window, cx)
        });
    }
    menu
}

/// The Add Widget items: every source of `groups` as a plain item, then a separator and "All
/// engines" for each GPU (named by the GPU when the box shows two), or the device's own group
/// ("All cores >" with Load and Speed in the CPU box), then a separator and `fixed`, the fixed
/// widget the box allows while it has none ("Memory by app").
pub fn add_items(
    menu: TlMenu,
    groups: &Rc<[SourceGroup]>,
    fixed: &[FixedAdd],
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let menu = sources(menu, groups, None, None, window, cx);
    let mut menu = group_adds(menu, groups, window, cx);
    if !fixed.is_empty() {
        menu = menu.separator();
    }
    for add in fixed {
        menu = menu.item(
            add.label.clone(),
            Box::new(PerfWidgetAddFixed {
                fixed: Arc::clone(&add.name),
            }),
        );
    }
    menu
}

/// The metrics phase 3 adds on a GPU, after the box's, in the order of the ADR-019 phase 3 key
/// table, with their name keys. `memory` lists by its menu label, "Memory (dedicated and
/// shared)", so it does not share an item text with VRAM.
const GPU_PAGE_METRICS: [(&str, &str); 4] = [
    (METRIC_GPU_MEMORY, "perf-widget-menu-gpu-memory"),
    (METRIC_VRAM_SHARED, "perf-widget-metric-vram-shared"),
    (METRIC_GPU_CLOCKS, "perf-widget-metric-gpu-clocks"),
    (METRIC_FAN, "perf-widget-metric-fan"),
];

/// The sources of the GPU page of `key` (ADR-019 phase 3 "Source lists"): the GPU box's list
/// for that GPU, then the phase 3 keys the GPU reports. Add Widget offers "All engines" for it
/// (`add_group` stays `None`, as for a GPU of the box).
fn gpu_page_source_groups(key: &str, snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let slot = SlotDevice {
        key: Arc::from(key),
        name: Arc::from(""),
    };
    let mut groups = source_groups(std::slice::from_ref(&slot), snap, cx);
    for group in &mut groups {
        let Some(entry) = snap
            .devices
            .entries
            .iter()
            .filter(|e| e.present)
            .find(|e| DeviceKey::from_identity(&e.identity).is_some_and(|k| *k.0 == *key))
        else {
            continue;
        };
        let Some(t) = gpu_totals(snap, entry.idx) else {
            continue;
        };
        for (metric_key, name_key) in GPU_PAGE_METRICS {
            let metric = MetricKey::new(metric_key);
            // Shared memory is listed only on a GPU that reports a shared total.
            let reported = match metric_key {
                METRIC_VRAM_SHARED => t.shared_total.validity != Validity::Unsupported,
                _ => reading(&metric, t).validity != Validity::Unsupported,
            };
            if !reported {
                continue;
            }
            group.items.push(SourceItem {
                offered: offered(&metric, snap, entry.idx),
                metric,
                label: tr(cx, name_key),
                engine: false,
                device: None,
            });
        }
    }
    groups
}

/// The metrics phase 3 adds on the CPU, after the box's, in the order of the ADR-019 phase 3
/// key table, with their name keys.
const CPU_PAGE_METRICS: [(&str, &str); 4] = [
    (METRIC_CORES_OVERLAID, "perf-widget-metric-cores-overlaid"),
    (METRIC_CORES_STACKED, "perf-widget-metric-cores-stacked"),
    (METRIC_CLOCK, "perf-widget-metric-clock"),
    (METRIC_FREQ_MAX, "perf-widget-metric-freq-max"),
];

/// The sources of the CPU page (ADR-019 phase 3 "Source lists"): the CPU box's list, then the
/// phase 3 keys the CPU reports, then "Core load >" and "Core speed >". Add Widget offers "All
/// cores >" as the box does.
fn cpu_page_source_groups(snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let mut groups = cpu_source_groups(snap, cx);
    let Some(entry) = snap
        .devices
        .entries
        .iter()
        .find(|e| e.present && &*e.persistent_key == DEVICE_CPU)
    else {
        return groups;
    };
    for group in &mut groups {
        for (key, name_key) in CPU_PAGE_METRICS {
            let metric = MetricKey::new(key);
            let (value, _) = box_reading(DeviceKind::Cpu, &metric, snap, entry.idx, None);
            if value.validity == Validity::Unsupported {
                continue;
            }
            group.items.push(SourceItem {
                offered: MetricMeta::of(DeviceKind::Cpu, &metric).map_or(FormSet::empty(), |m| {
                    m.offered_forms(Some((snap, entry.idx)))
                }),
                metric,
                label: tr(cx, name_key),
                engine: false,
                device: None,
            });
        }
    }
    groups
}

/// The metrics phase 3 adds on memory, after the box's, in the order of the ADR-019 phase 3
/// key table, with their item keys (the menu label where a metric has one).
const MEMORY_PAGE_METRICS: [(&str, &str); 3] = [
    (METRIC_PHYSICAL, "perf-widget-metric-physical"),
    (METRIC_COMMIT_PCT, "perf-widget-menu-commit-pct"),
    (METRIC_DIRTY, "perf-widget-metric-dirty"),
];

/// The sources of the Memory page (ADR-019 phase 3 "Source lists"): the Memory box's list,
/// then the phase 3 keys memory reports.
fn memory_page_source_groups(snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let mut groups = mem_source_groups(snap, cx);
    let Some(entry) = snap
        .devices
        .entries
        .iter()
        .find(|e| e.present && &*e.persistent_key == DEVICE_MEMORY)
    else {
        return groups;
    };
    for group in &mut groups {
        for (key, name_key) in MEMORY_PAGE_METRICS {
            let metric = MetricKey::new(key);
            let (value, _) = box_reading(DeviceKind::Memory, &metric, snap, entry.idx, None);
            if value.validity == Validity::Unsupported {
                continue;
            }
            group.items.push(SourceItem {
                offered: MetricMeta::of(DeviceKind::Memory, &metric)
                    .map_or(FormSet::empty(), |m| {
                        m.offered_forms(Some((snap, entry.idx)))
                    }),
                metric,
                label: tr(cx, name_key),
                engine: false,
                device: None,
            });
        }
    }
    groups
}

/// The metrics phase 3 adds on a disk, after the box's, in the order of the ADR-019 phase 3 key
/// table, with their name keys.
const DISK_PAGE_METRICS: [(&str, &str); 3] = [
    (METRIC_RESPONSE, "perf-widget-metric-response"),
    (METRIC_TOTAL_READ, "perf-widget-metric-total-read"),
    (METRIC_TOTAL_WRITTEN, "perf-widget-metric-total-written"),
];

/// The sources of the Disk page of `key` (ADR-019 phase 3 "Source lists"): the Disk box's list,
/// with the phase 3 keys the disk reports after the box's metrics of that disk.
fn disk_page_source_groups(key: &str, snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let mut groups = disk_source_groups(snap, cx);
    let Some(entry) = snap.devices.entries.iter().filter(|e| e.present).find(|e| {
        *e.persistent_key == *key
            || DeviceKey::from_identity(&e.identity).is_some_and(|k| *k.0 == *key)
    }) else {
        return groups;
    };
    for group in groups.iter_mut().filter(|g| *g.key == *key) {
        for (metric_key, name_key) in DISK_PAGE_METRICS {
            let metric = MetricKey::new(metric_key);
            let (value, _) = box_reading(DeviceKind::Disk, &metric, snap, entry.idx, None);
            if value.validity == Validity::Unsupported {
                continue;
            }
            group.items.push(SourceItem {
                offered: MetricMeta::of(DeviceKind::Disk, &metric).map_or(FormSet::empty(), |m| {
                    m.offered_forms(Some((snap, entry.idx)))
                }),
                metric,
                label: tr(cx, name_key),
                engine: false,
                device: None,
            });
        }
    }
    groups
}

/// The metrics phase 3 adds on an adapter, after the box's, in the order of the ADR-019 phase 3
/// key table, with their name keys.
const NET_PAGE_METRICS: [(&str, &str); 3] = [
    (METRIC_TOTAL_RX, "perf-widget-metric-total-rx"),
    (METRIC_TOTAL_TX, "perf-widget-metric-total-tx"),
    (METRIC_SIGNAL, "perf-widget-metric-signal"),
];

/// The sources of the Network page of `key` (ADR-019 phase 3 "Source lists"): the adapter's
/// Download, Upload and Download and upload, as the Network box lists them, then the phase 3
/// keys. A metric the adapter does not report (the signal of a wired adapter) is not listed.
/// The adapter is listed whether or not the Network box counts it. Empty while it is gone.
fn net_page_source_groups(key: &str, snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let Some(entry) = snap.devices.entries.iter().filter(|e| e.present).find(|e| {
        *e.persistent_key == *key
            || DeviceKey::from_identity(&e.identity).is_some_and(|k| *k.0 == *key)
    }) else {
        return Vec::new();
    };
    let items = group_metrics(&GroupKind::Adapters)
        .iter()
        .chain(NET_PAGE_METRICS.iter())
        .filter_map(|(metric_key, name_key)| {
            let metric = MetricKey::new(metric_key);
            let (value, _) = box_reading(DeviceKind::Net, &metric, snap, entry.idx, None);
            if value.validity == Validity::Unsupported {
                return None;
            }
            Some(SourceItem {
                offered: MetricMeta::of(DeviceKind::Net, &metric).map_or(FormSet::empty(), |m| {
                    m.offered_forms(Some((snap, entry.idx)))
                }),
                metric,
                label: tr(cx, name_key),
                engine: false,
                device: None,
            })
        })
        .collect();
    vec![SourceGroup {
        key: Arc::from(key),
        name: SharedString::from(entry.display_name.to_string()),
        items,
        subs: Vec::new(),
        add_group: None,
    }]
}

/// The sources of the device page `container` on `snap` (ADR-019 phase 3 "Source lists"): the
/// metrics of the page's own device, from the box lists. Only the CPU and GPU pages offer a
/// group: the CPU page "All cores >" (`tasklens-hzire.5`), the GPU page "All engines"
/// (`tasklens-hzire.4`). A page kind whose page bead has not come yet lists nothing.
pub fn page_source_groups(container: &str, snap: &Snapshot, cx: &App) -> Vec<SourceGroup> {
    let Some(key) = tl_ui_settings::layout_pages::page_key(container) else {
        return Vec::new();
    };
    let groups = match DeviceKind::of_key(key) {
        DeviceKind::Disk => disk_page_source_groups(key, snap, cx),
        DeviceKind::Disks => disk_source_groups(snap, cx),
        DeviceKind::Gpu => return gpu_page_source_groups(key, snap, cx),
        DeviceKind::Cpu => return cpu_page_source_groups(snap, cx),
        DeviceKind::Memory => return memory_page_source_groups(snap, cx),
        DeviceKind::Net => return net_page_source_groups(key, snap, cx),
        _ => Vec::new(),
    };
    groups
        .into_iter()
        .filter(|g| *g.key == *key)
        .map(|g| SourceGroup {
            add_group: None,
            ..g
        })
        .collect()
}

/// The group items of Add Widget after a separator: the device's own group, or "All engines"
/// for each GPU. Nothing for a device without a group (memory).
fn group_adds(
    menu: TlMenu,
    groups: &Rc<[SourceGroup]>,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let mut menu = menu;
    if groups.is_empty() {
        return menu;
    }
    // Memory and a device page other than CPU and GPU have no group to add.
    let no_group = groups.iter().all(|g| {
        g.add_group.is_none()
            && !matches!(
                DeviceKind::of_key(&g.key),
                DeviceKind::Gpu | DeviceKind::Other
            )
    });
    if groups.iter().all(|g| &*g.key == DEVICE_MEMORY) || no_group {
        return menu;
    }
    menu = menu.separator();
    // A device with its own group ("All cores >"): one item per member metric.
    if let Some(add) = groups
        .iter()
        .find_map(|g| g.add_group.clone().map(|a| (g.key.clone(), a)))
    {
        let (device, add) = add;
        return menu.submenu(add.label.clone(), window, cx, move |mut menu, _, _| {
            for (metric, label) in &add.metrics {
                menu = menu.item(
                    label.clone(),
                    Box::new(PerfWidgetAddGroup {
                        device: Arc::clone(&device),
                        group: Arc::from(add.kind.name()),
                        metric: Arc::from(metric.as_str()),
                    }),
                );
            }
            menu
        });
    }
    let all = tr(cx, "perf-widget-group-engines");
    for group in groups.iter() {
        let label = if groups.len() == 1 {
            all.clone()
        } else {
            tr_args(
                cx,
                "perf-widget-device-metric",
                &[
                    ("device", FluentValue::from(group.name.to_string())),
                    ("metric", FluentValue::from(all.to_string())),
                ],
            )
        };
        menu = menu.item(
            label,
            Box::new(PerfWidgetAddGroup {
                device: Arc::clone(&group.key),
                group: Arc::from(GroupKind::Engines.name()),
                metric: Arc::from(METRIC_ENGINE_ANY),
            }),
        );
    }
    menu
}

/// The group items of group widget `id` with selector `g` (UI-PRF-061 phase 2): "Items >",
/// "Arrange >" with Cells, and "Show at most >".
fn group_items(
    menu: TlMenu,
    id: u32,
    g: &GroupSelector,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let set = move |option: &str, value: &str| -> Box<dyn gpui_kit::Action> {
        Box::new(PerfWidgetSetGroup {
            id,
            option: Arc::from(option),
            value: Arc::from(value),
        })
    };
    let items = g.options.items;
    let rows = tr(cx, "perf-widget-items-rows");
    let cells = tr(cx, "perf-widget-items-cells");
    let mut menu = menu.submenu(
        tr(cx, "perf-widget-items"),
        window,
        cx,
        move |menu, _, _| {
            menu.radio_item(
                rows.clone(),
                items == GroupItems::Rows,
                set(GROUP_ITEMS, GroupItems::Rows.name()),
            )
            .radio_item(
                cells.clone(),
                items == GroupItems::Cells,
                set(GROUP_ITEMS, GroupItems::Cells.name()),
            )
        },
    );
    if items == GroupItems::Cells {
        let arrange = g
            .options
            .arrange
            .as_deref()
            .and_then(Arrange::from_id)
            .unwrap_or(Arrange::Auto);
        let labels: Vec<(Arrange, SharedString)> = Arrange::ALL
            .into_iter()
            .map(|a| (a, tr(cx, a.label())))
            .collect();
        menu = menu.submenu(
            tr(cx, "perf-overview-arrange"),
            window,
            cx,
            move |mut menu, _, _| {
                for (a, label) in &labels {
                    menu =
                        menu.radio_item(label.clone(), *a == arrange, set(GROUP_ARRANGE, a.id()));
                }
                menu
            },
        );
    }
    let limit = g.options.limit;
    let all = tr(cx, "perf-widget-limit-all");
    menu.submenu(
        tr(cx, "perf-widget-limit"),
        window,
        cx,
        move |menu, _, _| {
            let mut menu =
                menu.radio_item(all.clone(), limit.is_none(), set(GROUP_LIMIT, LIMIT_ALL));
            let mut choices: Vec<u16> = LIMIT_CHOICES.to_vec();
            if let Some(n) = limit
                && !choices.contains(&n)
            {
                choices.push(n);
                choices.sort_unstable();
            }
            for n in choices {
                menu = menu.radio_item(
                    SharedString::from(n.to_string()),
                    limit == Some(n),
                    set(GROUP_LIMIT, &n.to_string()),
                );
            }
            menu
        },
    )
}

/// The widget menu of `w`, whose metric offers `offered` (UI-PRF-061 "Widget menu").
pub fn widget_menu(
    menu: TlMenu,
    w: &WidgetSetting,
    offered: FormSet,
    groups: &Rc<[SourceGroup]>,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let id = w.id;
    let selected = device_key(w)
        .zip(w.content.metric_key())
        .map(|(k, m)| (Arc::from(k), m.clone()));
    if matches!(w.content, WidgetContent::Fixed(_)) {
        // A fixed widget has no source, forms or options: only "Remove Widget".
        return menu.item(tr(cx, "perf-widget-remove"), Box::new(PerfWidgetRemove));
    }
    let group = match &w.content {
        WidgetContent::Group(g) => Some(g.clone()),
        _ => None,
    };
    let mut menu = match &group {
        // A group lists the metrics of its kind; an engine group has none to pick.
        Some(g) => {
            let metrics: Vec<(&'static str, SharedString)> = group_metrics(&g.members)
                .iter()
                .map(|(key, name)| (*key, tr(cx, name)))
                .collect();
            if metrics.is_empty() {
                menu
            } else {
                let current = g.metric.clone();
                menu.submenu(
                    tr(cx, "perf-widget-source"),
                    window,
                    cx,
                    move |mut menu, _, _| {
                        for (key, label) in &metrics {
                            menu = menu.radio_item(
                                label.clone(),
                                current.as_str() == *key,
                                Box::new(PerfWidgetSetGroup {
                                    id,
                                    option: Arc::from(GROUP_METRIC),
                                    value: Arc::from(*key),
                                }),
                            );
                        }
                        menu
                    },
                )
            }
        }
        None => {
            let source_groups = Rc::clone(groups);
            menu.submenu(
                tr(cx, "perf-widget-source"),
                window,
                cx,
                move |menu, window, cx| {
                    sources(menu, &source_groups, Some(id), selected.clone(), window, cx)
                },
            )
        }
    };
    let forms = w.forms;
    let shown: Vec<(FormSet, &'static str, SharedString)> = FORM_ITEMS
        .iter()
        .filter(|(f, _, _)| offered.contains(*f))
        .map(|(f, name, key)| (*f, *name, tr(cx, key)))
        .collect();
    let on = (forms & offered).bits().count_ones();
    menu = menu.submenu(
        tr(cx, "perf-widget-show"),
        window,
        cx,
        move |mut menu, _, _| {
            for (form, name, label) in &shown {
                let checked = forms.contains(*form);
                menu = if checked && on <= 1 {
                    menu.item_disabled_checked(label.clone(), None)
                } else {
                    menu.check_item(
                        label.clone(),
                        checked,
                        Box::new(PerfWidgetSetForm {
                            id,
                            form: Arc::from(*name),
                            on: !checked,
                        }),
                    )
                };
            }
            menu
        },
    );
    if forms.contains(FormSet::CHART) {
        let options = w.options;
        let fill = tr(cx, "perf-widget-fill");
        let dashed = tr(cx, "perf-widget-dashed");
        menu = menu.submenu(
            tr(cx, "perf-widget-chart"),
            window,
            cx,
            move |menu, _, _| {
                menu.check_item(
                    fill.clone(),
                    options.fill,
                    Box::new(PerfWidgetSetOption {
                        id,
                        option: Arc::from(OPTION_FILL),
                        on: !options.fill,
                    }),
                )
                .check_item(
                    dashed.clone(),
                    options.dashed,
                    Box::new(PerfWidgetSetOption {
                        id,
                        option: Arc::from(OPTION_DASHED),
                        on: !options.dashed,
                    }),
                )
            },
        );
    }
    if let Some(g) = &group {
        menu = group_items(menu, id, g, window, cx);
    }
    menu.separator()
        .item(tr(cx, "perf-widget-remove"), Box::new(PerfWidgetRemove))
}

/// The box name for `{$box}`: "GPU" for the GPU box.
fn box_name(container: &str, cx: &App) -> SharedString {
    match container {
        tl_ui_settings::layout::GPU_BOX_CONTAINER => tr(cx, "perf-overview-box-gpu"),
        tl_ui_settings::layout::CPU_BOX_CONTAINER => tr(cx, "perf-card-heading-cpu"),
        tl_ui_settings::layout::MEMORY_BOX_CONTAINER => tr(cx, "perf-card-heading-memory"),
        tl_ui_settings::layout::DISK_BOX_CONTAINER => tr(cx, "perf-overview-box-disks"),
        tl_ui_settings::layout::NET_BOX_CONTAINER => tr(cx, "perf-overview-box-network"),
        tl_ui_settings::layout::PROCESSES_BOX_CONTAINER => tr(cx, "perf-overview-box-processes"),
        tl_ui_settings::layout::TIMELINE_BOX_CONTAINER => tr(cx, "perf-overview-box-timeline"),
        other => SharedString::from(other.to_string()),
    }
}

/// Opens `dialog` and runs `on_ok` when its action button ends it. A dialog that cannot open
/// counts as Cancel.
fn open(
    dialog: TlDialog,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let _ = dialog.open(window, cx, move |result, window, cx| {
        if matches!(result, DialogResult::Confirmed { .. }) {
            on_ok(window, cx);
        }
    });
}

/// "Reset the GPU box?" in Edit Widgets: the working layout goes back to the default, which
/// Undo can still bring back until Apply.
pub fn confirm_reset(
    container: &str,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let name = box_name(container, cx);
    let title = tr_args(
        cx,
        "perf-widget-reset-title",
        &[("box", FluentValue::from(name.to_string()))],
    );
    let dialog = TlDialog::confirm(
        tr(cx, "perf-widget-reset-body"),
        tr(cx, "perf-widget-reset-confirm"),
    )
    .heading(title);
    open(dialog, window, cx, on_ok);
}

/// "Reset the {$page} page?" of a device page (UI-PRF-061 phase 3): in Edit Widgets (`now`
/// false) the working layout goes back to the default, which Undo can still bring back until
/// Apply; from "Reset Page" (`now`) the default is saved at once.
pub fn confirm_reset_page(
    page: SharedString,
    now: bool,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let title = tr_args(
        cx,
        "perf-widget-reset-page-title",
        &[("page", FluentValue::from(page.to_string()))],
    );
    let body = if now {
        "perf-widget-reset-page-now-body"
    } else {
        "perf-widget-reset-page-body"
    };
    let dialog =
        TlDialog::confirm(tr(cx, body), tr(cx, "perf-widget-reset-confirm")).heading(title);
    open(dialog, window, cx, on_ok);
}

/// "Reset the GPU box?" from the box menu with a grid layout: it saves at once, so the user's
/// widgets cannot be restored.
pub fn confirm_reset_now(
    container: &str,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let name = box_name(container, cx);
    let title = tr_args(
        cx,
        "perf-widget-reset-title",
        &[("box", FluentValue::from(name.to_string()))],
    );
    let dialog = TlDialog::confirm(
        tr(cx, "perf-widget-reset-now-body"),
        tr(cx, "perf-widget-reset-confirm"),
    )
    .heading(title);
    open(dialog, window, cx, on_ok);
}

/// "Replace your widgets?" of a device page (UI-PRF-061 phase 3): a preset chosen in the page
/// header or menu over a grid layout. `page` is the page's header word, `preset` the preset's
/// shown name.
pub fn confirm_preset_page(
    page: SharedString,
    preset: SharedString,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let body = tr_args(
        cx,
        "perf-widget-preset-page-body",
        &[
            ("page", FluentValue::from(page.to_string())),
            ("preset", FluentValue::from(preset.to_string())),
        ],
    );
    let dialog = TlDialog::confirm(body, tr(cx, "perf-widget-preset-confirm"))
        .heading(tr(cx, "perf-widget-preset-title"));
    open(dialog, window, cx, on_ok);
}

/// "Replace your widgets?": a preset chosen in the box menu over a grid layout. `preset` is the
/// preset's shown name.
pub fn confirm_preset(
    container: &str,
    preset: SharedString,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let name = box_name(container, cx);
    let body = tr_args(
        cx,
        "perf-widget-preset-body",
        &[
            ("box", FluentValue::from(name.to_string())),
            ("preset", FluentValue::from(preset.to_string())),
        ],
    );
    let dialog = TlDialog::confirm(body, tr(cx, "perf-widget-preset-confirm"))
        .heading(tr(cx, "perf-widget-preset-title"));
    open(dialog, window, cx, on_ok);
}

#[cfg(test)]
mod tests {
    use tl_ui_settings::layout::{CPU_BOX_CONTAINER, LAYOUT_VERSION, WidgetCell};
    use tl_ui_settings::layout_boxes::{DEVICE_CPU, DEVICE_DISKS};

    use super::*;

    fn group_layout(w: WidgetSetting) -> ContainerLayout {
        ContainerLayout {
            container: CPU_BOX_CONTAINER.to_owned(),
            version: LAYOUT_VERSION,
            preset: None,
            widgets: vec![WidgetSetting {
                id: 1,
                cell: Some(WidgetCell {
                    col: 0,
                    row: 0,
                    cols: 12,
                    rows: 8,
                }),
                ..w
            }],
            next_id: 2,
        }
    }

    fn options(l: &ContainerLayout) -> (GroupOptions, MetricKey, FormSet) {
        match &l.widgets[0].content {
            WidgetContent::Group(g) => (g.options.clone(), g.metric.clone(), l.widgets[0].forms),
            _ => panic!("a group"),
        }
    }

    #[test]
    fn new_groups_take_their_defaults() {
        let cpu = DeviceSelector::Key(Arc::from(DEVICE_CPU));
        let w = new_group(cpu, GroupKind::Cores, MetricKey::new(METRIC_LOAD), None);
        assert_eq!(
            w.forms,
            FormSet::CHART | FormSet::BAR | FormSet::VALUE | FormSet::LABEL
        );
        let WidgetContent::Group(g) = &w.content else {
            panic!("a group");
        };
        assert_eq!(g.options.items, GroupItems::Rows);
        assert_eq!(g.options.limit, None);
        let disks = DeviceSelector::Key(Arc::from(DEVICE_DISKS));
        let w = new_group(
            disks,
            GroupKind::Disks,
            MetricKey::new(METRIC_DISK_IO),
            None,
        );
        // Read and write is a pair: no bar, no stats.
        assert_eq!(w.forms, FormSet::CHART | FormSet::VALUE | FormSet::LABEL);
        let WidgetContent::Group(g) = &w.content else {
            panic!("a group");
        };
        assert_eq!(g.options.items, GroupItems::Cells);
        let gpu = DeviceSelector::Key(Arc::from("gpu:GPU-x"));
        let name = Some(Arc::from("Card A"));
        let w = new_group(
            gpu,
            GroupKind::Engines,
            MetricKey::new(METRIC_ENGINE_ANY),
            name,
        );
        assert_eq!(w.forms, FormSet::CHART | FormSet::VALUE | FormSet::LABEL);
        assert_eq!(w.device_name.as_deref(), Some("Card A"));
    }

    #[test]
    fn group_options_change_only_known_values() {
        let cpu = DeviceSelector::Key(Arc::from(DEVICE_CPU));
        let mut l = group_layout(new_group(
            cpu,
            GroupKind::Cores,
            MetricKey::new(METRIC_LOAD),
            None,
        ));
        let g = InnerGrid::new(750., 12, 1.);
        assert!(set_group_option(&mut l, 1, GROUP_ITEMS, "cells", &g));
        assert_eq!(options(&l).0.items, GroupItems::Cells);
        assert!(
            !set_group_option(&mut l, 1, GROUP_ITEMS, "cells", &g),
            "no change"
        );
        assert!(!set_group_option(&mut l, 1, GROUP_ITEMS, "piles", &g));
        assert!(set_group_option(&mut l, 1, GROUP_ARRANGE, "vertical", &g));
        assert_eq!(options(&l).0.arrange.as_deref(), Some("vertical"));
        assert!(set_group_option(&mut l, 1, GROUP_ARRANGE, "auto", &g));
        assert_eq!(options(&l).0.arrange, None);
        assert!(set_group_option(&mut l, 1, GROUP_LIMIT, "8", &g));
        assert_eq!(options(&l).0.limit, Some(8));
        assert!(!set_group_option(&mut l, 1, GROUP_LIMIT, "0", &g));
        assert!(!set_group_option(&mut l, 1, GROUP_LIMIT, "257", &g));
        assert!(set_group_option(&mut l, 1, GROUP_LIMIT, LIMIT_ALL, &g));
        assert_eq!(options(&l).0.limit, None);
        // Speed keeps the forms it offers.
        assert!(set_group_option(&mut l, 1, GROUP_METRIC, METRIC_FREQ, &g));
        assert_eq!(options(&l).1.as_str(), METRIC_FREQ);
        assert!(!set_group_option(&mut l, 1, GROUP_METRIC, "nonsense", &g));
        assert!(
            !set_group_option(&mut l, 2, GROUP_LIMIT, "8", &g),
            "no widget 2"
        );
    }

    #[test]
    fn default_forms_follow_the_offered_forms() {
        let all = FormSet::all();
        assert_eq!(
            default_forms(all),
            FormSet::CHART | FormSet::VALUE | FormSet::LABEL
        );
        let no_chart = FormSet::BAR | FormSet::VALUE | FormSet::LABEL | FormSet::STATS;
        assert_eq!(
            default_forms(no_chart),
            FormSet::BAR | FormSet::VALUE | FormSet::LABEL
        );
        // Without a bar: Value and Name.
        assert_eq!(
            default_forms(FormSet::VALUE | FormSet::LABEL | FormSet::STATS),
            FormSet::VALUE | FormSet::LABEL
        );
        // Never empty when something is offered.
        assert_eq!(default_forms(FormSet::STATS), FormSet::STATS);
    }
}
