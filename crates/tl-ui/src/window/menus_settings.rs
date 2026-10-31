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
//!
//! P4 row (E21-040, UI-SHL-040a, R-237): the "Rendering GPU" submenu, last in Monitoring. Each
//! open shows "Auto", a disabled "Detecting adapters…" and "Software renderer" at once, then a
//! foreground task waits up to 2 s for the adapter probe (`AppEnv.list_adapters`, run by tl-app on
//! its own thread) and rebuilds the submenu with one row per adapter, or "No adapters found".
//! A row writes `rendering.gpu`; a new value shows the Info toast "Restart tasklens to use the new
//! GPU".
//!
//! P4 row (E21-042, UI-SHL-040, UI-SHL-091, R-222): "Skip password prompts (experimental)" in
//! Window & startup, after "Unlock privileged actions at launch". Turning it on asks first
//! ([`confirm_passwordless`]), then sends `RunSetup { PolkitRule { on: true } }`; turning it off
//! sends `PolkitRule { on: false }` at once. The UI never writes the polkit rule itself.
//! [`PasswordlessFlow`] writes `privilege.passwordless` only when `health.setup.run` reports the
//! step succeeded. A cancelled prompt shows the Info toast "Authentication cancelled", another
//! failure the Error toast "Setup failed", and the key stays as it was.

use std::future::Future;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, FutureExt as _, Global,
    SharedString, Subscription, Window, px,
};
use tl_model::command::{Command, SetupPhase, SetupStep, Tab};
use tl_model::settings::{
    HISTORY_RETENTION_MENU_H, NetworkUnit, PROCESS_INTERVAL_MENU_MS, Settings, StartTab,
};
use tracing::warn;

use super::confirm::{ResetSettings, confirm_passwordless};
use super::main::ShellState;
use super::menubar::{MenuSliders, TAB_SWITCH};
use super::public_ip::request_public_ip_fetch;
use crate::action_tracker::host_send_allowed;
use crate::actions::{OpenSettings, SetSetting, ToggleSetting};
use crate::app::{AdapterInfo, AppEnv};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::host::HostContext;
use crate::host_settings;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{tr, tr_args};
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::window::main::push_main_toast;

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

/// How long the Rendering GPU rows wait for the adapter probe (UI-SHL-040a).
pub const ADAPTER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

gpui_kit::actions!(
    tasklens,
    [
        TogglePublicIpAutoFetch,
        /// Writes or deletes the autostart entry, then flips `startup.start_at_login`.
        ToggleStartAtLogin,
        /// Flips `startup.start_minimized`; rewrites the entry while Start at login is on.
        ToggleStartMinimized,
        /// Flips `history.enabled` and, once the engine applied it, shows the Info toast "History
        /// recording on" or "off".
        ToggleRecordHistory,
        /// Asks, then installs the polkit rule; or removes it. `privilege.passwordless` follows
        /// the setup result.
        TogglePasswordless,
    ]
);

/// Writes `rendering.gpu` and, when the value changes, shows the Info toast "Restart tasklens to
/// use the new GPU".
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetRenderingGpu(pub SharedString);

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
        // App-level: acts on the host the main window shows now.
        let host = HostContext::shown(cx);
        // A host setting: nothing to flip before the engine gave it.
        let current = host_settings::current(&host, cx);
        if current.rev == 0 {
            return;
        }
        let on = !current.values.history.enabled;
        let key = if on {
            "shell-toast-history-on"
        } else {
            "shell-toast-history-off"
        };
        // The toast waits for the engine's Ack: a full queue, a conflict or a lost connection
        // leave recording as it was, so they show no toast.
        let window = cx.active_window();
        host_settings::set_then(
            &host,
            cx,
            SettingKey::HistoryEnabled,
            &SettingValue::Bool(on),
            move |cx| {
                if let Some(window) = window {
                    info_toast_in(window, cx, key);
                }
            },
        );
    });
    cx.on_action(|a: &SetRenderingGpu, cx| {
        if settings_now(cx).rendering.gpu == a.0.as_ref() {
            return;
        }
        TlSettings::set(cx, SettingKey::RenderingGpu, SettingValue::Str(a.0.clone()));
        info_toast(cx, "shell-toast-gpu-restart");
    });
    cx.on_action(|_: &TogglePasswordless, cx| {
        // App-level: the setup step runs on the host the main window shows now.
        let host = HostContext::shown(cx);
        let Some(flow) = PasswordlessFlow::get(&host, cx) else {
            warn!(target: "tasklens::ui", "no hub, skip password prompts not changed");
            return;
        };
        if flow.read(cx).pending.is_some() {
            return;
        }
        let active = cx.active_window();
        if settings_now(cx).privilege.passwordless {
            flow.update(cx, |flow, cx| flow.send(false, active, cx));
            return;
        }
        let Some(handle) = active else {
            return;
        };
        // The window is busy dispatching this action; the confirmation needs it.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                confirm_passwordless(window, cx, move |_, cx| {
                    flow.update(cx, |flow, cx| flow.send(true, Some(handle), cx));
                });
            });
        });
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

