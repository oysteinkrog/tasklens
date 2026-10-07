//! Settings window, Units page (E21-019, UI-SHL-064, R-015, R-081): network transfer unit and
//! auto-scale.

use gpui_kit::App;

use super::{PageBuilder, PageId};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue};

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Units);
    // One untitled group: the page title already says "Units".
    let group = "";
    page.dropdown(
        group,
        SettingKey::UnitsNetwork,
        tr(cx, "shell-menu-settings-network-unit"),
        &[
            (
                tr(cx, "shell-menu-settings-bits"),
                SettingValue::Str("bits".into()),
            ),
            (
                tr(cx, "shell-menu-settings-bytes"),
                SettingValue::Str("bytes".into()),
            ),
        ],
    )
    .switch(
        group,
        SettingKey::UnitsNetworkAutoScale,
        tr(cx, "shell-menu-settings-network-auto-scale"),
        None,
    );
    page
}
