//! Settings window, Performance page, P2 rows (E21-023, UI-SHL-066, PERF-025, R-016, R-053,
//! R-054): graphs, CPU, memory, GPU options, the hidden GPU engine checklist and the device
//! visibility switches.
//!
//! The Fans group (E21-037) is the last group: one row per present fan, with a label input and a
//! temperature sensor dropdown, writing `fans."<fan key>"`. The engine checklist lists
//! every present GPU of the device catalog; the Settings window re-renders on
//! `ComponentMask::DEVICES`, so it follows hotplug.
//!
//! The "Device order and visibility" list (E21-024, UI-SHL-066, PERF-005, PERF-006, R-090) has
//! one row per sidebar device, in sidebar order ([`sidebar_order`], so the Overview row is
//! included, D-022). A row moves by dragging its handle or with Alt+Up and Alt+Down while it
//! has focus; either writes the full `performance.device_order`. Its Default / Show / Hide
//! segments edit `performance.device_overrides`. The Performance sidebar edit mode writes the
//! same keys.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::Focusable as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div,
};
use tl_model::devices::{DeviceIdentity, DeviceKind};
use tl_model::settings::{DeviceVisibility, FanOverride, GRAPH_RANGES_S};
use tl_model::snapshot::Snapshot;
use tl_model::system::SensorKind;
use tl_model::units;
use tl_model::validity::Validity;