/// Builds the Settings menu for `host`. Item actions go to `return_focus`; the Tab Switch row
/// uses the bar's `sliders`.
pub fn build(
    host: &HostContext,
    window: &mut Window,
    cx: &mut App,
    return_focus: &FocusHandle,
    sliders: &MenuSliders,
) -> Entity<TlMenuPopup> {
    let tab_switch = sliders.tab_switch.clone();
    let host = host.clone();
    TlMenu::build(window, cx, return_focus.clone(), move |menu, window, cx| {
        let s = settings_now(cx);
        // The refresh interval and History are host settings, disabled until the engine gave
        // them (ADR-018 Settings).
        let current = host_settings::current(&host, cx);
        let waiting = (current.rev == 0).then(|| tr(cx, "shell-settings-host-waiting"));
        let history = &current.values.history;
        let refresh = tr(cx, "shell-menu-settings-refresh");
        let menu = menu.header(tr(cx, "shell-menu-settings-monitoring"));
        let menu = if waiting.is_some() {
            menu.item_disabled(refresh, waiting.clone())
        } else {
            let host = host.clone();
            menu.submenu(refresh, window, cx, move |menu, window, cx| {
                refresh_submenu(&host, menu, window, cx)
            })
        };
        let menu = menu
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
            );
        let record = tr(cx, "shell-menu-settings-record-history");
        let storage = tr(cx, "shell-menu-settings-history-storage");
        let persist = tr(cx, "shell-menu-settings-history-persist");
        let menu = if waiting.is_some() {
            menu.item_disabled(record, waiting.clone())
                .item_disabled(storage, waiting.clone())
                .item_disabled(persist, waiting.clone())
        } else {
            let menu = menu
                .check_item(record, history.enabled, Box::new(ToggleRecordHistory))
                .submenu(storage, window, cx, {
                    let host = host.clone();
                    move |menu, window, cx| history_storage_submenu(&host, menu, window, cx)
                });
            if history.enabled {
                menu.check_item(persist, history.persist, toggle(SettingKey::HistoryPersist))
            } else {
                menu.item_disabled(persist, Some(tr(cx, "shell-menu-settings-history-first")))
            }
        };
        let menu = menu
            .submenu(
                tr(cx, "shell-menu-settings-rendering-gpu"),
                window,
                cx,
                rendering_gpu_submenu,
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
            tr(cx, "shell-menu-settings-passwordless"),
            s.privilege.passwordless,
            Box::new(TogglePasswordless),
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
        .value(language_current(cx))
        .separator()
        .item(tr(cx, "shell-menu-settings-all"), Box::new(OpenSettings))
        .item(tr(cx, "shell-menu-settings-reset"), Box::new(ResetSettings))
    })
}

/// Pushes the Info toast `key` in the active window. Deferred: the window is busy dispatching
/// the menu action.
fn info_toast(cx: &mut App, key: &str) {
    if let Some(handle) = cx.active_window() {
        info_toast_in(handle, cx, key);
    }
}

