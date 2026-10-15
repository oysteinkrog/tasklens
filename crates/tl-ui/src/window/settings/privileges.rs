//! Settings window, Privileges page (E21-035, UI-SHL-070, APP-023, APP-049, R-028). The switch
//! only writes `privilege.unlock_at_launch`; the button opens the Additional Setup window. No
//! privileged call happens here. The Skip password prompts row (E21-042, tasklens-bj6.46) goes
//! before the button and the helper status line (E29, tasklens-qfx.29) after it, both by chain
//! edit.

use std::sync::Arc;

use gpui_kit::{App, IntoElement as _};
use tl_model::health::HelperState;

use super::{PageBuilder, PageId};
use crate::actions::OpenAdditionalSetup;
use crate::components::button::TlButtonKind;
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::settings_global::SettingKey;

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Privileges);
    let group = "";
    page.switch(
        group,
        SettingKey::PrivilegeUnlockAtLaunch,
        tr(cx, "shell-menu-settings-unlock-at-launch"),
        None,
    );
    // E21-042 (tasklens-bj6.46) adds the Skip password prompts row here.
    page.button(
        group,
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
    page.custom(group, &[], tr(cx, key), |_, _| {
        gpui_kit::Empty.into_any_element()
    });
    page
}
