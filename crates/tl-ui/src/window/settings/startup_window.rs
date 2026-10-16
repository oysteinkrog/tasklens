//! Settings window, Startup and window page, P2 rows (E21-026, UI-SHL-069, UI-SHL-040, APP-023,
//! APP-048, APP-009, R-058). Always On Top, Start at login, Start minimized and Auto-fetch Public
//! IP dispatch the same actions as their menu items, so the KWin keep-above, the autostart entry
//! and the public IP notice follow the switch. The Tray group (E31-010, APP-047: close to tray,
//! minimize to tray, always show tray icon) sits between Window and Login. The Memory group (E32-013,
//! PROC-097, R-152: rescue window switch) is the last group, after Network.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::{App, SharedString};
use tracing::warn;

use super::{LocalSwitch, PageBuilder, PageId};
use crate::app::AppEnv;
use crate::components::window_frame::ToggleAlwaysOnTop;
use crate::i18n::tr;
use crate::settings_global::{SettingKey, TlSettings};
use crate::window::main::WINDOW_TITLE;
use crate::window::menus_settings::{
    TogglePublicIpAutoFetch, ToggleStartAtLogin, ToggleStartMinimized,
};
use crate::window::registry::WindowKey;
use crate::window::titlebar::keep_above_supported;

/// The key combination "Open with Ctrl+Esc" reads and writes.
const CTRL_ESC: &str = "Ctrl+Esc";

/// "Start minimized" sits under "Start at login", indented by 16 before the UI scale.
const START_MINIMIZED_INDENT: f32 = 16.;

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::StartupWindow);
    let window = tr(cx, "shell-settings-group-window");
    let tray = tr(cx, "shell-settings-group-tray");
    let login = tr(cx, "shell-settings-group-login");
    let network = tr(cx, "shell-settings-group-network");
    let memory = tr(cx, "shell-settings-group-memory");
    // R-152: the tooltip adds a note when the unit had to run with OOMScoreAdjust 0.
    let lowmem_tooltip = if TlSettings::state(cx).lowmem.oom_score_adjust == Some(0) {
        "shell-settings-lowmem-tooltip-normal-oom"
    } else {
        "shell-settings-lowmem-tooltip"
    };
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
    .switch(
        tray.clone(),
        SettingKey::TrayCloseToTray,
        tr(cx, "shell-menu-settings-close-to-tray"),
        None,
    )
    .switch(
        tray.clone(),
        SettingKey::TrayMinimizeToTray,
        tr(cx, "shell-menu-settings-minimize-to-tray"),
        None,
    )
    .switch(
        tray,
        SettingKey::TrayAlwaysShow,
        tr(cx, "shell-menu-settings-always-show-tray"),
        None,
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
        login.clone(),
        SettingKey::StartupStartMinimized,
        tr(cx, "shell-menu-settings-start-minimized"),
        || Box::new(ToggleStartMinimized),
    )
    .local_switch(
        login,
        tr(cx, "shell-settings-ctrl-esc"),
        ctrl_esc_switch(cx),
    )
    // Turning it on asks through the public IP notice first (R-058).
    .action_switch(
        network,
        SettingKey::NetworkAutoFetchPublicIp,
        tr(cx, "shell-menu-settings-public-ip"),
        || Box::new(TogglePublicIpAutoFetch),
    )
    .tooltip_next(Some(tr(cx, lowmem_tooltip)))
    .switch(
        memory,
        SettingKey::LowmemEnabled,
        tr(cx, "shell-menu-settings-rescue-window"),
        None,
    );
    page
}

/// The "Open with Ctrl+Esc" switch (UI-SHL-069, APP-051). It has no settings key: the value is
/// the KDE global shortcut, read once when the page is built. With no `global_shortcut` it is
/// off and disabled.
fn ctrl_esc_switch(cx: &App) -> LocalSwitch {
    let on = Rc::new(Cell::new(false));
    let note: Rc<RefCell<Option<SharedString>>> = Rc::new(RefCell::new(None));
    let Some(api) = cx
        .try_global::<AppEnv>()
        .and_then(|e| e.global_shortcut.as_ref())
    else {
        let unavailable = tr(cx, "shell-settings-not-available");
        return LocalSwitch {
            get: Rc::new(|| false),
            description: Rc::new(move || Some(unavailable.clone())),
            set: Rc::new(|_, _| {}),
            disabled: true,
        };
    };
    let (get, set) = (api.get.clone(), api.set.clone());
    on.set(get().as_deref() == Some(CTRL_ESC));
    let relogin = tr(cx, "shell-settings-ctrl-esc-relogin");
    let (shown, shown_note) = (on.clone(), note.clone());
    LocalSwitch {
        get: Rc::new(move || shown.get()),
        description: Rc::new(move || shown_note.borrow().clone()),
        set: Rc::new(move |want, _| {
            match set(want.then_some(CTRL_ESC)) {
                Ok(()) => {
                    on.set(want);
                    // U-16: when the shortcut does not show the requested state, KDE applies
                    // it at the next login. The switch keeps the requested state.
                    let applied = get().as_deref() == Some(CTRL_ESC);
                    *note.borrow_mut() = (applied != want).then(|| relogin.clone());
                }
                // The switch keeps its old state. No toast.
                Err(error) => {
                    warn!(target: "tasklens::ui", %error, "Ctrl+Esc shortcut write failed");
                }
            }
        }),
        disabled: false,
    }
}