/// Pushes the Info toast `key` in `handle`'s window. Deferred, as `info_toast`.
fn info_toast_in(handle: AnyWindowHandle, cx: &mut App, key: &str) {
    let text = tr(cx, key);
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
fn refresh_submenu(host: &HostContext, mut menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    let marks = refresh_marks(host, cx);
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
fn history_storage_submenu(
    host: &HostContext,
    mut menu: TlMenu,
    _: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let current = host_settings::current(host, cx).values.history.retention_h;
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

/// "Auto", one row per adapter and "Software renderer". Each open shows "Detecting adapters…" in
/// place of the adapters and starts the probe; its result rebuilds the submenu.
fn rendering_gpu_submenu(menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
    gpu_rows(menu, None, cx).on_open(|menu, window, cx| {
        menu.rebuild(|menu, _, cx| gpu_rows(menu, None, cx), window, cx);
        let probe = probe_adapters(cx);
        cx.spawn_in(window, async move |this, cx| {
            let adapters = probe.await;
            let _ = this.update_in(cx, |menu, window, cx| {
                menu.rebuild(
                    |menu, _, cx| gpu_rows(menu, Some(&adapters), cx),
                    window,
                    cx,
                );
            });
        })
    })
}

/// The Rendering GPU rows. `adapters` is `None` while the probe runs.
fn gpu_rows(menu: TlMenu, adapters: Option<&[AdapterInfo]>, cx: &App) -> TlMenu {
    let current = settings_now(cx).rendering.gpu;
    let row = |value: String| Box::new(SetRenderingGpu(value.into()));
    let mut menu = menu.radio_item(
        tr(cx, "shell-menu-settings-gpu-auto"),
        current == "auto",
        row("auto".to_owned()),
    );
    match adapters {
        None => menu = menu.item_disabled(tr(cx, "shell-menu-settings-gpu-detecting"), None),
        Some([]) => menu = menu.item_disabled(tr(cx, "shell-menu-settings-gpu-none"), None),
        Some(adapters) => {
            for adapter in adapters {
                let value = adapter_setting_value(adapter);
                menu = menu.radio_item(adapter.name.clone(), current == value, row(value));
            }
        }
    }
    menu.radio_item(
        tr(cx, "shell-menu-settings-gpu-software"),
        current == "software",
        row("software".to_owned()),
    )
}

/// The `rendering.gpu` value of an adapter: `pci:<bus id>` when the adapter reports one,
/// otherwise `name:<adapter name>` (R-237).
pub fn adapter_setting_value(adapter: &AdapterInfo) -> String {
    match &adapter.pci_bus_id {
        Some(bus) => format!("pci:{bus}"),
        None => format!("name:{}", adapter.name),
    }
}

/// Starts the adapter probe through `AppEnv.list_adapters` (R-171). The future gives the
/// adapters, or an empty list when the probe sends nothing within [`ADAPTER_PROBE_TIMEOUT`], ends
/// without a result or there is no `AppEnv`. tl-app probes on its own thread, so awaiting this on
/// the main thread never blocks it.
pub(crate) fn probe_adapters(cx: &App) -> impl Future<Output = Vec<AdapterInfo>> + 'static {
    let rx = cx.try_global::<AppEnv>().map(|env| (env.list_adapters)());
    let executor = cx.background_executor().clone();
    async move {
        let Some(rx) = rx else {
            return Vec::new();
        };
        match rx
            .recv()
            .with_timeout(ADAPTER_PROBE_TIMEOUT, &executor)
            .await
        {
            Ok(Ok(adapters)) => adapters,
            Ok(Err(_)) => {
                warn!(target: "tasklens::ui", "adapter probe ended without a result");
                Vec::new()
            }
            Err(_) => {
                warn!(target: "tasklens::ui", "adapter probe sent nothing within 2 s");
                Vec::new()
            }
        }
    }
}

/// The radio marks of the Refresh items, in menu order. The interval is a setting of `host`.
pub fn refresh_marks(host: &HostContext, cx: &App) -> [bool; 6] {
    let current = host_settings::current(host, cx).values.interval_ms;
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

/// The name of the language in use, shown after "Language:" in the Settings menu.
fn language_current(cx: &App) -> SharedString {
    let key = SHIPPED_LANGUAGES
        .into_iter()
        .zip(&language_marks(cx)[1..])
        .find_map(|((_, key), on)| on.then_some(key))
        .unwrap_or("shell-menu-settings-system-default");
    tr(cx, key)
}

/// Sends the `polkit-rule on|off` setup step for "Skip password prompts" and writes
/// `privilege.passwordless` from its result (E21-042, R-222). One step runs at a time.
pub struct PasswordlessFlow {
    /// The host the step runs on. Set when the flow is made; never changes.
    host: HostContext,
    hub: Entity<SnapshotHub>,
    /// The value asked for while its step runs.
    pending: Option<bool>,
    /// The window that asked, for the result toast.
    requester: Option<AnyWindowHandle>,
    /// The `setup.run` seq when the step was sent. Only a newer run answers it.
    seen_seq: u64,
    _health: Subscription,
}

struct GlobalPasswordlessFlow(Entity<PasswordlessFlow>);

impl Global for GlobalPasswordlessFlow {}

impl PasswordlessFlow {
    /// The app's flow for `host`, created on first use. A flow made for another host is replaced
    /// unless its step runs. `None` without the hub: no result could arrive.
    fn get(host: &HostContext, cx: &mut App) -> Option<Entity<Self>> {
        if let Some(flow) = cx
            .try_global::<GlobalPasswordlessFlow>()
            .map(|f| f.0.clone())
        {
            let current = flow.read(cx);
            if current.pending.is_some() || host.hub().is_none_or(|hub| *hub == current.hub) {
                return Some(flow);
            }
        }
        let hub = host.hub()?.clone();
        let host = host.clone();
        let flow = cx.new(|cx| {
            let _health = cx.subscribe(&hub, |this: &mut Self, _, ev: &SnapshotChanged, cx| {
                if ev.mask.contains(ComponentMask::HEALTH) {
                    this.take_health(cx);
                }
            });
            Self {
                host,
                hub,
                pending: None,
                requester: None,
                seen_seq: 0,
                _health,
            }
        });
        cx.set_global(GlobalPasswordlessFlow(flow.clone()));
        Some(flow)
    }

    fn send(&mut self, on: bool, requester: Option<AnyWindowHandle>, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let Some(engine) = self.host.client() else {
            warn!(target: "tasklens::ui", "no engine, polkit rule step not sent");
            return;
        };
        if !host_send_allowed(&self.host, cx) {
            return;
        }
        let step = SetupStep::PolkitRule { on };
        if let Err(err) = engine.send(Command::RunSetup { step }) {
            warn!(target: "tasklens::ui", ?err, "polkit rule step not sent");
            return;
        }
        self.seen_seq = current_setup_seq(&self.hub, cx);
        self.pending = Some(on);
        self.requester = requester;
    }

    /// Takes the end of the pending step from `health.setup.run`: success writes the key, a
    /// cancel or a failure shows a toast and leaves it.
    fn take_health(&mut self, cx: &mut Context<Self>) {
        let Some(on) = self.pending else {
            return;
        };
        let Some(run) = self.hub.read(cx).current().health.setup.run.clone() else {
            return;
        };
        if run.seq <= self.seen_seq || run.step != (SetupStep::PolkitRule { on }) {
            return;
        }
        let toast = match &run.phase {
            SetupPhase::Running => return,
            SetupPhase::Succeeded => None,
            SetupPhase::Cancelled => Some((ToastKind::Info, "fnd-shield-auth-cancelled")),
            SetupPhase::Failed(_) => {
                warn!(target: "tasklens::ui", on, "polkit rule step failed");
                Some((ToastKind::Error, "setup-failed"))
            }
        };
        self.seen_seq = run.seq;
        self.pending = None;
        let requester = self.requester.take();
        match toast {
            None => TlSettings::set(
                cx,
                SettingKey::PrivilegePasswordless,
                SettingValue::Bool(on),
            ),
            Some((kind, key)) => {
                let text = tr(cx, key);
                toast_in(requester, kind, text, cx);
            }
        }
    }
}

/// True while a polkit rule step is running. The Privileges switch is disabled meanwhile.
pub fn passwordless_running(cx: &App) -> bool {
    cx.try_global::<GlobalPasswordlessFlow>()
        .is_some_and(|flow| flow.0.read(cx).pending.is_some())
}

fn current_setup_seq(hub: &Entity<SnapshotHub>, cx: &App) -> u64 {
    hub.read(cx)
        .current()
        .health
        .setup
        .run
        .as_ref()
        .map_or(0, |run| run.seq)
}

/// Pushes the toast into `window`'s stack when it is open and has one, else into the main
/// window's.
fn toast_in(window: Option<AnyWindowHandle>, kind: ToastKind, text: SharedString, cx: &mut App) {
    let queue = window.and_then(|w| {
        w.update(cx, |_, window, cx| ToastQueue::of_window(window, cx))
            .ok()
            .flatten()
    });
    match queue {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => push_main_toast(cx, kind, text, None, None),
    }
}
