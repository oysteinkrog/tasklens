//! Settings window, Startup and window page, P2 rows (E21-026, UI-SHL-069, UI-SHL-040, APP-023,
//! APP-048, APP-009, R-058). Always On Top, Start at login, Start minimized and Auto-fetch Public
//! IP dispatch the same actions as their menu items, so the KWin keep-above, the autostart entry
//! and the public IP notice follow the switch. The Tray group (E31) goes between Window and
//! Login, the Memory group (E32) after Network, both by chain edit.

use gpui_kit::App;

use super::{PageBuilder, PageId};
use crate::components::window_frame::ToggleAlwaysOnTop;
use crate::i18n::tr;
use crate::settings_global::{SettingKey, TlSettings};
use crate::window::main::WINDOW_TITLE;
use crate::window::menus_settings::{
    TogglePublicIpAutoFetch, ToggleStartAtLogin, ToggleStartMinimized,
};
use crate::window::registry::WindowKey;
use crate::window::titlebar::keep_above_supported;

/// "Start minimized" sits under "Start at login", indented by 16 before the UI scale.
const START_MINIMIZED_INDENT: f32 = 16.;

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::StartupWindow);
    let window = tr(cx, "shell-settings-group-window");
    let login = tr(cx, "shell-settings-group-login");
    let network = tr(cx, "shell-settings-group-network");
    let keep_above = keep_above_supported(cx);
    let start_at_login = TlSettings::settings(cx).startup.start_at_login;
    page.switch(
        window.clone(),
        SettingKey::WindowCloseWithEsc,
        tr(cx, "shell-menu-settings-close-with-esc"),
        None,
    )
    .disable_next(!keep_above)
    .tooltip_next((!keep_above).then(|| tr(cx, "fnd-pin-unsupported")))
    .action_switch(
        window,
        SettingKey::WindowAlwaysOnTop,
        tr(cx, "shell-menu-window-always-on-top"),
        || {
            Box::new(ToggleAlwaysOnTop {
                window: WindowKey::Main,
                caption: WINDOW_TITLE.into(),
            })
        },
    )
    .action_switch(
        login.clone(),
        SettingKey::StartupStartAtLogin,
        tr(cx, "shell-menu-settings-start-at-login"),
        || Box::new(ToggleStartAtLogin),
    )
    .indent_next(START_MINIMIZED_INDENT)
    .disable_next(!start_at_login)
    .action_switch(
        login,
        SettingKey::StartupStartMinimized,
        tr(cx, "shell-menu-settings-start-minimized"),
        || Box::new(ToggleStartMinimized),
    )
    // Turning it on asks through the public IP notice first (R-058).
    .action_switch(
        network,
        SettingKey::NetworkAutoFetchPublicIp,
        tr(cx, "shell-menu-settings-public-ip"),
        || Box::new(TogglePublicIpAutoFetch),
    );
    page
}
