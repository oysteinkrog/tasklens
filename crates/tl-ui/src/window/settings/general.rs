//! Settings window, General page (E21-019, UI-SHL-061, R-024, R-132): Default tab and
//! Language.

use gpui_kit::{App, SharedString};
use tl_model::command::Tab;

use super::{PageBuilder, PageId};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue};
use crate::window::main::ShellState;
use crate::window::menus_settings::SHIPPED_LANGUAGES;

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::General);
    let startup = tr(cx, "shell-settings-group-startup");
    let language = tr(cx, "shell-settings-group-language");

    // The built tabs in tab order, from the same list as the tab strip, then "Last used".
    let built: Vec<Tab> = cx
        .try_global::<ShellState>()
        .map(|s| s.built_tabs.to_vec())
        .unwrap_or_default();
    let mut tabs: Vec<(SharedString, SettingValue)> = Tab::ALL
        .into_iter()
        .filter(|t| built.contains(t))
        .map(|tab| {
            let (key, value) = match tab {
                Tab::Processes => ("shell-tab-processes", "processes"),
                Tab::Performance => ("shell-tab-performance", "performance"),
                Tab::Services => ("shell-tab-services", "services"),
                Tab::Startup => ("shell-tab-startup", "startup"),
                Tab::Connections => ("shell-tab-connections", "connections"),
                Tab::Information => ("shell-tab-information", "information"),
            };
            (tr(cx, key), str_value(value))
        })
        .collect();
    tabs.push((
        tr(cx, "shell-menu-settings-last-used"),
        str_value("last_used"),
    ));
    page.dropdown(
        startup,
        SettingKey::GeneralStartTab,
        tr(cx, "shell-menu-settings-default-tab"),
        &tabs,
    )
    .keywords(&["start", "tab", "launch"]);

    let mut languages = vec![(
        tr(cx, "shell-menu-settings-system-default"),
        str_value("system"),
    )];
    languages.extend(
        SHIPPED_LANGUAGES
            .into_iter()
            .map(|(value, key)| (tr(cx, key), str_value(value))),
    );
    page.dropdown(
        language,
        SettingKey::GeneralLanguage,
        tr(cx, "shell-settings-language"),
        &languages,
    )
    .description(tr(cx, "shell-settings-language-desc"))
    .keywords(&["language", "locale"]);
    page
}

fn str_value(value: &'static str) -> SettingValue {
    SettingValue::Str(value.into())
}
