//! The View menu of the menu bar (E21-001, UI-SHL-033, UI-SHL-034, APP-017, APP-018).
//!
//! The root lists submenus in a fixed order: "Columns", "Process View", "Window", "Appearance",
//! "Theme", "Graphs". Each appears once its bead adds it. View > Columns acts on the table of the
//! active tab (PROC-024 to PROC-026, R-065, R-075). Performance and Information have no table, so
//! "Columns" is disabled there with the reason as tooltip. View > Process View (E21-002,
//! UI-SHL-035) holds the view mode radios, two process display checks and "Collapse all"; the
//! view mode Actions and their toasts are the Processes page's. View > Window (E21-008,
//! UI-SHL-036) holds "Always On Top", the menu path to the main window pin; the pin's owner
//! handles the Action. View > Appearance (E21-009, UI-SHL-037) holds the UI Scale, Row Height
//! and intensity slider rows and two checks that keep the menu open; the menu bar owns the
//! slider states and saves their values. View > Graphs (E21-003, UI-SHL-039, R-089) holds the
//! seven minigraph checks and "Reset Minigraph Layout"; the GPU checks are disabled when the
//! device catalog has no GPU.

use gpui_kit::{App, Entity, FocusHandle, SharedString, Window};
use tl_model::command::Tab;
use tl_model::devices::DeviceKind;
use tl_model::settings::{ColumnResize, ProcessesSettings};
use tl_model::table::TableMode;

use super::main::{ShellState, WINDOW_TITLE};
use super::menubar::{HEAT_INTENSITY, MenuSliders, ROW_HEIGHT, SHADOW_INTENSITY, UI_SCALE};
use super::registry::WindowKey;
use super::titlebar::keep_above_supported;
use crate::actions::{SetSetting, SetViewMode, ToggleSetting};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::components::window_frame::ToggleAlwaysOnTop;
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::pages::minigraph::actions::ResetMinigraphLayout;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::table::actions::{CollapseAll, OpenColumnPicker, ResetColumns, ResizeColumnsToFit};
use crate::table::columns::TableId;

/// Builds the View menu. Item actions go to `return_focus`; the slider rows use `sliders`.
pub fn build(
    window: &mut Window,
    cx: &mut App,
    return_focus: &FocusHandle,
    sliders: &MenuSliders,
) -> Entity<TlMenuPopup> {
    let table = active_table(cx);
    let sliders = sliders.clone();
    TlMenu::build(window, cx, return_focus.clone(), move |menu, window, cx| {
        let columns = tr(cx, "shell-menu-view-columns");
        let menu = match table {
            Some(table) => menu.submenu(columns, window, cx, move |menu, window, cx| {
                columns_submenu(menu, table, window, cx)
            }),
            None => menu.item_disabled(columns, Some(tr(cx, "shell-menu-view-no-table"))),
        };
        menu.submenu(
            tr(cx, "shell-menu-view-process-view"),
            window,
            cx,
            process_view_submenu,
        )
        .submenu(tr(cx, "shell-menu-view-window"), window, cx, window_submenu)
        .submenu(
            tr(cx, "shell-menu-view-appearance"),
            window,
            cx,
            move |menu, _, cx| appearance_submenu(menu, &sliders, cx),
        )
        .submenu(tr(cx, "shell-menu-view-graphs"), window, cx, graphs_submenu)
    })
}

/// The table of the active tab: Processes, or History while History mode is on, Services,
/// Startup Apps and Connections. `None` on Performance and Information.
pub fn active_table(cx: &App) -> Option<TableId> {
    let (tab, history) = cx
        .try_global::<ShellState>()
        .map_or((Tab::Processes, false), |s| (s.active_tab, s.history_mode));
    match tab {
        Tab::Processes if history => Some(TableId::History),
        Tab::Processes => Some(TableId::Processes),
        Tab::Services => Some(TableId::Services),
        Tab::Startup => Some(TableId::Startup),
        Tab::Connections => Some(TableId::Connections),
        Tab::Performance | Tab::Information => None,
    }
}

/// "Select Columns…", "Resize columns to fit content", "Resize Behavior", a separator and
/// "Reset to default", all on `table`. "Hide This Column" is in the header menu only (E18).
fn columns_submenu(menu: TlMenu, table: TableId, window: &mut Window, cx: &mut App) -> TlMenu {
    menu.item(
        tr(cx, "shell-menu-columns-select"),
        Box::new(OpenColumnPicker { table }),
    )
    .item(
        tr(cx, "shell-menu-columns-fit"),
        Box::new(ResizeColumnsToFit { table }),
    )
    .submenu(
        tr(cx, "shell-menu-columns-resize-behavior"),
        window,
        cx,
        resize_behavior_submenu,
    )
    .separator()
    .item(
        tr(cx, "shell-menu-columns-reset"),
        Box::new(ResetColumns { table }),
    )
}

