//! The View menu of the menu bar (E21-001, UI-SHL-033, UI-SHL-034, APP-017, APP-018).
//!
//! The root lists submenus in a fixed order: "Columns", "Process View", "Window", "Appearance",
//! "Theme", "Graphs". Each appears once its bead adds it. View > Columns acts on the table of the
//! active tab (PROC-024 to PROC-026, R-065, R-075). Performance and Information have no table, so
//! "Columns" is disabled there with the reason as tooltip. View > Process View (E21-002,
//! UI-SHL-035) holds the view mode radios, two process display checks and "Collapse all"; the
//! view mode Actions and their toasts are the Processes page's.

use gpui_kit::{App, Entity, FocusHandle, Window};
use tl_model::command::Tab;
use tl_model::settings::{ColumnResize, ProcessesSettings};
use tl_model::table::TableMode;

use super::main::ShellState;
use crate::actions::{SetSetting, SetViewMode, ToggleSetting};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::table::actions::{CollapseAll, OpenColumnPicker, ResetColumns, ResizeColumnsToFit};
use crate::table::columns::TableId;

/// Builds the View menu. Item actions go to `return_focus`.
pub fn build(window: &mut Window, cx: &mut App, return_focus: &FocusHandle) -> Entity<TlMenuPopup> {
    let table = active_table(cx);
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
