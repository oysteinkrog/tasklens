//! Settings window, Processes page, P2 rows (E21-022, UI-SHL-065, R-010, R-075, R-099,
//! PROC-099): refresh, view, table display and Properties window options.
//!
//! Group order when every phase is built: Refresh, View, Tables (every table), Properties
//! windows, Dumps (E21-036), History (E30-027).

use gpui_kit::{App, SharedString};
use tl_model::settings::{HISTORY_RETENTION_MENU_H, PROCESS_INTERVAL_MENU_MS};

use super::{PageBuilder, PageId};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::window::menus_settings::{HISTORY_STORAGE_KEYS, REFRESH_KEYS};

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

    let dumps = tr(cx, "shell-settings-group-dumps");
    page.dropdown(
        dumps,
        SettingKey::ProcessesDumpType,
        tr(cx, "shell-settings-dump-type"),
        &options(
            cx,
            &[
                ("shell-settings-dump-mini", "mini"),
                ("shell-settings-dump-full", "full"),
            ],
        ),
    );

    let history = tr(cx, "shell-settings-group-history");
    let recording = TlSettings::settings(cx).history.enabled;
    let storage: Vec<(SharedString, SettingValue)> = HISTORY_STORAGE_KEYS
        .into_iter()
        .zip(HISTORY_RETENTION_MENU_H)
        .map(|(key, h)| (tr(cx, key), SettingValue::Int(i64::from(h))))
        .collect();
    page.switch(
        history.clone(),
        SettingKey::HistoryEnabled,
        tr(cx, "shell-menu-settings-record-history"),
        None,
    )
    .dropdown(
        history.clone(),
        SettingKey::HistoryRetentionH,
        tr(cx, "shell-menu-settings-history-storage"),
        &storage,
    )
    .disable_next(!recording)
    .tooltip_next((!recording).then(|| tr(cx, "shell-menu-settings-history-first")))
    .switch(
        history.clone(),
        SettingKey::HistoryPersist,
        tr(cx, "shell-menu-settings-history-persist"),
        None,
    )
    .number(
        history,
        SettingKey::HistoryMaxDiskMb,
        tr(cx, "shell-settings-history-disk-limit"),
        64..=4096,
        64,
        tr(cx, "shell-settings-unit-mib"),
    );
    page
}

/// `(label key, stored value)` pairs as dropdown options.
fn options(cx: &App, pairs: &[(&str, &'static str)]) -> Vec<(SharedString, SettingValue)> {
    pairs
        .iter()
        .map(|(key, value)| (tr(cx, key), SettingValue::Str((*value).into())))
        .collect()
}