/// The radio marks of "Proportional" and "Last Column": which one is `tables.column_resize`.
pub fn resize_marks(cx: &App) -> [bool; 2] {
    let current = cx
        .try_global::<TlSettings>()
        .map_or_else(ColumnResize::default, |_| {
            TlSettings::settings(cx).tables.column_resize
        });
    [
        current == ColumnResize::Proportional,
        current == ColumnResize::LastColumn,
    ]
}

/// Two radio items bound to `tables.column_resize`.
fn resize_behavior_submenu(menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let [proportional, last_column] = resize_marks(cx);
    let set = |value: &'static str| {
        Box::new(SetSetting {
            key: SettingKey::TablesColumnResize,
            value: SettingValue::Str(value.into()),
        })
    };
    menu.radio_item(
        tr(cx, "shell-menu-columns-proportional"),
        proportional,
        set("proportional"),
    )
    .radio_item(
        tr(cx, "shell-menu-columns-last-column"),
        last_column,
        set("last_column"),
    )
}

/// The settings the Process View items show: `processes.mode`, the two checks.
fn processes_settings(cx: &App) -> ProcessesSettings {
    cx.try_global::<TlSettings>()
        .map_or_else(ProcessesSettings::default, |_| {
            TlSettings::settings(cx).processes.clone()
        })
}

/// The radio marks of "Tree View", "Task Manager Grouping" and "List View": which one is
/// `processes.mode`.
pub fn view_mode_marks(cx: &App) -> [bool; 3] {
    let mode = processes_settings(cx).mode;
    VIEW_MODES.map(|(_, m)| m == mode)
}

const VIEW_MODES: [(&str, TableMode); 3] = [
    ("shell-menu-pv-tree", TableMode::Tree),
    ("shell-menu-pv-grouped", TableMode::Grouped),
    ("shell-menu-pv-list", TableMode::Flat),
];

/// The view mode radios (disabled while History mode is on, which shows a flat list), the
/// connector lines and highlight checks, and "Collapse all" (Processes tab only). Enabled on
/// every tab, since its settings apply when the Processes tab shows.
fn process_view_submenu(mut menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let p = processes_settings(cx);
    let (tab, history) = cx
        .try_global::<ShellState>()
        .map_or((Tab::Processes, false), |s| (s.active_tab, s.history_mode));
    let marks = view_mode_marks(cx);
    for ((key, mode), on) in VIEW_MODES.into_iter().zip(marks) {
        menu = if history {
            menu.item_disabled(tr(cx, key), Some(tr(cx, "shell-menu-pv-history-flat")))
        } else {
            menu.radio_item(tr(cx, key), on, Box::new(SetViewMode(mode)))
        };
    }
    menu = menu
        .separator()
        .check_item(
            tr(cx, "shell-menu-pv-guides"),
            p.tree_guide_lines,
            Box::new(ToggleSetting(SettingKey::ProcessesTreeGuideLines)),
        )
        .check_item(
            tr(cx, "shell-menu-pv-highlight"),
            p.highlight_new_exited,
            Box::new(ToggleSetting(SettingKey::ProcessesHighlightNewExited)),
        )
        .separator();
    let collapse = tr(cx, "shell-menu-pv-collapse-all");
    if tab == Tab::Processes {
        menu.item(collapse, Box::new(CollapseAll))
    } else {
        menu.item_disabled(collapse, None)
    }
}

/// "Always On Top": `(enabled, checked)`. Enabled when the caption pin can work
/// ([`keep_above_supported`]); checked from `window.always_on_top`.
pub fn always_on_top_marks(cx: &App) -> (bool, bool) {
    (
        keep_above_supported(cx),
        TlSettings::settings(cx).window.always_on_top,
    )
}

fn window_submenu(menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let label = tr(cx, "shell-menu-window-always-on-top");
    match always_on_top_marks(cx) {
        (true, checked) => menu.check_item(
            label,
            checked,
            Box::new(ToggleAlwaysOnTop {
                window: WindowKey::Main,
                caption: WINDOW_TITLE.into(),
            }),
        ),
        (false, _) => menu.item_disabled(label, Some(tr(cx, "fnd-pin-unsupported"))),
    }
}

