//! The View menu of the menu bar (E21-001, UI-SHL-033, UI-SHL-034, APP-017, APP-018).
//!
//! The root lists submenus in a fixed order: "Columns", "Process View", "Window", "Appearance",
//! "Theme", "Graphs". Each appears once its bead adds it. View > Columns acts on the table of the
//! active tab (PROC-024 to PROC-026, R-065, R-075). Performance and Information have no table, so
//! "Columns" is disabled there with the reason as tooltip. View > Process View (E21-002,
//! UI-SHL-035) holds the view mode radios, two process display checks and "Collapse all"; the
//! view mode Actions and their toasts are the Processes page's. View > Window (E21-008,
//! UI-SHL-036) holds "Always On Top", the menu path to the main window pin, and "Performance mini
//! window" (E21-034), a check that follows the registry; the owners handle the Actions. View > Appearance (E21-009, UI-SHL-037) holds the UI Scale, Row Height
//! and intensity slider rows and two checks that keep the menu open; the menu bar owns the
//! slider states and saves their values. View > Graphs (E21-003, UI-SHL-039, R-089) holds the
//! seven minigraph checks and "Reset Minigraph Layout"; the GPU checks are disabled when the
//! device catalog has no GPU. View > Theme (E21-012, UI-SHL-038, UI-SHL-080) lists every theme
//! painted in its own colours, then "Sync theme with system" and the two preferred-theme
//! submenus. `ActivateTheme` (declared in `theme/store.rs`) is handled here: an explicit choice
//! writes `theme.active` and turns sync off; turning sync on applies the preferred theme of the
//! current portal scheme at once.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, Entity, FocusHandle, SharedString, Window, gpui};
use serde::Deserialize;
use tl_model::command::Tab;
use tl_model::devices::DeviceKind;
use tl_model::settings::{ColumnResize, ProcessesSettings};
use tl_model::table::TableMode;

