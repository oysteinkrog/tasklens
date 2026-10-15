//! The Settings menu of the menu bar (E21-004, UI-SHL-040, UI-SHL-031, UI-SHL-131, APP-022,
//! APP-043, R-015, R-017, R-024, R-081, R-132).
//!
//! P1 rows under group headers: Monitoring (Refresh, Network transfer unit, Auto-scale),
//! Motion & interaction (Animations, Smooth Graphs, Smooth Scroll, Close with Esc), Window &
//! startup (Default tab) and Language, then "Reset Settings to Defaults". Later beads add the
//! P2 to P4 rows (E21-010, E21-011, E21-033, E21-040, E21-042). A header shows only when a row
//! under it exists.
//!
//! P2 rows (E21-010): the "Tab Switch" slider in Motion & interaction, "Auto-fetch Public IP on
//! launch" in Window & startup, and "All settings…" above Reset. Turning the auto-fetch on goes
//! through the public IP notice ([`request_public_ip_fetch`]), so the key is set only once the
//! user has accepted it; turning it off needs no notice.
//!
//! P2 rows (E21-011): "Start at login" and the indented "Start minimized" in Window & startup,
//! before "Default tab". tl-ui writes no file (ADR-011): both go through
//! `AppEnv.set_autostart(enabled, minimized)` (R-171, R-236), and a key changes only when that
//! call succeeds. On failure the Error toast "Could not change the autostart entry" shows.
//!
//! P3 rows (E21-033): "Record history", "History storage" and "Save history to disk" in
//! Monitoring; the three tray checks, "Unlock privileged actions at launch" and "Open rescue
//! window on critical memory pressure" in Window & startup. These rows only write their keys
//! (plus the history toast). The features that react to the keys belong to E29 to E32.

use fluent_bundle::FluentValue;
use gpui_kit::{App, Entity, FocusHandle, SharedString, Window, px};
use tl_model::command::Tab;
use tl_model::settings::{
    HISTORY_RETENTION_MENU_H, NetworkUnit, PROCESS_INTERVAL_MENU_MS, Settings, StartTab,
};
use tracing::warn;

use super::confirm::ResetSettings;
use super::main::ShellState;
use super::menubar::{MenuSliders, TAB_SWITCH};
use super::public_ip::request_public_ip_fetch;
use crate::actions::{OpenSettings, SetSetting, ToggleSetting};
use crate::app::AppEnv;
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{tr, tr_args};
use crate::settings_global::{SettingKey, SettingValue, TlSettings};

/// The Refresh items: label key and `processes.interval_ms`, in `PROCESS_INTERVAL_MENU_MS` order.
pub(crate) const REFRESH_KEYS: [&str; 6] = [
    "shell-menu-settings-refresh-250",
    "shell-menu-settings-refresh-500",
    "shell-menu-settings-refresh-1000",
    "shell-menu-settings-refresh-2000",
    "shell-menu-settings-refresh-5000",
    "shell-menu-settings-refresh-10000",
];

/// The History storage items: label key per `HISTORY_RETENTION_MENU_H` entry.
pub(crate) const HISTORY_STORAGE_KEYS: [&str; 7] = [
    "shell-menu-settings-history-1h",
    "shell-menu-settings-history-2h",
    "shell-menu-settings-history-3h",
    "shell-menu-settings-history-4h",
    "shell-menu-settings-history-6h",
    "shell-menu-settings-history-8h",
    "shell-menu-settings-history-12h",
];

/// The locales shipped in this build after "System default": `general.language` value and the
/// label key with the locale's own name. English only this round (R-132).
pub(crate) const SHIPPED_LANGUAGES: [(&str, &str); 1] =
    [("en-US", "shell-menu-settings-language-en-us")];

/// "Start minimized" sits under "Start at login", indented by 16 before the UI scale.
const START_MINIMIZED_INDENT: f32 = 16.;

