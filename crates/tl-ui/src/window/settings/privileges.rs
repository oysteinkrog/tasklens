//! Settings window, Privileges page (E21-035, UI-SHL-070, APP-023, APP-049, R-028). The first
//! switch only writes `privilege.unlock_at_launch`; the button opens the Additional Setup window.
//! No privileged call happens here. The Skip password prompts switch (E21-042) dispatches
//! `TogglePasswordless`, which asks first and runs the polkit rule setup step; the key follows
//! the step's result, and the switch is disabled while the step runs. The helper status line (E29, tasklens-qfx.29) goes after the button.

use std::sync::Arc;

use gpui_kit::{App, IntoElement as _};
use tl_model::health::HelperState;

use super::{PageBuilder, PageId};
use crate::actions::OpenAdditionalSetup;
use crate::components::button::TlButtonKind;
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::settings_global::SettingKey;
use crate::window::menus_settings::{TogglePasswordless, passwordless_running};

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Privileges);
    let unlock = tr(cx, "shell-settings-group-unlock");
    let prompts = tr(cx, "shell-settings-group-password-prompts");
    let helper = tr(cx, "shell-settings-group-helper");
    page.switch(
        unlock,
        SettingKey::PrivilegeUnlockAtLaunch,
        tr(cx, "shell-menu-settings-unlock-at-launch"),
        None,
    );
    page.disable_next(passwordless_running(cx));
    page.action_switch(
        prompts,
        SettingKey::PrivilegePasswordless,
        tr(cx, "shell-menu-settings-passwordless"),
        || Box::new(TogglePasswordless),
    );
    page.button(
        helper.clone(),
        tr(cx, "shell-menu-help-setup"),
        Box::new(OpenAdditionalSetup { focus_row: None }),
        TlButtonKind::Secondary,
    );
    // Helper status line (E29, tasklens-qfx.29). The window re-renders on `HEALTH`.
    let installed = cx
        .try_global::<GlobalHub>()
        .map(|hub| Arc::clone(hub.0.read(cx).current()))
        .is_some_and(|snap| snap.health.helper != HelperState::Missing);
    let key = if installed {
        "settings-privileges-helper-installed"
    } else {
        "settings-privileges-helper-not-installed"
    };
    page.custom(helper, &[], tr(cx, key), |_, _| {
        gpui_kit::Empty.into_any_element()
    });
    page
}