use super::main::{ShellState, WINDOW_TITLE};
use super::menubar::{HEAT_INTENSITY, MenuSliders, ROW_HEIGHT, SHADOW_INTENSITY, UI_SCALE};
use super::registry::{GlobalRegistry, WindowKey};
use super::titlebar::keep_above_supported;
use crate::actions::{SetSetting, SetViewMode, ToggleMiniWindow, ToggleSetting};
use crate::components::icons::TlIcon;
use crate::components::menu::{MenuSuffixAction, TlMenu, TlMenuPopup};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::ToggleAlwaysOnTop;
use crate::hub::GlobalHub;
use crate::i18n::{tr, tr_args};
use crate::pages::minigraph::actions::ResetMinigraphLayout;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::table::actions::{CollapseAll, OpenColumnPicker, ResetColumns, ResizeColumnsToFit};
use crate::table::columns::TableId;
use crate::theme::share_code::{ShareCodeError, import_text};
use crate::theme::store::{self, ActivateSource, ActivateTheme, ThemeError, ThemeStore};
use crate::theme::{ThemeDef, ThemeEvent};
use crate::window::confirm::confirm_delete_theme;
use crate::window::share_theme::ShareTheme;
use crate::window::theme_editor::EditTheme;

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct CopyTheme {
    pub name: SharedString,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct DeleteTheme {
    pub name: SharedString,
}

gpui_kit::actions!(
    tasklens,
    [
        /// View > Theme > Import theme: adds the themes in the clipboard text (E21-031).
        ImportTheme
    ]
);

/// Registers the `ActivateTheme` handler and the follow-system observer. `TasklensUi::init`
/// calls it after `TlSettings::init`.
pub fn init(cx: &mut App) {
    cx.on_action(|a: &ActivateTheme, cx| activate_theme(&a.name, cx));
    cx.on_action(|a: &CopyTheme, cx| copy_theme(&a.name, cx));
    cx.on_action(|a: &DeleteTheme, cx| delete_theme(&a.name, cx));
    cx.on_action(|_: &ImportTheme, cx| import_theme(cx));
    let following = Rc::new(Cell::new(follow_system(cx)));
    cx.observe_global::<TlSettings>(move |cx| {
        let now = follow_system(cx);
        if following.replace(now) || !now {
            return;
        }
        let theme = TlSettings::settings(cx).theme.clone();
        if let Some(Err(e)) = store::on_follow_system_enabled(&theme, cx) {
            tracing::warn!(target: "tasklens::ui", ?e, "follow-system theme not applied");
        }
    })
    .detach();
}

fn copy_theme(name: &str, cx: &mut App) {
    match ThemeStore::copy_theme(name, cx) {
        Ok(crate::theme::ThemeEvent::Copied { name }) => {
            TlSettings::update(cx, |d| d.settings.theme.active = name.to_string());
            toast(ToastKind::Success, "shell-toast-theme-copied", cx);
        }
        Err(ThemeError::PreviewActive) => {
            toast(ToastKind::Info, "shell-toast-theme-editor-busy", cx);
        }
        Err(e) => tracing::warn!(target: "tasklens::ui", ?e, name, "theme not copied"),
        Ok(event) => tracing::warn!(target: "tasklens::ui", ?event, "unexpected copy event"),
    }
}

/// The themes in the clipboard text, named so none clashes with an existing theme (user or
/// built-in) or an earlier one of the batch: a taken name gets " 2", " 3" and so on. The text is
/// untrusted and is never logged. Has no side effects.
pub(crate) fn import_from_clipboard(cx: &App) -> Result<Vec<ThemeDef>, ShareCodeError> {
    let text = cx
        .read_from_clipboard()
        .and_then(|c| c.text())
        .ok_or(ShareCodeError::NotAThemeCode)?;
    let store = cx.global::<ThemeStore>();
    import_defs(&text, |n| store.resolve(n).is_some())
}

fn import_defs(text: &str, taken: impl Fn(&str) -> bool) -> Result<Vec<ThemeDef>, ShareCodeError> {
    let mut defs = import_text(text)?;
    if defs.is_empty() {
        return Err(ShareCodeError::NotAThemeCode);
    }
    let mut used: Vec<Arc<str>> = Vec::new();
    for def in &mut defs {
        let clash = |n: &str| taken(n) || used.iter().any(|u| &**u == n);
        if clash(&def.name) {
            let base = def.name.clone();
            def.name = (2..)
                .map(|n| format!("{base} {n}"))
                .find(|n| !clash(n))
                .expect("a free suffix exists")
                .into();
        }
        used.push(def.name.clone());
    }
    Ok(defs)
}

fn import_theme(cx: &mut App) {
    if cx.global::<ThemeStore>().preview.is_some() {
        toast(ToastKind::Info, "shell-toast-theme-editor-busy", cx);
        return;
    }
    let defs = match import_from_clipboard(cx) {
        Ok(defs) => defs,
        Err(e) => {
            tracing::debug!(target: "tasklens::ui", ?e, "clipboard theme not imported");
            toast(ToastKind::Info, "shell-toast-theme-no-code", cx);
            return;
        }
    };
    let first = defs[0].name.clone();
    match ThemeStore::import_themes(defs, cx) {
        Ok(ThemeEvent::Imported { count }) => {
            if let Err(e) = store::activate(&first, ActivateSource::User, cx) {
                tracing::warn!(target: "tasklens::ui", ?e, "imported theme not activated");
            }
            TlSettings::update(cx, |d| d.settings.theme.active = first.to_string());
            if count == 1 {
                toast(ToastKind::Success, "shell-toast-theme-imported", cx);
            } else {
                let text = tr_args(
                    cx,
                    "shell-toast-themes-imported",
                    &[("count", count.into())],
                );
                toast_text(ToastKind::Success, text, cx);
            }
        }
        Ok(_) | Err(_) => toast(ToastKind::Info, "shell-toast-theme-no-code", cx),
    }
}

fn delete_theme(name: &str, cx: &mut App) {
    let Some((_, builtin)) = cx.global::<ThemeStore>().resolve(name) else {
        tracing::warn!(target: "tasklens::ui", name, "theme to delete does not exist");
        return;
    };
    if builtin {
        toast(ToastKind::Warning, "shell-toast-theme-builtin-delete", cx);
        return;
    }
    let Some(window) = cx.active_window() else {
        tracing::warn!(target: "tasklens::ui", "no window for the delete theme confirmation");
        return;
    };
    let name: SharedString = name.to_owned().into();
    cx.defer(move |cx| {
        if window
            .update(cx, |_, window, cx| confirm_delete_theme(name, window, cx))
            .is_err()
        {
            tracing::warn!(target: "tasklens::ui", "delete theme: the window is gone");
        }
    });
}

fn follow_system(cx: &App) -> bool {
    cx.try_global::<TlSettings>()
        .is_some_and(|_| TlSettings::settings(cx).theme.follow_system)
}

/// An explicit theme choice (UI-SHL-080). Refused while the Theme Editor previews a theme.
/// Writes `theme.active`, and turns sync off with an Info toast when it was on, so the next
/// portal change does not overwrite the choice. The switch itself shows no toast.
fn activate_theme(name: &str, cx: &mut App) {
    match store::activate(name, ActivateSource::User, cx) {
        Ok(_) => {
            let was_following = follow_system(cx);
            TlSettings::update(cx, |d| {
                d.settings.theme.active = name.to_owned();
                d.settings.theme.follow_system = false;
            });
            if was_following {
                toast(ToastKind::Info, "shell-toast-theme-sync-off", cx);
            }
        }
        Err(ThemeError::PreviewActive) => {
            toast(ToastKind::Info, "shell-toast-theme-editor-busy", cx);
        }
        Err(e) => tracing::warn!(target: "tasklens::ui", ?e, name, "theme not activated"),
    }
}

/// Pushes the toast `key` in the active window. Deferred: the window is busy dispatching the
/// menu action.
fn toast(kind: ToastKind, key: &str, cx: &mut App) {
    let text = tr(cx, key);
    toast_text(kind, text, cx);
}

/// Like `toast`, with ready text.
fn toast_text(kind: ToastKind, text: SharedString, cx: &mut App) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| q.push(kind, text, None, None, cx));
            }
        });
    });
}

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
        .submenu(tr(cx, "shell-menu-view-theme"), window, cx, theme_submenu)
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