gpui_kit::actions!(
    tasklens,
    [
        TogglePublicIpAutoFetch,
        /// Writes or deletes the autostart entry, then flips `startup.start_at_login`.
        ToggleStartAtLogin,
        /// Flips `startup.start_minimized`; rewrites the entry while Start at login is on.
        ToggleStartMinimized,
        /// Flips `history.enabled` and shows the Info toast "History recording on" or "off".
        ToggleRecordHistory,
    ]
);

/// Registers the "Auto-fetch Public IP on launch" and autostart handlers. `menubar::init` calls
/// it.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &ToggleStartAtLogin, cx| {
        let startup = settings_now(cx).startup;
        let on = !startup.start_at_login;
        if write_autostart(cx, on, startup.start_minimized) {
            TlSettings::set(cx, SettingKey::StartupStartAtLogin, SettingValue::Bool(on));
        }
    });
    cx.on_action(|_: &ToggleStartMinimized, cx| {
        let startup = settings_now(cx).startup;
        let minimized = !startup.start_minimized;
        if !startup.start_at_login || write_autostart(cx, true, minimized) {
            TlSettings::set(
                cx,
                SettingKey::StartupStartMinimized,
                SettingValue::Bool(minimized),
            );
        }
    });
    cx.on_action(|_: &ToggleRecordHistory, cx| {
        let on = !settings_now(cx).history.enabled;
        TlSettings::set(cx, SettingKey::HistoryEnabled, SettingValue::Bool(on));
        let key = if on {
            "shell-toast-history-on"
        } else {
            "shell-toast-history-off"
        };
        info_toast(cx, key);
    });
    cx.on_action(|_: &TogglePublicIpAutoFetch, cx| {
        if settings_now(cx).network.auto_fetch_public_ip {
            TlSettings::update(cx, |d| d.settings.network.auto_fetch_public_ip = false);
            return;
        }
        let Some(handle) = cx.active_window() else {
            return;
        };
        // The window is busy dispatching this action; the notice needs it.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                request_public_ip_fetch(window, cx, |_, cx| {
                    TlSettings::update(cx, |d| d.settings.network.auto_fetch_public_ip = true);
                });
            });
        });
    });
}

/// Builds the Settings menu. Item actions go to `return_focus`; the Tab Switch row uses the
/// bar's `sliders`.
pub fn build(
    window: &mut Window,
    cx: &mut App,
    return_focus: &FocusHandle,
    sliders: &MenuSliders,
) -> Entity<TlMenuPopup> {
    let tab_switch = sliders.tab_switch.clone();
    TlMenu::build(window, cx, return_focus.clone(), move |menu, window, cx| {
        let s = settings_now(cx);
        let menu = menu
            .header(tr(cx, "shell-menu-settings-monitoring"))
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
            .check_item(
                tr(cx, "shell-menu-settings-record-history"),
                s.history.enabled,
                Box::new(ToggleRecordHistory),
            )
            .submenu(
                tr(cx, "shell-menu-settings-history-storage"),
                window,
                cx,
                history_storage_submenu,
            );
        let persist = tr(cx, "shell-menu-settings-history-persist");
        let menu = if s.history.enabled {
            menu.check_item(
                persist,
                s.history.persist,
                toggle(SettingKey::HistoryPersist),
            )
        } else {
            menu.item_disabled(persist, Some(tr(cx, "shell-menu-settings-history-first")))
        };
        let menu = menu
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
            .slider_row(
                tr(cx, "shell-menu-settings-tab-switch"),
                &tab_switch,
                TAB_SWITCH.range,
                ms_text(cx),
            )
            .check_item(
                tr(cx, "shell-menu-settings-close-with-esc"),
                s.window.close_with_esc,
                toggle(SettingKey::WindowCloseWithEsc),
            )
            .header(tr(cx, "shell-menu-settings-window-startup"))
            .check_item(
                tr(cx, "shell-menu-settings-close-to-tray"),
                s.tray.close_to_tray,
                toggle(SettingKey::TrayCloseToTray),
            )
            .check_item(
                tr(cx, "shell-menu-settings-minimize-to-tray"),
                s.tray.minimize_to_tray,
                toggle(SettingKey::TrayMinimizeToTray),
            )
            .check_item(
                tr(cx, "shell-menu-settings-always-show-tray"),
                s.tray.always_show,
                toggle(SettingKey::TrayAlwaysShow),
            )
            .check_item(
                tr(cx, "shell-menu-settings-start-at-login"),
                s.startup.start_at_login,
                Box::new(ToggleStartAtLogin),
            );
        let minimized = tr(cx, "shell-menu-settings-start-minimized");
        let menu = if s.startup.start_at_login {
            menu.check_item(
                minimized,
                s.startup.start_minimized,
                Box::new(ToggleStartMinimized),
            )
        } else {
            menu.item_disabled(minimized, None)
        }
        .indent(px(START_MINIMIZED_INDENT));
        menu.submenu(
            tr(cx, "shell-menu-settings-default-tab"),
            window,
            cx,
            default_tab_submenu,
        )
        .check_item(
            tr(cx, "shell-menu-settings-unlock-at-launch"),
            s.privilege.unlock_at_launch,
            toggle(SettingKey::PrivilegeUnlockAtLaunch),
        )
        .check_item(
            tr(cx, "shell-menu-settings-public-ip"),
            s.network.auto_fetch_public_ip,
            Box::new(TogglePublicIpAutoFetch),
        )
        .check_item(
            tr(cx, "shell-menu-settings-rescue-window"),
            s.lowmem.enabled,
            toggle(SettingKey::LowmemEnabled),
        )
        .header(tr(cx, "shell-menu-settings-language-group"))
        .submenu(
            tr(cx, "shell-menu-settings-language"),
            window,
            cx,
            language_submenu,
        )
        .separator()
        .item(tr(cx, "shell-menu-settings-all"), Box::new(OpenSettings))
        .item(tr(cx, "shell-menu-settings-reset"), Box::new(ResetSettings))
    })
}

