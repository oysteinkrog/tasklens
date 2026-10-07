//! Settings window, Processes page, P2 rows (E21-022, UI-SHL-065, R-010, R-075, R-099,
//! PROC-099): refresh, view, table display and Properties window options.
//!
//! Group order when every phase is built: Refresh, View, Tables (every table), Properties
//! windows, Dumps (E21-036), History (E30).

use gpui_kit::{App, SharedString};
use tl_model::settings::PROCESS_INTERVAL_MENU_MS;

use super::{PageBuilder, PageId};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue};
use crate::window::menus_settings::REFRESH_KEYS;

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Processes);
    let refresh = tr(cx, "shell-settings-group-refresh");
    let view = tr(cx, "shell-settings-group-view");
    let tables = tr(cx, "shell-settings-group-tables");
    let properties = tr(cx, "shell-settings-group-properties");

    let intervals: Vec<(SharedString, SettingValue)> = REFRESH_KEYS
        .into_iter()
        .zip(PROCESS_INTERVAL_MENU_MS)
        .map(|(key, ms)| (tr(cx, key), SettingValue::Int(i64::from(ms))))
        .collect();
    page.dropdown(
        refresh,
        SettingKey::ProcessesIntervalMs,
        tr(cx, "shell-menu-settings-refresh"),
        &intervals,
    );

    // The page writes `processes.mode` directly: the mode toast belongs to the menu and
    // toolbar Actions.
    let modes = options(
        cx,
        &[
            ("shell-menu-pv-tree", "tree"),
            ("shell-menu-pv-grouped", "grouped"),
            ("shell-menu-pv-list", "flat"),
        ],
    );
    page.dropdown(
        view.clone(),
        SettingKey::ProcessesMode,
        tr(cx, "shell-menu-view-process-view"),
        &modes,
    );
    for (key, label) in [
        (SettingKey::ProcessesTreeGuideLines, "shell-menu-pv-guides"),
        (
            SettingKey::ProcessesHighlightNewExited,
            "shell-menu-pv-highlight",
        ),
        (
            SettingKey::ProcessesFilterIncludeChildren,
            "shell-settings-include-children",
        ),
        (
            SettingKey::ProcessesHideSystem,
            "shell-settings-hide-system",
        ),
        (
            SettingKey::ProcessesSubtreeTotals,
            "shell-settings-subtree-totals",
        ),
    ] {
        page.switch(view.clone(), key, tr(cx, label), None);
    }

    let resize = options(
        cx,
        &[
            ("shell-menu-columns-proportional", "proportional"),
            ("shell-menu-columns-last-column", "last_column"),
        ],
    );
    page.dropdown(
        tables.clone(),
        SettingKey::TablesColumnResize,
        tr(cx, "shell-menu-columns-resize-behavior"),
        &resize,
    )
    .switch(
        tables,
        SettingKey::TablesDimZeroValues,
        tr(cx, "shell-settings-dim-zero-values"),
        None,
    );

    for (key, label) in [
        (
            SettingKey::PropertiesHideUnnamedHandles,
            "shell-settings-hide-unnamed-handles",
        ),
        (
            SettingKey::PropertiesSharedObjectsOnly,
            "shell-settings-shared-objects-only",
        ),
        (SettingKey::PropertiesEnvMask, "shell-settings-env-mask"),
    ] {
        page.switch(properties.clone(), key, tr(cx, label), None);
    }
    page
}

/// `(label key, stored value)` pairs as dropdown options.
fn options(cx: &App, pairs: &[(&str, &'static str)]) -> Vec<(SharedString, SettingValue)> {
    pairs
        .iter()
        .map(|(key, value)| (tr(cx, key), SettingValue::Str((*value).into())))
        .collect()
}
