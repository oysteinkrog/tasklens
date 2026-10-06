//! Window-level Actions of the shell and the Global key bindings (E20-002, UI-SHL-120,
//! canonical table (b) Global rows, R-045, R-128, R-136, D-008).
//!
//! One command is one Action: a menu item, a toolbar button and a key dispatch the same one.
//! Every Action here lives in the one `tasklens` namespace (R-128). Handlers live with the code
//! that owns the behaviour; this file registers only the two generic setting handlers.
//!
//! Declared by their owners, not here (GPUI fails on a duplicate name in one namespace):
//! - `StartHelperSession`, `EndHelperSession`: `components/shield.rs` (E16-010), re-exported
//!   below.
//! - `ToggleAlwaysOnTop { window: WindowKey }`, `CloseSecondaryWindow`:
//!   `components/window_frame.rs` (E16-008).
//! - `OpenProperties` and the other table Actions: `table/actions.rs` (E18).
//! - `ActivateTheme`: `theme/store.rs` (E17).
//! - Page Actions: each page's `actions.rs`.

use gpui_kit::{Action, App, KeyBinding};
use tl_model::table::TableMode;

use crate::components::checkbox;
pub use crate::components::shield::{EndHelperSession, StartHelperSession};
use crate::settings_global::{SettingKey, SettingValue, TlSettings};

gpui_kit::actions!(
    tasklens,
    [
        NextTab,
        PrevTab,
        OpenRunDialog,
        OpenHandleSearch,
        Quit,
        CloseWindow,
        MinimizeWindow,
        ZoomWindow,
        OpenSettings,
        OpenShortcuts,
        FocusFilter,
        ClearFilter,
        FilterEscape,
        FilterAccept,
        RefreshCurrentTab,
        EscapeChain,
        ScaleUp,
        ScaleDown,
        ScaleReset,
        ToggleMiniWindow,
        StartWindowPick,
        CancelWindowPick,
        ToggleHideSystem,
        CycleViewMode,
        ToggleHistoryMode,
        OpenCollectorHealth,
        ShowSystemInformation,
        ShowMainWindow,
        ToggleMainWindow,
    ]
);

/// Key context of the main window and every shell window.
pub const GLOBAL_CONTEXT: &str = "Global";
/// Key context pushed while window picking runs (E20-027).
pub const PICKER_CONTEXT: &str = "Picker";
/// Key context the filter box sets on its input (E20-009).
pub const SHELL_FILTER_CONTEXT: &str = "ShellFilter";

/// Selects a tab by its index into `tl_model::command::Tab::ALL` (0 = Processes ..
/// 5 = Information).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SelectTab(pub usize);

/// A top-level menu of the menu bar.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MenuId {
    File,
    View,
    Settings,
    Help,
    Debug,
}

/// Opens a top-level menu (F10 and the Alt mnemonics).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct OpenMenu(pub MenuId);

/// Sets the Processes table layout. Handled by E20-017.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetViewMode(pub TableMode);

/// Flips a Bool setting through `TlSettings::toggle`.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct ToggleSetting(pub SettingKey);

/// Writes one setting through `TlSettings::set`.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetSetting {
    pub key: SettingKey,
    pub value: SettingValue,
}

/// Binds the Global rows of canonical table (b), the Picker and ShellFilter rows, and the
/// component keys (checkbox, radio, slider and the others in `bind_keys_components`). Runs
/// once at init, before any menu is built. A binding whose Action has no handler yet does
/// nothing.
pub fn bind_keys_global(cx: &mut App) {
    checkbox::bind_keys_components(cx);

    let global = Some(GLOBAL_CONTEXT);
    let mut bindings = Vec::new();
    for i in 0..6 {
        bindings.push(KeyBinding::new(
            &format!("ctrl-{}", i + 1),
            SelectTab(i),
            global,
        ));
        bindings.push(KeyBinding::new(
            &format!("alt-{}", i + 1),
            SelectTab(i),
            global,
        ));
    }
    let menus = [
        ("alt-f", MenuId::File),
        ("alt-v", MenuId::View),
        ("alt-s", MenuId::Settings),
        ("alt-h", MenuId::Help),
        ("alt-d", MenuId::Debug),
    ];
    for (keys, menu) in menus {
        bindings.push(KeyBinding::new(keys, OpenMenu(menu), global));
    }
    cx.bind_keys(bindings);

    cx.bind_keys([
        KeyBinding::new("ctrl-tab", NextTab, global),
        KeyBinding::new("ctrl-shift-tab", PrevTab, global),
        KeyBinding::new("ctrl-n", OpenRunDialog, global),
        KeyBinding::new("ctrl-q", Quit, global),
        KeyBinding::new("ctrl-w", CloseWindow, global),
        KeyBinding::new("ctrl-,", OpenSettings, global),
        KeyBinding::new("ctrl-?", OpenShortcuts, global),
        KeyBinding::new("ctrl-shift-/", OpenShortcuts, global),
        KeyBinding::new("f10", OpenMenu(MenuId::File), global),
        KeyBinding::new("ctrl-f", FocusFilter, global),
        KeyBinding::new("f5", RefreshCurrentTab, global),
        KeyBinding::new("escape", EscapeChain, global),
        KeyBinding::new("ctrl-=", ScaleUp, global),
        KeyBinding::new("ctrl-+", ScaleUp, global),
        KeyBinding::new("ctrl-shift-=", ScaleUp, global),
        KeyBinding::new("ctrl--", ScaleDown, global),
        KeyBinding::new("ctrl-0", ScaleReset, global),
        KeyBinding::new("ctrl-shift-m", ToggleMiniWindow, global),
        KeyBinding::new("escape", CancelWindowPick, Some(PICKER_CONTEXT)),
        KeyBinding::new("escape", FilterEscape, Some(SHELL_FILTER_CONTEXT)),
        KeyBinding::new("enter", FilterAccept, Some(SHELL_FILTER_CONTEXT)),
        KeyBinding::new("down", FilterAccept, Some(SHELL_FILTER_CONTEXT)),
    ]);
}

/// Registers the two generic setting handlers: `ToggleSetting` and `SetSetting`.
pub fn register_global_handlers(cx: &mut App) {
    cx.on_action(|a: &ToggleSetting, cx| TlSettings::toggle(cx, a.0));
    cx.on_action(|a: &SetSetting, cx| TlSettings::set(cx, a.key, a.value.clone()));
}