use super::{CheckSection, PageBuilder, PageId};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::input::TlTextInput;
use crate::components::select::{TlSelect, TlSelectItem, TlSelectState};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::validity::copy_text;
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::pages::performance::card::{CardWords, card_words};
use crate::pages::performance::gpu::device_engines;
use crate::pages::performance::{
    DeviceKey, DeviceKind as PerfKind, DeviceRow, PerformanceSettingsView, sidebar_order,
    type_number,
};
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Performance);
    let graphs = tr(cx, "shell-settings-perf-group-graphs");
    let cpu = tr(cx, "shell-settings-perf-group-cpu");
    let memory = tr(cx, "shell-settings-perf-group-memory");
    let gpu = tr(cx, "shell-settings-perf-group-gpu");
    let devices = tr(cx, "shell-settings-perf-group-devices");
    let network = tr(cx, "shell-settings-perf-group-network");
    let overview = tr(cx, "shell-settings-perf-group-overview");

    let ranges: Vec<(SharedString, SettingValue)> = GRAPH_RANGES_S
        .into_iter()
        .zip([
            "shell-settings-perf-range-30",
            "shell-settings-perf-range-60",
            "shell-settings-perf-range-300",
            "shell-settings-perf-range-600",
        ])
        .map(|(s, key)| (tr(cx, key), SettingValue::Int(i64::from(s))))
        .collect();
    page.dropdown(
        graphs.clone(),
        SettingKey::PerformanceGraphRangeS,
        tr(cx, "shell-settings-perf-graph-range"),
        &ranges,
    );
    // Off: the network graph axis is the link speed. UI-SHL-066 gives no description.
    for (key, label) in [
        (
            SettingKey::PerformanceCurvedLines,
            "shell-settings-perf-curved-lines",
        ),
        (
            SettingKey::PerformanceNetworkDynamicScale,
            "shell-settings-perf-network-dynamic-scale",
        ),
    ] {
        page.switch(graphs.clone(), key, tr(cx, label), None);
    }

    let modes = options(
        cx,
        &[
            ("shell-settings-perf-cpu-overall", "overall"),
            ("shell-settings-perf-cpu-per-core", "per_core"),
            ("shell-settings-perf-cpu-overlaid", "all_cores_overlaid"),
            ("shell-settings-perf-cpu-stacked", "all_cores_stacked"),
        ],
    );
    page.dropdown(
        cpu.clone(),
        SettingKey::PerformanceCpuGraphMode,
        tr(cx, "shell-settings-perf-cpu-graph"),
        &modes,
    )
    .switch(
        cpu.clone(),
        SettingKey::PerformanceCpuKernel,
        tr(cx, "shell-settings-perf-kernel-times"),
        None,
    );
    let secondary = options(
        cx,
        &[
            ("shell-settings-perf-secondary-none", "none"),
            ("shell-settings-perf-secondary-power", "power"),
            ("shell-settings-perf-secondary-clock", "clock"),
            ("shell-settings-perf-secondary-temperature", "temperature"),
        ],
    );
    page.dropdown(
        cpu,
        SettingKey::PerformanceCpuSecondaryGraph,
        tr(cx, "shell-settings-perf-secondary-graph"),
        &secondary,
    );

    for (key, label) in [
        (
            SettingKey::PerformanceMemoryComposition,
            "shell-settings-perf-memory-composition",
        ),
        (
            SettingKey::PerformanceMemorySwapGraph,
            "shell-settings-perf-swap",
        ),
    ] {
        page.switch(memory.clone(), key, tr(cx, label), None);
    }

    page.switch(
        overview,
        SettingKey::PerformanceOverviewTopProcesses,
        tr(cx, "shell-settings-perf-overview-top-processes"),
        None,
    );

    let optional = CheckSection {
        heading: SharedString::default(),
        options: vec![
            (tr(cx, "shell-settings-perf-gpu-power"), "power".into()),
            (tr(cx, "shell-settings-perf-gpu-clocks"), "clocks".into()),
        ],
    };
    page.checklist(
        gpu.clone(),
        SettingKey::PerformanceGpuOptionalGraphs,
        tr(cx, "shell-settings-perf-gpu-optional"),
        vec![optional],
        false,
        SharedString::default(),
    );
    let engines = engine_sections(cx);
    page.checklist(
        gpu,
        SettingKey::PerformanceHiddenGpuEngines,
        tr(cx, "shell-settings-perf-hidden-engines"),
        engines,
        true,
        tr(cx, "shell-settings-perf-no-gpu"),
    );

    for (key, label) in [
        (
            SettingKey::PerformanceShowDisks,
            "shell-settings-perf-disks",
        ),
        (SettingKey::PerformanceShowGpus, "shell-settings-perf-gpus"),
        (SettingKey::PerformanceShowFans, "shell-settings-perf-fans"),
        (
            SettingKey::PerformanceShowBatteries,
            "shell-settings-perf-batteries",
        ),
    ] {
        page.switch(devices.clone(), key, tr(cx, label), None);
    }
    for (key, label) in [
        (
            SettingKey::PerformanceShowNetworkWired,
            "shell-settings-perf-net-wired",
        ),
        (
            SettingKey::PerformanceShowNetworkWireless,
            "shell-settings-perf-net-wireless",
        ),
        (
            SettingKey::PerformanceShowNetworkVpn,
            "shell-settings-perf-net-vpn",
        ),
        (
            SettingKey::PerformanceShowNetworkVirtual,
            "shell-settings-perf-net-virtual",
        ),
        (
            SettingKey::PerformanceShowNetworkOther,
            "shell-settings-perf-net-other",
        ),
    ] {
        page.switch(network.clone(), key, tr(cx, label), None);
    }
    page.switch(
        devices,
        SettingKey::PerformanceShowDisconnectedFans,
        tr(cx, "shell-settings-perf-disconnected-fans"),
        None,
    );

    let words = Rc::new(ListWords {
        default: tr(cx, "shell-settings-device-default"),
        show: tr(cx, "shell-settings-device-show"),
        hide: tr(cx, "shell-settings-device-hide"),
        reset_order: tr(cx, "shell-settings-device-reset-order"),
        show_all: tr(cx, "shell-settings-device-show-all"),
        order_reset: tr(cx, "shell-toast-sidebar-order-reset"),
    });
    page.custom(
        tr(cx, "shell-settings-sidebar"),
        &[
            SettingKey::PerformanceDeviceOrder,
            SettingKey::PerformanceDeviceOverrides,
        ],
        tr(cx, "shell-settings-device-order"),
        move |window, cx| device_list(&words, window, cx),
    );

    let fan_words = Rc::new(FanWords {
        label: tr(cx, "shell-settings-fan-label"),
        temp: tr(cx, "shell-settings-fan-temp"),
        none: tr(cx, "shell-settings-fan-temp-none"),
    });
    page.custom(
        tr(cx, "shell-settings-perf-group-fans"),
        &[SettingKey::Fans],
        tr(cx, "shell-settings-perf-group-fans"),
        move |window, cx| fans_group(&fan_words, window, cx),
    );
    page
}

