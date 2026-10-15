//! Settings window, Privileges page (E21-035, UI-SHL-070, APP-023, APP-049, R-028). The switch
//! only writes `privilege.unlock_at_launch`; the button opens the Additional Setup window. No
//! privileged call happens here. The Skip password prompts row (E21-042, tasklens-bj6.46) goes
//! before the button and the helper status line (E29, tasklens-qfx.29) after it, both by chain
//! edit.

use gpui_kit::App;

use super::{PageBuilder, PageId};
use crate::actions::OpenAdditionalSetup;
use crate::components::button::TlButtonKind;
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
    // E29 (tasklens-qfx.29) adds the helper status line here.
    page
}
