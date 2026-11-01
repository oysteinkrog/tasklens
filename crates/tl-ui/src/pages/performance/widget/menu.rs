//! The widget menu, the Add Widget list and the confirmations of Edit Widgets (UI-PRF-061).
//!
//! - **Sources**: the metrics of every GPU the box can show ([`source_groups`]). Per GPU: the
//!   engines it reports, in the GPU page's order and with its names, then VRAM, Graphics clock,
//!   Memory clock, Power draw and Temperature. A metric the GPU does not report is not listed.
//! - **Widget menu** ([`widget_menu`]): "Source >" (radio items, one submenu per GPU when the
//!   box shows two), "Show >" (check items for the forms the metric offers; the last checked one
//!   is disabled), "Chart >" while Chart is on (Fill under the line, Dashed line), a separator,
//!   then "Remove Widget".
//! - **Add Widget** ([`add_items`]): the same sources as plain items, then a separator and
//!   "All engines", one item per GPU (a group widget, ADR-019 phase 2).
//! - **Group widgets** (UI-PRF-061 phase 2): "Source >" lists only the metrics of the group's
//!   kind ([`group_metrics`]; none for engines), and after "Chart >" come "Items >" (Rows,
//!   Cells), "Arrange >" with Cells (Automatic, Stack vertically, Side by side) and
//!   "Show at most >" (All, 4, 8, 16, 32, and the saved number when it is another).
//!   [`set_group_option`] and [`new_group`] are the edits they make.
//! - **Confirmations**: Reset in Edit Widgets ([`confirm_reset`]), Reset Box with a grid layout
//!   ([`confirm_reset_now`]) and a preset over a grid layout ([`confirm_preset`]).
//!
//! The items dispatch the `PerfWidget*` actions to the editor's focus handle.

use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{App, SharedString, Window};
use tl_model::ids::DeviceIdx;
use tl_model::layout::{
    ContainerLayout, DeviceSelector, FormSet, GROUP_LIMIT_RANGE, GroupItems, GroupKind,
    GroupOptions, GroupSelector, METRIC_CLOCK_GFX, METRIC_CLOCK_MEM, METRIC_ENGINE_ANY,
    METRIC_POWER, METRIC_TEMP, METRIC_VRAM, MetricKey, WidgetContent, WidgetOptions, WidgetSetting,
};
use tl_model::layout_boxes::{
    DeviceKind, METRIC_DISK_ACTIVE, METRIC_DISK_IO, METRIC_DISK_READ, METRIC_DISK_WRITE,
    METRIC_FREQ, METRIC_LOAD, METRIC_NET_RX, METRIC_NET_TRAFFIC, METRIC_NET_TX,
};
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::grid::{self, InnerGrid, SlotDevice};
use super::meta::MetricMeta;
use super::source::reading;
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::menu::TlMenu;
use crate::i18n::{tr, tr_args};
use crate::pages::performance::DeviceKey;
use crate::pages::performance::actions::{
    PerfWidgetAdd, PerfWidgetAddGroup, PerfWidgetRemove, PerfWidgetSetForm, PerfWidgetSetGroup,
    PerfWidgetSetOption, PerfWidgetSetSource, WidgetSource,
};
use crate::pages::performance::gpu::{GpuVendor, gpu_totals, grid_engines};
use crate::pages::performance::overview::layout::Arrange;

/// The device facts after the engines, in menu order, with their name keys.
const OTHER_METRICS: [(&str, &str); 5] = [
    (METRIC_VRAM, "perf-overview-gpu-vram"),
    (METRIC_CLOCK_GFX, "perf-widget-metric-clock-gfx"),
    (METRIC_CLOCK_MEM, "perf-widget-metric-clock-mem"),
    (METRIC_POWER, "perf-widget-metric-power"),
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

/// One metric of one GPU in the source list.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceItem {
    pub metric: MetricKey,
    pub label: SharedString,
    /// The forms the metric offers on this GPU.
    pub offered: FormSet,
    /// An engine; a separator comes between the engines and the other metrics.
    pub engine: bool,
}

/// The metrics of one GPU the box can show.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceGroup {
    /// The GPU's persistent device key.
    pub key: Arc<str>,
    pub name: SharedString,
    pub items: Vec<SourceItem>,
}

impl SourceGroup {
    /// The item of `metric`, if the GPU reports it.
    pub fn item(&self, metric: &MetricKey) -> Option<&SourceItem> {
        self.items.iter().find(|i| &i.metric == metric)
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
                });
            }
        }
        groups.push(SourceGroup {
            key: Arc::clone(&slot.key),
            name: SharedString::from(entry.display_name.to_string()),
            items,
        });
    }
    groups
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
    selected: Option<&MetricKey>,
) -> TlMenu {
    let mut engines = false;
    for item in &group.items {
        if engines && !item.engine {
            menu = menu.separator();
        }
        engines = item.engine;
        let source = WidgetSource {
            device: Arc::clone(&group.key),
            metric: Arc::from(item.metric.as_str()),
        };
        menu = match id {
            Some(id) => menu.radio_item(
                item.label.clone(),
                selected == Some(&item.metric),
                Box::new(PerfWidgetSetSource { id, source }),
            ),
            None => menu.item(item.label.clone(), Box::new(PerfWidgetAdd { source })),
        };
    }
    menu
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
        let metric = selected
            .as_ref()
            .filter(|(k, _)| *k == groups[0].key)
            .map(|(_, m)| m);
        return source_items(menu, &groups[0], id, metric);
    }
    let mut menu = menu;
    for (i, group) in groups.iter().enumerate() {
        let groups = Rc::clone(groups);
        let selected = selected.clone();
        menu = menu.submenu(group.name.clone(), window, cx, move |menu, _, _| {
            let group = &groups[i];
            let metric = selected
                .as_ref()
                .filter(|(k, _)| *k == group.key)
                .map(|(_, m)| m);
            source_items(menu, group, id, metric)
        });
    }
    menu
}

/// The Add Widget items: every source of `groups` as a plain item, then a separator and "All
/// engines" for each GPU (named by the GPU when the box shows two).
pub fn add_items(
    menu: TlMenu,
    groups: &Rc<[SourceGroup]>,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let mut menu = sources(menu, groups, None, None, window, cx);
    if groups.is_empty() {
        return menu;
    }
    menu = menu.separator();
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
        tl_model::layout::GPU_BOX_CONTAINER => tr(cx, "perf-overview-box-gpu"),
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
    use tl_model::layout::{CPU_BOX_CONTAINER, LAYOUT_VERSION, WidgetCell};
    use tl_model::layout_boxes::{DEVICE_CPU, DEVICE_DISKS};

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