/// The fixed words of the device order list, translated when the page is built.
struct ListWords {
    default: SharedString,
    show: SharedString,
    hide: SharedString,
    reset_order: SharedString,
    show_all: SharedString,
    order_reset: SharedString,
}

/// One row of the device order list.
pub struct DeviceListRow {
    pub key: Arc<str>,
    pub name: SharedString,
    /// The device type, as the sidebar card heading names it ("Disk 1", "Wi-Fi").
    pub kind: SharedString,
    /// `device_overrides[key]`; `None` is "Default".
    pub visibility: Option<DeviceVisibility>,
}

/// The rows in sidebar order: `device_order` first, then the other present devices by kind and
/// catalog order. Empty without a hub.
pub fn device_list_rows(cx: &mut App) -> Vec<DeviceListRow> {
    let Some(snap) = cx
        .try_global::<GlobalHub>()
        .map(|hub| Arc::clone(hub.0.read(cx).current()))
    else {
        return Vec::new();
    };
    let p = TlSettings::settings(cx).performance.clone();
    let key = |k: &str| DeviceKey(Arc::from(k));
    let order: Vec<DeviceKey> = p.device_order.iter().map(|k| key(k)).collect();
    let overrides: HashMap<DeviceKey, DeviceVisibility> = p
        .device_overrides
        .iter()
        .map(|(k, v)| (key(k), *v))
        .collect();
    let view = PerformanceSettingsView {
        order: &order,
        overrides: &overrides,
        show_disks: p.show_disks,
        show_gpus: p.show_gpus,
        show_fans: p.show_fans,
        show_disconnected_fans: p.show_disconnected_fans,
        show_batteries: p.show_batteries,
        show_network_wired: p.show_network_wired,
        show_network_wireless: p.show_network_wireless,
        show_network_vpn: p.show_network_vpn,
        show_network_virtual: p.show_network_virtual,
        show_network_other: p.show_network_other,
    };
    let mut rows = Vec::new();
    sidebar_order(&snap.devices, &snap.system.sensors, &view, &mut rows);
    let words = card_words(cx);
    rows.iter()
        .map(|row| DeviceListRow {
            key: row.key.0.clone(),
            name: match row.kind {
                PerfKind::Overview => words.overview.clone(),
                _ => snap
                    .devices
                    .get(row.idx)
                    .map(|e| SharedString::from(e.display_name.to_string()))
                    .unwrap_or_default(),
            },
            kind: kind_text(row, &rows, &words),
            visibility: p.device_overrides.get(&*row.key.0).copied(),
        })
        .collect()
}

