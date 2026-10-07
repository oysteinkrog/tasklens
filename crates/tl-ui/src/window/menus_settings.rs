//! The Settings menu of the menu bar (E21-004, UI-SHL-040, UI-SHL-031, UI-SHL-131, APP-022,
//! APP-043, R-015, R-017, R-024, R-081, R-132).
//!
//! P1 rows under group headers: Monitoring (Refresh, Network transfer unit, Auto-scale),
//! Motion & interaction (Animations, Smooth Graphs, Smooth Scroll, Close with Esc), Window &
//! startup (Default tab) and Language, then "Reset Settings to Defaults". Later beads add the
//! P2 to P4 rows (E21-010, E21-011, E21-033, E21-040, E21-042). A header shows only when a row
//! under it exists.

use gpui_kit::{App, Entity, FocusHandle, SharedString, Window};
use tl_model::command::Tab;
use tl_model::settings::{NetworkUnit, PROCESS_INTERVAL_MENU_MS, Settings, StartTab};

use super::confirm::ResetSettings;
use super::main::ShellState;
use crate::actions::{SetSetting, ToggleSetting};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};

/// The Refresh items: label key and `processes.interval_ms`, in `PROCESS_INTERVAL_MENU_MS` order.
const REFRESH_KEYS: [&str; 6] = [
    "shell-menu-settings-refresh-250",
    "shell-menu-settings-refresh-500",
    "shell-menu-settings-refresh-1000",
    "shell-menu-settings-refresh-2000",
    "shell-menu-settings-refresh-5000",
    "shell-menu-settings-refresh-10000",
];

/// The locales shipped in this build after "System default": `general.language` value and the
/// label key with the locale's own name. English only this round (R-132).
pub(crate) const SHIPPED_LANGUAGES: [(&str, &str); 1] =
    [("en-US", "shell-menu-settings-language-en-us")];

/// Builds the Settings menu. Item actions go to `return_focus`.
pub fn build(window: &mut Window, cx: &mut App, return_focus: &FocusHandle) -> Entity<TlMenuPopup> {
    TlMenu::build(window, cx, return_focus.clone(), |menu, window, cx| {
        let s = settings_now(cx);
        menu.header(tr(cx, "shell-menu-settings-monitoring"))
            .submenu(
                tr(cx, "shell-menu-settings-refresh"),
                window,
                cx,
                refresh_submenu,
            )
            .submenu(
                tr(cx, "shell-menu-settings-network-unit"),
                window,
                cx,
                network_unit_submenu,
            )
            .check_item(
                tr(cx, "shell-menu-settings-network-auto-scale"),
                s.units.network_auto_scale,
                toggle(SettingKey::UnitsNetworkAutoScale),
            )
            .header(tr(cx, "shell-menu-settings-motion"))
            .check_item(
                tr(cx, "shell-menu-settings-animations"),
                s.motion.animations,
                toggle(SettingKey::MotionAnimations),
            )
            .check_item(
                tr(cx, "shell-menu-settings-smooth-graphs"),
                s.motion.smooth_graphs,
                toggle(SettingKey::MotionSmoothGraphs),
            )
            .check_item(
                tr(cx, "shell-menu-settings-smooth-scroll"),
                s.motion.smooth_scroll,
                toggle(SettingKey::MotionSmoothScroll),
            )
            .check_item(
                tr(cx, "shell-menu-settings-close-with-esc"),
                s.window.close_with_esc,
                toggle(SettingKey::WindowCloseWithEsc),
            )
            .header(tr(cx, "shell-menu-settings-window-startup"))
            .submenu(
                tr(cx, "shell-menu-settings-default-tab"),
                window,
                cx,
                default_tab_submenu,
            )
            .header(tr(cx, "shell-menu-settings-language-group"))
            .submenu(
                tr(cx, "shell-menu-settings-language"),
                window,
                cx,
                language_submenu,
            )
            .separator()
            .item(tr(cx, "shell-menu-settings-reset"), Box::new(ResetSettings))
    })
}