/// A slider value as a whole percent, "120%".
fn pct_text(v: f32) -> SharedString {
    format!("{v:.0}%").into()
}

fn appearance_submenu(menu: TlMenu, sliders: &MenuSliders, cx: &mut App) -> TlMenu {
    let appearance = &TlSettings::settings(cx).appearance;
    let (rounded, heat) = (appearance.rounded_corners, appearance.heat_coloring);
    menu.slider_row(
        tr(cx, "shell-menu-appearance-ui-scale"),
        &sliders.ui_scale,
        UI_SCALE.range,
        pct_text,
    )
    .slider_row(
        tr(cx, "shell-menu-appearance-row-height"),
        &sliders.row_height,
        ROW_HEIGHT.range,
        pct_text,
    )
    .check_item(
        tr(cx, "shell-menu-appearance-rounded-corners"),
        rounded,
        Box::new(ToggleSetting(SettingKey::AppearanceRoundedCorners)),
    )
    .keep_open()
    .check_item(
        tr(cx, "shell-menu-appearance-heat-coloring"),
        heat,
        Box::new(ToggleSetting(SettingKey::AppearanceHeatColoring)),
    )
    .keep_open()
    .slider_row(
        tr(cx, "shell-menu-appearance-heat-intensity"),
        &sliders.heat_intensity,
        HEAT_INTENSITY.range,
        pct_text,
    )
    .slider_row(
        tr(cx, "shell-menu-appearance-shadow-intensity"),
        &sliders.shadow_intensity,
        SHADOW_INTENSITY.range,
        pct_text,
    )
}

/// The seven minigraph checks: label key, setting key, and whether the item needs a GPU.
const GRAPHS: [(&str, SettingKey, bool); 7] = [
    (
        "shell-menu-graphs-cpu",
        SettingKey::GraphsMinigraphCpu,
        false,
    ),
    (
        "shell-menu-graphs-memory",
        SettingKey::GraphsMinigraphMemory,
        false,
    ),
    (
        "shell-menu-graphs-disk",
        SettingKey::GraphsMinigraphDisk,
        false,
    ),
    (
        "shell-menu-graphs-network",
        SettingKey::GraphsMinigraphNetwork,
        false,
    ),
    (
        "shell-menu-graphs-gpu",
        SettingKey::GraphsMinigraphGpu,
        true,
    ),
    (
        "shell-menu-graphs-vram",
        SettingKey::GraphsMinigraphVram,
        true,
    ),
    (
        "shell-menu-graphs-gpu-temp",
        SettingKey::GraphsMinigraphGpuTemperature,
        true,
    ),
];

/// The check marks of the seven minigraph items, in menu order, from `graphs.minigraph_*`.
pub fn graphs_marks(cx: &App) -> [bool; 7] {
    let g = &TlSettings::settings(cx).graphs;
    [
        g.minigraph_cpu,
        g.minigraph_memory,
        g.minigraph_disk,
        g.minigraph_network,
        g.minigraph_gpu,
        g.minigraph_vram,
        g.minigraph_gpu_temperature,
    ]
}

/// Whether the device catalog of the installed snapshot holds a present GPU. False without a hub.
pub fn has_gpu(cx: &App) -> bool {
    cx.try_global::<GlobalHub>().is_some_and(|hub| {
        hub.0
            .read(cx)
            .current()
            .devices
            .present_of(DeviceKind::Gpu)
            .next()
            .is_some()
    })
}

/// The seven minigraph checks, a separator and "Reset Minigraph Layout". The GPU, VRAM and GPU
/// Temperature checks are disabled with "No GPU found" when [`has_gpu`] is false. E21-034 adds
/// its P3 item between the separator and Reset.
fn graphs_submenu(mut menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let gpu = has_gpu(cx);
    let marks = graphs_marks(cx);
    for ((label, key, needs_gpu), checked) in GRAPHS.into_iter().zip(marks) {
        menu = if needs_gpu && !gpu {
            menu.item_disabled(tr(cx, label), Some(tr(cx, "shell-settings-perf-no-gpu")))
        } else {
            menu.check_item(tr(cx, label), checked, Box::new(ToggleSetting(key)))
        };
    }
    menu.separator().item(
        tr(cx, "shell-menu-graphs-reset"),
        Box::new(ResetMinigraphLayout),
    )
}
