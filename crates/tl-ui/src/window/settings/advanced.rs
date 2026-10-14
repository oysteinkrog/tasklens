//! Settings window, Advanced page, P2 rows (E21-A2, UI-SHL-071, APP-023, APP-032, R-071).
//! The settings folder comes from `AppEnv`, and opening it goes through the action tracker.

use gpui_kit::App;
use tracing::warn;

use super::{PageBuilder, PageId};
use crate::action_tracker::ToastTexts;
use crate::app::AppEnv;
use crate::components::button::TlButtonKind;
use crate::components::toast::ToastQueue;
use crate::i18n::tr;
use crate::settings_global::SettingKey;
use crate::window::about::{file_uri, send_open_uri};
use crate::window::confirm::ResetSettings;

gpui_kit::actions!(tasklens, [OpenSettingsFolder]);

const OPEN_FAILED: &str = "shell-about-logs-failed";

pub(super) fn init(cx: &mut App) {
    cx.on_action(|_: &OpenSettingsFolder, cx| {
        let Some(window) = cx.active_window() else {
            warn!(target: "tasklens::ui", "no window; the settings folder was not opened");
            return;
        };
        let Some(dir) = cx.try_global::<AppEnv>().map(|env| env.config_dir.clone()) else {
            warn!(target: "tasklens::ui", "no AppEnv; the settings folder was not opened");
            return;
        };
        let failed = tr(cx, OPEN_FAILED);
        cx.defer(move |cx| {
            if window
                .update(cx, |_, window, cx| {
                    if ToastQueue::of_window(window, cx).is_none() {
                        warn!(target: "tasklens::ui", "no toast queue; the settings folder was not opened");
                        return;
                    }
                    send_open_uri(
                        file_uri(&dir),
                        ToastTexts::new("", failed),
                        window,
                        cx,
                    );
                })
                .is_err()
            {
                warn!(target: "tasklens::ui", "settings folder: the window is gone");
            }
        });
    });
}

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Advanced);
    let group = "";
    page.switch(
        group,
        SettingKey::NetworkPublicIpNoticeAccepted,
        tr(cx, "shell-settings-public-ip-notice"),
        None,
    )
    .switch(
        group,
        SettingKey::DebugShowFps,
        tr(cx, "shell-settings-show-fps"),
        None,
    )
    .button(
        group,
        tr(cx, "shell-settings-open-folder"),
        Box::new(OpenSettingsFolder),
        TlButtonKind::Secondary,
    )
    .button(
        group,
        tr(cx, "shell-menu-settings-reset"),
        Box::new(ResetSettings),
        TlButtonKind::Danger,
    );
    page
}