fn settings_now(cx: &App) -> Settings {
    cx.try_global::<TlSettings>()
        .map_or_else(Settings::default, |_| TlSettings::settings(cx).clone())
}

fn toggle(key: SettingKey) -> Box<ToggleSetting> {
    Box::new(ToggleSetting(key))
}

fn set(key: SettingKey, value: SettingValue) -> Box<SetSetting> {
    Box::new(SetSetting { key, value })
}

fn set_str(key: SettingKey, value: &'static str) -> Box<SetSetting> {
    set(key, SettingValue::Str(SharedString::new_static(value)))
}

/// "0.25s" to "10.0s". A stored value that is none of the six (hand-edited) marks nothing.
fn refresh_submenu(mut menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let marks = refresh_marks(cx);
    for ((key, ms), on) in REFRESH_KEYS
        .into_iter()
        .zip(PROCESS_INTERVAL_MENU_MS)
        .zip(marks)
    {
        menu = menu.radio_item(
            tr(cx, key),
            on,
            set(
                SettingKey::ProcessesIntervalMs,
                SettingValue::Int(ms.into()),
            ),
        );
    }
    menu
}

fn network_unit_submenu(menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let current = settings_now(cx).units.network;
    menu.radio_item(
        tr(cx, "shell-menu-settings-bits"),
        current == NetworkUnit::Bits,
        set_str(SettingKey::UnitsNetwork, "bits"),
    )
    .radio_item(
        tr(cx, "shell-menu-settings-bytes"),
        current == NetworkUnit::Bytes,
        set_str(SettingKey::UnitsNetwork, "bytes"),
    )
}

/// One item per built tab in tab order, with the tab strip's labels, then "Last used".
fn default_tab_submenu(mut menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let current = settings_now(cx).general.start_tab;
    let built: Vec<Tab> = cx
        .try_global::<ShellState>()
        .map(|s| s.built_tabs.to_vec())
        .unwrap_or_default();
    for tab in Tab::ALL.into_iter().filter(|t| built.contains(t)) {
        let (key, value, start) = match tab {
            Tab::Processes => ("shell-tab-processes", "processes", StartTab::Processes),
            Tab::Performance => (
                "shell-tab-performance",
                "performance",
                StartTab::Performance,
            ),
            Tab::Services => ("shell-tab-services", "services", StartTab::Services),
            Tab::Startup => ("shell-tab-startup", "startup", StartTab::Startup),
            Tab::Connections => (
                "shell-tab-connections",
                "connections",
                StartTab::Connections,
            ),
            Tab::Information => (
                "shell-tab-information",
                "information",
                StartTab::Information,
            ),
        };
        menu = menu.radio_item(
            tr(cx, key),
            current == start,
            set_str(SettingKey::GeneralStartTab, value),
        );
    }
    menu.radio_item(
        tr(cx, "shell-menu-settings-last-used"),
        current == StartTab::LastUsed,
        set_str(SettingKey::GeneralStartTab, "last_used"),
    )
}

/// "System default", then each shipped locale by its own name. The live switch is i18n's.
fn language_submenu(mut menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let marks = language_marks(cx);
    menu = menu.radio_item(
        tr(cx, "shell-menu-settings-system-default"),
        marks[0],
        set_str(SettingKey::GeneralLanguage, "system"),
    );
    for ((value, key), on) in SHIPPED_LANGUAGES.into_iter().zip(&marks[1..]) {
        menu = menu.radio_item(
            tr(cx, key),
            *on,
            set_str(SettingKey::GeneralLanguage, value),
        );
    }
    menu
}

/// The radio marks of the Refresh items, in menu order.
pub fn refresh_marks(cx: &App) -> [bool; 6] {
    let current = settings_now(cx).processes.interval_ms;
    PROCESS_INTERVAL_MENU_MS.map(|ms| ms == current)
}

/// The radio marks of the Language items, in menu order.
pub fn language_marks(cx: &App) -> Vec<bool> {
    let current = settings_now(cx).general.language;
    std::iter::once("system")
        .chain(SHIPPED_LANGUAGES.map(|(value, _)| value))
        .map(|value| value == current)
        .collect()
}