/// The type word of a row: the card heading for single devices, the PERF-054 type for network
/// interfaces.
fn kind_text(row: &DeviceRow, rows: &[DeviceRow], words: &CardWords) -> SharedString {
    let numbered = |template: &crate::pages::performance::details::Template| {
        let mut out = String::new();
        let _ = template.write(&mut out, &[&type_number(rows, &row.key)]);
        SharedString::from(out)
    };
    match row.kind {
        PerfKind::Overview => SharedString::default(),
        PerfKind::Cpu => words.cpu.clone(),
        PerfKind::Memory => words.memory.clone(),
        PerfKind::Gpu => numbered(&words.gpu),
        PerfKind::Disk => numbered(&words.disk),
        PerfKind::Fan => numbered(&words.fan),
        PerfKind::Battery => words.battery.clone(),
        PerfKind::AggregateDisk => words.aggregate_storage.clone(),
        PerfKind::AggregateGpu => words.aggregate_graphics.clone(),
        PerfKind::Net => match row.net_class {
            Some(tl_model::devices::NetKind::Wired) => words.ethernet.clone(),
            Some(tl_model::devices::NetKind::Wireless) => words.wifi.clone(),
            Some(tl_model::devices::NetKind::VpnTunnel) => words.vpn.clone(),
            Some(tl_model::devices::NetKind::Virtual) => words.virtual_net.clone(),
            Some(tl_model::devices::NetKind::Other) | None => words.other_net.clone(),
        },
    }
}

/// Writes `performance.device_order` as `keys` with the key at `from` moved to `to`. Does
/// nothing when either index is out of range or they are equal.
pub fn move_device(keys: &[Arc<str>], from: usize, to: usize, cx: &mut App) {
    if from == to || from >= keys.len() || to >= keys.len() {
        return;
    }
    let mut order: Vec<Arc<str>> = keys.to_vec();
    let key = order.remove(from);
    order.insert(to, key);
    let list: Arc<[SharedString]> = order
        .iter()
        .map(|k| SharedString::from(k.to_string()))
        .collect();
    TlSettings::set(
        cx,
        SettingKey::PerformanceDeviceOrder,
        SettingValue::StrList(list),
    );
}

/// Sets `device_overrides[key]`, or removes it for `None` ("Default").
pub fn set_device_visibility(key: &str, visibility: Option<DeviceVisibility>, cx: &mut App) {
    TlSettings::update(cx, |d| {
        let overrides = &mut d.settings.performance.device_overrides;
        match visibility {
            Some(v) => {
                overrides.insert(key.to_owned(), v);
            }
            None => {
                overrides.remove(key);
            }
        }
    });
}

/// The payload of a row drag: the dragged device's key and name. It also draws the drag ghost.
#[derive(Clone)]
struct DraggedDevice {
    key: Arc<str>,
    name: SharedString,
}

impl Render for DraggedDevice {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = current(cx).clone();
        let m = *metrics(cx);
        let mut ghost = div()
            .px(m.sp[4])
            .py(m.sp[1])
            .rounded(m.radius_small)
            .bg(t.menu_bg)
            .border_1()
            .border_color(t.divider)
            .text_color(t.text_primary)
            .child(self.name.clone());
        *ghost.text_style() = t_base(&m);
        ghost
    }
}

/// Row height before the UI scale.
const ROW_H: f32 = 28.;

