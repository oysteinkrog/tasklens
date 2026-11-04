//! Settings window, Startup and window page, P2 rows (E21-026, UI-SHL-069, UI-SHL-040, APP-023,
//! APP-048, APP-009, R-058). Always On Top, Start at login, Start minimized and Auto-fetch Public
//! IP dispatch the same actions as their menu items, so the KWin keep-above, the autostart entry
//! and the public IP notice follow the switch. The Tray group (E31-010, APP-047: close to tray,
//! minimize to tray, always show tray icon) sits between Window and Login. The Memory group (E32-013,
//! PROC-097, R-152: rescue window switch) is the last group, after Network.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::{App, Global, SharedString};
use tracing::warn;

use super::{LocalSwitch, PageBuilder, PageId};
use crate::window::main::WINDOW_TITLE;
use crate::window::menus_settings::{
    TogglePublicIpAutoFetch, ToggleStartAtLogin, ToggleStartMinimized,
};
use crate::window::titlebar::keep_above_supported;
use tl_ui_base::app_env::{AppEnv, GlobalShortcutApi};
use tl_ui_base::host::HostContext;
use tl_ui_base::i18n::tr;
use tl_ui_base::registry::WindowKey;
use tl_ui_base::settings_global::{SettingKey, TlSettings};
use tl_ui_kit::components::window_frame::ToggleAlwaysOnTop;

/// The key combination "Open with Meta+Esc" reads and writes.
const META_ESC: &str = "Meta+Esc";

/// "Start minimized" sits under "Start at login", indented by 16 before the UI scale.
const START_MINIMIZED_INDENT: f32 = 16.;

pub fn page(host: &HostContext, cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(host, PageId::StartupWindow);
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
        tr(cx, "shell-settings-meta-esc"),
        meta_esc_switch(cx),
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

/// The "Open with Meta+Esc" state. The page is rebuilt on every render, so the state lives in a
/// global: kglobalaccel is read when the Settings window opens ([`read_meta_esc`]) and after each
/// write, never while building the page.
#[derive(Default)]
struct MetaEsc {
    on: Cell<bool>,
    /// A read or write is running. A click meanwhile does nothing.
    busy: Cell<bool>,
    /// The "log out and back in" note (U-16).
    note: RefCell<Option<SharedString>>,
}

#[derive(Default)]
struct MetaEscState(Rc<MetaEsc>);

impl Global for MetaEscState {}

/// Starts reading the KDE global shortcut (APP-051). tl-app reads it on its own thread; the reply
/// sets the switch. The Settings window calls this when it opens.
pub(super) fn read_meta_esc(cx: &mut App) {
    let Some(get) = shortcut_api(cx).map(|api| api.get.clone()) else {
        return;
    };
    let state = cx.default_global::<MetaEscState>().0.clone();
    if state.busy.replace(true) {
        return;
    }
    let reply = get();
    cx.spawn(async move |cx| {
        match reply.recv().await {
            Ok(Ok(keys)) => {
                state.on.set(keys.as_deref() == Some(META_ESC));
                *state.note.borrow_mut() = None;
            }
            Ok(Err(error)) => {
                warn!(target: "tasklens::ui", %error, "Meta+Esc shortcut read failed")
            }
            Err(_) => warn!(target: "tasklens::ui", "Meta+Esc shortcut read ended without a reply"),
        }
        state.busy.set(false);
        cx.update(|cx| cx.refresh_windows());
    })
    .detach();
}

fn shortcut_api(cx: &App) -> Option<&GlobalShortcutApi> {
    cx.try_global::<AppEnv>()
        .and_then(|e| e.global_shortcut.as_ref())
}

/// The "Open with Meta+Esc" switch (UI-SHL-069, APP-051). It has no settings key: the value is
/// the KDE global shortcut. With no `global_shortcut` it is off and disabled. A click shows the
/// new state at once and goes back when the write fails.
fn meta_esc_switch(cx: &mut App) -> LocalSwitch {
    let Some(set) = shortcut_api(cx).map(|api| api.set.clone()) else {
        let unavailable = tr(cx, "shell-settings-not-available");
        return LocalSwitch {
            get: Rc::new(|| false),
            description: Rc::new(move || Some(unavailable.clone())),
            set: Rc::new(|_, _| {}),
            disabled: true,
        };
    };
    let state = cx.default_global::<MetaEscState>().0.clone();
    let relogin = tr(cx, "shell-settings-meta-esc-relogin");
    let (shown, shown_note) = (state.clone(), state.clone());
    LocalSwitch {
        get: Rc::new(move || shown.on.get()),
        description: Rc::new(move || shown_note.note.borrow().clone()),
        set: Rc::new(move |want, cx| {
            if state.busy.replace(true) {
                return;
            }
            let was = state.on.replace(want);
            let reply = set(want.then_some(META_ESC));
            let (state, relogin) = (state.clone(), relogin.clone());
            cx.spawn(async move |cx| {
                match reply.recv().await {
                    // U-16: when the shortcut does not read back as requested, KDE applies it
                    // at the next login. The switch keeps the requested state.
                    Ok(Ok(keys)) => {
                        let applied = keys.as_deref() == Some(META_ESC);
                        *state.note.borrow_mut() = (applied != want).then_some(relogin);
                    }
                    // The switch goes back to its old state. No toast.
                    Ok(Err(error)) => {
                        state.on.set(was);
                        warn!(target: "tasklens::ui", %error, "Meta+Esc shortcut write failed");
                    }
                    Err(_) => {
                        state.on.set(was);
                        warn!(target: "tasklens::ui", "Meta+Esc shortcut write ended without a reply");
                    }
                }
                state.busy.set(false);
                cx.update(|cx| cx.refresh_windows());
            })
            .detach();
        }),
        disabled: false,
    }
}