/// Pushes the Info toast `key` in the active window. Deferred: the window is busy dispatching
/// the menu action.
fn info_toast(cx: &mut App, key: &str) {
    let text = tr(cx, key);
    let Some(handle) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| q.push(ToastKind::Info, text, None, None, cx));
            }
        });
    });
}

/// Calls `AppEnv.set_autostart(enabled, minimized)`. On failure (or with no `AppEnv`) pushes
/// the Error toast and returns false.
fn write_autostart(cx: &mut App, enabled: bool, minimized: bool) -> bool {
    let result = match cx.try_global::<AppEnv>() {
        Some(env) => (env.set_autostart.clone())(enabled, minimized),
        None => Err("no AppEnv".to_owned()),
    };
    let Err(err) = result else {
        return true;
    };
    warn!(target: "tasklens::ui", %err, enabled, minimized, "autostart entry not changed");
    let text = tr(cx, "shell-toast-autostart-failed");
    if let Some(handle) = cx.active_window() {
        // The window is busy dispatching the menu action.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    queue.update(cx, |q, cx| q.push(ToastKind::Error, text, None, None, cx));
                }
            });
        });
    }
    false
}

fn settings_now(cx: &App) -> Settings {
    cx.try_global::<TlSettings>()
        .map_or_else(Settings::default, |_| TlSettings::settings(cx).clone())
}

/// The Tab Switch value text, "250 ms". Translated once per menu open; the slider formats each
/// value into the translated text without `cx`.
fn ms_text(cx: &App) -> impl Fn(f32) -> SharedString + 'static {
    const MARK: &str = "\u{1}";
    let template = tr_args(
        cx,
        "shell-menu-settings-tab-switch-ms",
        &[("ms", FluentValue::from(MARK))],
    );
    move |v| template.replace(MARK, &format!("{v:.0}")).into()
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

/// "1 hour" to "12 hours". A stored value that is none of the seven marks nothing.
fn history_storage_submenu(mut menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let current = settings_now(cx).history.retention_h;
    for (key, hours) in HISTORY_STORAGE_KEYS
        .into_iter()
        .zip(HISTORY_RETENTION_MENU_H)
    {
        menu = menu.radio_item(
            tr(cx, key),
            hours == current,
            set(
                SettingKey::HistoryRetentionH,
                SettingValue::Int(hours.into()),
            ),
        );
    }
    menu
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