/// The list rows, then "Reset order" and "Show all".
fn device_list(words: &Rc<ListWords>, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = current(cx).clone();
    let m = *metrics(cx);
    let rows = device_list_rows(cx);
    let keys: Rc<[Arc<str>]> = rows.iter().map(|r| r.key.clone()).collect();
    let hover = t.row_hover;
    let mut list = div()
        .debug_selector(|| "DeviceOrderList".into())
        .flex()
        .flex_col()
        .w_full();
    for (i, row) in rows.into_iter().enumerate() {
        // One focus handle per device, kept across renders, so focus follows a moved row.
        let focus = window
            .use_keyed_state(
                ElementId::Name(format!("device-order-focus-{}", row.key).into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let count = keys.len();
        let (k1, k2) = (keys.clone(), keys.clone());
        let mut name = div()
            .flex_1()
            .min_w_0()
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .text_color(t.text_primary)
            .child(row.name.clone());
        *name.text_style() = t_base(&m);
        let mut kind = div()
            .flex_none()
            .text_color(t.text_secondary)
            .child(row.kind.clone());
        *kind.text_style() = t_sm(&m);
        let handle = div()
            .id(ElementId::Name(
                format!("device-order-handle-{}", row.key).into(),
            ))
            .debug_selector({
                let key = row.key.clone();
                move || format!("DeviceOrderHandle-{key}")
            })
            .flex_none()
            .cursor_grab()
            .child(
                Icon::new(IconName::GripVertical)
                    .size(m.icon.md)
                    .text_color(t.text_secondary),
            )
            .on_drag(
                DraggedDevice {
                    key: row.key.clone(),
                    name: row.name.clone(),
                },
                |dragged, _, _, cx| cx.new(|_| dragged.clone()),
            );
        list = list.child(
            div()
                .id(ElementId::Name(
                    format!("device-order-row-{}", row.key).into(),
                ))
                .debug_selector({
                    let key = row.key.clone();
                    move || format!("DeviceOrderRow-{key}")
                })
                .track_focus(&focus)
                .flex()
                .items_center()
                .gap(m.sp[2])
                .h(gpui_kit::px(ROW_H * m.ui_scale))
                .px(m.sp[2])
                .rounded(m.radius_small)
                .on_key_down(move |ev: &KeyDownEvent, _, cx| {
                    let k = &ev.keystroke;
                    if !k.modifiers.alt {
                        return;
                    }
                    let to = match k.key.as_str() {
                        "up" if i > 0 => i - 1,
                        "down" if i + 1 < count => i + 1,
                        _ => return,
                    };
                    move_device(&k1, i, to, cx);
                    cx.stop_propagation();
                })
                .drag_over::<DraggedDevice>(move |style, _, _, _| style.bg(hover))
                .on_drop(move |dragged: &DraggedDevice, _, cx| {
                    if let Some(from) = k2.iter().position(|k| *k == dragged.key) {
                        move_device(&k2, from, i, cx);
                    }
                })
                .child(handle)
                .child(name)
                .child(kind)
                .child(segments(&row.key, row.visibility, words)),
        );
    }
    let buttons = div()
        .flex()
        .gap(m.sp[2])
        .pt(m.sp[2])
        .child(
            TlButton::new("device-order-reset", TlButtonKind::Secondary)
                .label(words.reset_order.clone())
                .on_click({
                    let toast = words.order_reset.clone();
                    move |_, window, cx| {
                        TlSettings::update(cx, |d| d.settings.performance.device_order.clear());
                        if let Some(queue) = ToastQueue::of_window(window, cx) {
                            let text = toast.clone();
                            queue.update(cx, |q, cx| {
                                q.push(ToastKind::Success, text, None, None, cx)
                            });
                        }
                    }
                }),
        )
        .child(
            TlButton::new("device-order-show-all", TlButtonKind::Secondary)
                .label(words.show_all.clone())
                .on_click(|_, _, cx| {
                    TlSettings::update(cx, |d| d.settings.performance.device_overrides.clear());
                }),
        );
    list.child(buttons).into_any_element()
}

/// The Default / Show / Hide segments of one row.
fn segments(key: &Arc<str>, current: Option<DeviceVisibility>, words: &ListWords) -> AnyElement {
    let mut row = div().flex().flex_none();
    for (i, (label, value)) in [
        (words.default.clone(), None),
        (words.show.clone(), Some(DeviceVisibility::Show)),
        (words.hide.clone(), Some(DeviceVisibility::Hide)),
    ]
    .into_iter()
    .enumerate()
    {
        let key = key.clone();
        row = row.child(
            TlButton::new(
                ElementId::Name(format!("device-vis-{key}-{i}").into()),
                TlButtonKind::Segment {
                    on: current == value,
                },
            )
            .label(label)
            .on_click(move |_, _, cx| set_device_visibility(&key, value, cx)),
        );
    }
    row.into_any_element()
}

/// One section per present GPU of the device catalog: its name, then its engines.
fn engine_sections(cx: &App) -> Vec<CheckSection> {
    let Some(snap) = cx
        .try_global::<GlobalHub>()
        .map(|hub| Arc::clone(hub.0.read(cx).current()))
    else {
        return Vec::new();
    };
    gpu_sections(&snap)
}

fn gpu_sections(snap: &Snapshot) -> Vec<CheckSection> {
    snap.devices
        .present_of(DeviceKind::Gpu)
        .map(|gpu| CheckSection {
            heading: SharedString::from(gpu.display_name.to_string()),
            options: device_engines(snap, gpu.idx)
                .into_iter()
                .map(|(key, name)| (name, SharedString::from(key.to_string())))
                .collect(),
        })
        .collect()
}

/// `(label key, stored value)` pairs as dropdown options.
fn options(cx: &App, pairs: &[(&str, &'static str)]) -> Vec<(SharedString, SettingValue)> {
    pairs
        .iter()
        .map(|(key, value)| (tr(cx, key), SettingValue::Str((*value).into())))
        .collect()
}

// The Fans group (E21-037).

/// The fixed words of the Fans group, translated when the page is built.
struct FanWords {
    label: SharedString,
    temp: SharedString,
    none: SharedString,
}

/// One row of the Fans group.
pub struct FanRowData {
    /// The `fans` key, which is also the sidebar device key.
    pub key: Arc<str>,
    /// The default name, "Fan N", as on the sidebar card.
    pub name: SharedString,
}

/// One present fan per row, in sidebar order. Empty without a hub or without a fan.
pub fn fan_rows(cx: &mut App) -> Vec<FanRowData> {
    device_list_rows(cx)
        .into_iter()
        .filter(|row| DeviceKey(row.key.clone()).kind() == PerfKind::Fan)
        .map(|row| FanRowData {
            key: row.key,
            name: row.kind,
        })
        .collect()
}

/// One option of the sensor dropdown.
pub struct SensorOption {
    /// The stored `temp` value: the sensor key without its `sensor:` prefix.
    pub value: Arc<str>,
    /// "<label> (<chip>) 45 °C", or the glyph word in place of the reading.
    pub text: SharedString,
}

/// Every present hwmon temperature sensor in catalog order. Empty without a hub.
pub fn sensor_options(cx: &mut App) -> Vec<SensorOption> {
    let Some(snap) = cx
        .try_global::<GlobalHub>()
        .map(|hub| Arc::clone(hub.0.read(cx).current()))
    else {
        return Vec::new();
    };
    sensor_options_of(&snap)
}

fn sensor_options_of(snap: &Snapshot) -> Vec<SensorOption> {
    snap.devices
        .entries
        .iter()
        .filter(|e| e.present)
        .filter_map(|e| {
            let DeviceIdentity::Sensor { chip, label } = &e.identity else {
                return None;
            };
            let reading = snap
                .system
                .sensors
                .iter()
                .find(|(idx, r)| *idx == e.idx && r.kind == SensorKind::Temperature)
                .map(|(_, r)| r.value);
            let (validity, c) =
                reading.map_or((Validity::WarmingUp, 0.), |m| (m.validity, m.value));
            let mut text = String::new();
            units::celsius(&mut text, c);
            let reading = copy_text(validity, &SharedString::from(text));
            let value = e
                .persistent_key
                .strip_prefix("sensor:")
                .unwrap_or(&e.persistent_key);
            Some(SensorOption {
                value: Arc::from(value),
                text: format!("{label} ({chip}) {reading}").into(),
            })
        })
        .collect()
}

/// Writes `fans[key].label`, the trimmed text, or removes the field when it is empty. The entry
/// goes when neither field is left.
pub fn set_fan_label(key: &str, text: &str, cx: &mut App) {
    let text = text.trim();
    update_fan(key, cx, |f| {
        f.label = (!text.is_empty()).then(|| text.to_owned());
    });
}

/// Writes `fans[key].temp`, or removes the field for `None`. The entry goes when neither field is
/// left.
pub fn set_fan_temp(key: &str, temp: Option<&str>, cx: &mut App) {
    update_fan(key, cx, |f| f.temp = temp.map(str::to_owned));
}

fn update_fan(key: &str, cx: &mut App, edit: impl FnOnce(&mut FanOverride)) {
    TlSettings::update(cx, |d| {
        let mut fan = d.settings.fans.get(key).cloned().unwrap_or_default();
        edit(&mut fan);
        if fan.label.is_none() && fan.temp.is_none() {
            d.settings.fans.remove(key);
        } else {
            d.settings.fans.insert(key.to_owned(), fan);
        }
    });
}

/// The widgets of one fan row, kept across renders.
struct FanRowState {
    label: gpui_kit::Entity<InputState>,
    temp: gpui_kit::Entity<TlSelectState>,
    _subs: Vec<gpui_kit::Subscription>,
}

/// The dropdown items: "None", every sensor, and the stored sensor when it is no longer present.
fn temp_items(
    options: &[SensorOption],
    stored: Option<&str>,
    words: &FanWords,
) -> Vec<TlSelectItem> {
    let mut items = vec![TlSelectItem::new("", words.none.clone())];
    items.extend(
        options
            .iter()
            .map(|o| TlSelectItem::new(o.value.to_string(), o.text.clone())),
    );
    if let Some(stored) = stored.filter(|s| !options.iter().any(|o| &*o.value == *s)) {
        items.push(TlSelectItem::new(stored.to_owned(), stored.to_owned()));
    }
    items
}

/// The fan rows: the fan's default name, a Label input, then a Temperature sensor dropdown.
/// Nothing when the catalog has no fan.
fn fans_group(words: &Rc<FanWords>, window: &mut Window, cx: &mut App) -> AnyElement {
    let rows = fan_rows(cx);
    let t = current(cx).clone();
    let m = *metrics(cx);
    let mut list = div()
        .debug_selector(|| "FansGroup".into())
        .flex()
        .flex_col()
        .gap(m.sp[2])
        .w_full();
    if rows.is_empty() {
        return list.into_any_element();
    }
    let options = sensor_options(cx);
    let fans = TlSettings::settings(cx).fans.clone();
    for row in rows {
        let stored = fans.get(&*row.key);
        let stored_label = stored.and_then(|f| f.label.clone()).unwrap_or_default();
        let stored_temp = stored.and_then(|f| f.temp.clone());
        let state = window.use_keyed_state(
            ElementId::Name(format!("fan-row-{}", row.key).into()),
            cx,
            {
                let key = row.key.clone();
                let (label, temp) = (stored_label.clone(), stored_temp.clone());
                let items = temp_items(&options, stored_temp.as_deref(), words);
                move |window, cx| {
                    let input = cx.new(|cx| InputState::new(window, cx));
                    input.update(cx, |s, cx| s.set_value(label, window, cx));
                    let select = cx.new(|cx| {
                        TlSelectState::new(items, Some(temp.as_deref().unwrap_or("")), cx)
                    });
                    // Enter or blur applies the text. A change from code does not.
                    let sub =
                        cx.subscribe_in(&input, window, move |_, input, ev: &InputEvent, _, cx| {
                            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                                let text = input.read(cx).value().to_string();
                                set_fan_label(&key, &text, cx);
                            }
                        });
                    FanRowState {
                        label: input,
                        temp: select,
                        _subs: vec![sub],
                    }
                }
            },
        );
        let (input, select) = {
            let st = state.read(cx);
            (st.label.clone(), st.temp.clone())
        };
        // Follow the stored values and the sensor list, which change with the catalog.
        let items = temp_items(&options, stored_temp.as_deref(), words);
        select.update(cx, |s, cx| {
            let same = s.items().len() == items.len()
                && s.items()
                    .iter()
                    .zip(&items)
                    .all(|(a, b)| a.value == b.value && a.label == b.label);
            if !same {
                s.set_items(items, cx);
            }
            s.set_selected_value(stored_temp.as_deref().unwrap_or(""), cx);
        });
        if !input.read(cx).focus_handle(cx).is_focused(window)
            && input.read(cx).value() != stored_label.as_str()
        {
            input.update(cx, |s, cx| s.set_value(stored_label.clone(), window, cx));
        }

        let mut name = div()
            .flex_none()
            .w(gpui_kit::px(96. * m.ui_scale))
            .whitespace_nowrap()
            .text_color(t.text_primary)
            .child(row.name.clone());
        *name.text_style() = t_base(&m);
        let key = row.key.clone();
        let caption = |text: &SharedString| {
            let mut c = div()
                .flex_none()
                .text_color(t.text_secondary)
                .child(text.clone());
            *c.text_style() = t_sm(&m);
            c
        };
        list = list.child(
            div()
                .id(ElementId::Name(format!("fan-row-el-{}", row.key).into()))
                .debug_selector({
                    let key = row.key.clone();
                    move || format!("FanRow-{key}")
                })
                .flex()
                .items_center()
                .gap(m.sp[2])
                .child(name)
                .child(caption(&words.label))
                .child(
                    TlTextInput::new(format!("fan-label-{}", row.key), &input)
                        .placeholder(row.name.clone())
                        .aria_label(words.label.clone())
                        .width(gpui_kit::px(160. * m.ui_scale)),
                )
                .child(caption(&words.temp))
                .child(
                    TlSelect::new(format!("fan-temp-{}", row.key), &select).on_change(
                        move |value, _, cx| {
                            let temp = (!value.is_empty()).then(|| value.to_string());
                            set_fan_temp(&key, temp.as_deref(), cx);
                        },
                    ),
                ),
        );
    }
    list.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::devices::{DeviceCatalog, DeviceEntry};
    use tl_model::ids::DeviceIdx;
    use tl_model::snapshot::SystemSnapshot;
    use tl_model::system::{Metric, SensorReading};
    use tl_model::time::BootId;

    fn sensor(idx: u16, chip: &str, label: &str, key: &str) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity: DeviceIdentity::Sensor {
                chip: Arc::from(chip),
                label: Arc::from(label),
            },
            generation: 0,
            display_name: Arc::from(label),
            present: true,
            parent: None,
            persistent_key: Arc::from(key),
            net_kind: None,
        }
    }

    fn reading(idx: u16, value: Metric<f32>) -> (DeviceIdx, SensorReading) {
        (
            DeviceIdx(idx),
            SensorReading {
                device: DeviceIdx(idx),
                kind: SensorKind::Temperature,
                value,
                pwm_pct: Metric::with(Validity::Unsupported),
                fan_control: None,
                fan_connected: false,
            },
        )
    }

    #[test]
    fn sensor_options_show_reading_or_glyph() {
        let mut snap = Snapshot::empty(BootId(0), 0);
        snap.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![
                sensor(
                    0,
                    "k10temp",
                    "Tctl",
                    "sensor:k10temp:/devices/pci0000:00/x:temp1",
                ),
                sensor(1, "nvme", "Composite", "sensor:nvme::temp1"),
            ]),
        });
        let system = SystemSnapshot {
            sensors: Arc::from(vec![
                reading(0, Metric::valid(45.2)),
                reading(1, Metric::with(Validity::Failed)),
            ]),
            ..SystemSnapshot::default()
        };
        snap.system = Arc::new(system);
        let options = sensor_options_of(&snap);
        assert_eq!(&*options[0].value, "k10temp:/devices/pci0000:00/x:temp1");
        assert_eq!(options[0].text, "Tctl (k10temp) 45 °C");
        assert_eq!(&*options[1].value, "nvme::temp1");
        assert_eq!(options[1].text, "Composite (nvme) !");
    }
}
