//! The widget menu, the Add Widget list and the confirmations of Edit Widgets (UI-PRF-061).
//!
//! - **Sources**: the metrics of every GPU the box can show ([`source_groups`]). Per GPU: the
//!   engines it reports, in the GPU page's order and with its names, then VRAM, Graphics clock,
//!   Memory clock, Power draw and Temperature. A metric the GPU does not report is not listed.
//! - **Widget menu** ([`widget_menu`]): "Source >" (radio items, one submenu per GPU when the
//!   box shows two), "Show >" (check items for the forms the metric offers; the last checked one
//!   is disabled), "Chart >" while Chart is on (Fill under the line, Dashed line), a separator,
//!   then "Remove Widget".
//! - **Add Widget** ([`add_items`]): the same sources as plain items.
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
    DeviceSelector, FormSet, METRIC_CLOCK_GFX, METRIC_CLOCK_MEM, METRIC_POWER, METRIC_TEMP,
    METRIC_VRAM, MetricKey, WidgetSetting,
};
use tl_model::layout_boxes::DeviceKind;
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::grid::SlotDevice;
use super::meta::MetricMeta;
use super::source::reading;
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::menu::TlMenu;
use crate::i18n::{tr, tr_args};
use crate::pages::performance::DeviceKey;
use crate::pages::performance::actions::{
    PerfWidgetAdd, PerfWidgetRemove, PerfWidgetSetForm, PerfWidgetSetOption, PerfWidgetSetSource,
    WidgetSource,
};
use crate::pages::performance::gpu::{GpuVendor, gpu_totals, grid_engines};

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

/// The Add Widget items: every source of `groups` as a plain item.
pub fn add_items(
    menu: TlMenu,
    groups: &Rc<[SourceGroup]>,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    sources(menu, groups, None, None, window, cx)
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
    let source_groups = Rc::clone(groups);
    let mut menu = menu.submenu(
        tr(cx, "perf-widget-source"),
        window,
        cx,
        move |menu, window, cx| {
            sources(menu, &source_groups, Some(id), selected.clone(), window, cx)
        },
    );
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
    use super::*;

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