/// Whether the window registry holds the performance mini window. False when no registry is
/// installed.
pub fn mini_window_open(cx: &App) -> bool {
    cx.try_global::<GlobalRegistry>()
        .is_some_and(|r| r.0.read(cx).get(&WindowKey::Mini).is_some())
}

fn window_submenu(menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let label = tr(cx, "shell-menu-window-always-on-top");
    let menu = match always_on_top_marks(cx) {
        (true, checked) => menu.check_item(
            label,
            checked,
            Box::new(ToggleAlwaysOnTop {
                window: WindowKey::Main,
                caption: WINDOW_TITLE.into(),
            }),
        ),
        (false, _) => menu.item_disabled(label, Some(tr(cx, "fnd-pin-unsupported"))),
    };
    menu.check_item(
        tr(cx, "shell-menu-window-mini"),
        mini_window_open(cx),
        Box::new(ToggleMiniWindow),
    )
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
/// Temperature checks are disabled with "No GPU found" when [`has_gpu`] is false. The
/// separator is followed by "Keep detached graphs open when closing main window" (E21-034) and
/// Reset.
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
    let keep = TlSettings::settings(cx).graphs.keep_detached_on_close;
    menu.separator()
        .check_item(
            tr(cx, "shell-menu-graphs-keep-detached"),
            keep,
            Box::new(ToggleSetting(SettingKey::GraphsKeepDetachedOnClose)),
        )
        .item(
            tr(cx, "shell-menu-graphs-reset"),
            Box::new(ResetMinigraphLayout),
        )
}

/// One theme row: its name as stored, its menu label, row colours and whether it carries the
/// radio mark.
pub struct ThemeRow {
    pub name: SharedString,
    pub label: SharedString,
    pub bg: gpui_kit::Hsla,
    pub fg: gpui_kit::Hsla,
    pub selected: bool,
}

/// The theme rows in menu order: built-ins in TaskSlinger order, then user themes in file order.
/// A user theme with a built-in name reads "<name> (custom)". The theme that renders carries
/// the mark; while the Theme Editor previews it, it reads "<name> (editing)".
pub fn theme_rows(cx: &App) -> Vec<ThemeRow> {
    let theme_store = cx.global::<ThemeStore>();
    let shown = store::current(cx);
    let editing = theme_store.preview.is_some();
    let (custom, editing_suffix) = (
        tr(cx, "shell-menu-theme-custom"),
        tr(cx, "shell-menu-theme-editing"),
    );
    store::names(cx)
        .into_iter()
        .filter_map(|(name, builtin)| {
            let def = if builtin {
                theme_store.builtin.iter().find(|d| d.name == name)
            } else {
                theme_store.user.iter().find(|d| d.name == name)
            }?;
            let shadows = !builtin && theme_store.builtin.iter().any(|d| d.name == name);
            let selected = shown.name == name && shown.builtin == builtin;
            let mut label = name.to_string();
            if shadows {
                label.push_str(&custom);
            }
            if selected && editing {
                label.push_str(&editing_suffix);
            }
            Some(ThemeRow {
                name: SharedString::from(name.to_string()),
                label: label.into(),
                bg: def.slots[crate::theme::ThemeSlot::MenuBg as usize],
                fg: def.slots[crate::theme::ThemeSlot::TextPrimary as usize],
                selected,
            })
        })
        .collect()
}

/// Every theme name once, in menu order, for the preferred-theme radios.
fn theme_names(cx: &App) -> Vec<SharedString> {
    let mut names: Vec<SharedString> = Vec::new();
    for (name, _) in store::names(cx) {
        if !names.iter().any(|n| n.as_ref() == &*name) {
            names.push(SharedString::from(name.to_string()));
        }
    }
    names
}

/// The theme rows, copy and delete actions, then sync and the preferred-theme submenus.
fn theme_submenu(mut menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
    let copy_label = tr(cx, "shell-menu-theme-create-copy");
    for row in theme_rows(cx) {
        let copy = CopyTheme {
            name: row.name.clone(),
        };
        menu = menu
            .painted_radio_item(
                row.label,
                row.bg,
                row.fg,
                Some(TlIcon::Copy),
                row.selected,
                Box::new(ActivateTheme { name: row.name }),
            )
            .painted_suffix_action(
                MenuSuffixAction::new(Box::new(copy), copy_label.clone(), copy_label.clone())
                    .keep_open(),
            );
    }
    let active: SharedString = store::current(cx).name.to_string().into();
    let follow = TlSettings::settings(cx).theme.follow_system;
    menu.separator()
        .item(
            copy_label,
            Box::new(CopyTheme {
                name: active.clone(),
            }),
        )
        .item(
            tr(cx, "shell-menu-theme-edit"),
            Box::new(EditTheme {
                name: active.clone(),
            }),
        )
        .item(
            tr(cx, "shell-menu-theme-share"),
            Box::new(ShareTheme {
                name: active.clone(),
            }),
        )
        .item(
            tr(cx, "shell-menu-theme-delete"),
            Box::new(DeleteTheme { name: active }),
        )
        .separator()
        .item(tr(cx, "shell-menu-theme-import"), Box::new(ImportTheme))
        .check_item(
            tr(cx, "shell-menu-theme-sync"),
            follow,
            Box::new(ToggleSetting(SettingKey::ThemeFollowSystem)),
        )
        .submenu(
            tr(cx, "shell-menu-theme-light"),
            window,
            cx,
            |menu, _, cx| preferred_submenu(menu, SettingKey::ThemeLight, cx),
        )
        .submenu(
            tr(cx, "shell-menu-theme-dark"),
            window,
            cx,
            |menu, _, cx| preferred_submenu(menu, SettingKey::ThemeDark, cx),
        )
}

/// Every theme name as a radio bound to `theme.light` or `theme.dark`.
fn preferred_submenu(mut menu: TlMenu, key: SettingKey, cx: &mut App) -> TlMenu {
    let theme = &TlSettings::settings(cx).theme;
    let current = match key {
        SettingKey::ThemeLight => theme.light.clone(),
        _ => theme.dark.clone(),
    };
    for name in theme_names(cx) {
        let on = name.as_ref() == current;
        menu = menu.radio_item(
            name.clone(),
            on,
            Box::new(SetSetting {
                key,
                value: SettingValue::Str(name),
            }),
        );
    }
    menu
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn import_fuzz_no_panic(s in any::<String>()) {
            for taken in [false, true] {
                for text in [s.clone(), format!("TL1.{s}")] {
                    let _ = import_defs(&text, |_| taken);
                }
                let ts1 = format!("TS1.{s}");
                prop_assert!(import_defs(&ts1, |_| taken).is_err());
            }
        }
    }
}
