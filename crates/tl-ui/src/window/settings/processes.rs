//! Settings window, Processes page, P2 rows (E21-022, UI-SHL-065, R-010, R-075, R-099,
//! PROC-099): refresh, view, table display and Properties window options.
//!
//! P4 row (E21-041): "Show column separators" follows Resize Behavior in the Tables group.
//!
//! Group order when every phase is built: Refresh, View, Tables (every table), Properties
//! windows, Dumps (E21-036), History (E30-027).
//!
//! The refresh interval and the History group are host settings (ADR-018 Settings): they show
//! the host's values and stay disabled until the engine has given them.

use gpui_kit::{App, SharedString};
use tl_model::settings::{HISTORY_RETENTION_MENU_H, PROCESS_INTERVAL_MENU_MS};

use super::{PageBuilder, PageId};
use crate::components::banner::TlBannerKind;
use crate::host_settings;
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue};
use crate::window::menus_settings::{HISTORY_STORAGE_KEYS, REFRESH_KEYS};

pub fn page(cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(PageId::Processes);
    let refresh = tr(cx, "shell-settings-group-refresh");
    let view = tr(cx, "shell-settings-group-view");
    let tables = tr(cx, "shell-settings-group-tables");
    let properties = tr(cx, "shell-settings-group-properties");
    let host = host_settings::current(cx);
    let known = host.rev > 0;
    let waiting = (!known).then(|| tr(cx, "shell-settings-host-waiting"));

    let intervals: Vec<(SharedString, SettingValue)> = REFRESH_KEYS
        .into_iter()
        .zip(PROCESS_INTERVAL_MENU_MS)
        .map(|(key, ms)| (tr(cx, key), SettingValue::Int(i64::from(ms))))
        .collect();
    if host.read_only {
        page.banner(
            refresh.clone(),
            TlBannerKind::Warning,
            tr(cx, "shell-settings-host-read-only"),
        );
    }
    page.disable_next(!known).dropdown(
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
        tables.clone(),
        SettingKey::TablesColumnSeparators,
        tr(cx, "shell-settings-column-separators"),
        None,
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
    let recording = host.values.history.enabled;
    let storage: Vec<(SharedString, SettingValue)> = HISTORY_STORAGE_KEYS
        .into_iter()
        .zip(HISTORY_RETENTION_MENU_H)
        .map(|(key, h)| (tr(cx, key), SettingValue::Int(i64::from(h))))
        .collect();
    page.disable_next(!known)
        .tooltip_next(waiting.clone())
        .switch(
            history.clone(),
            SettingKey::HistoryEnabled,
            tr(cx, "shell-menu-settings-record-history"),
            None,
        )
        .disable_next(!known)
        .dropdown(
            history.clone(),
            SettingKey::HistoryRetentionH,
            tr(cx, "shell-menu-settings-history-storage"),
            &storage,
        )
        .disable_next(!known || !recording)
        .tooltip_next(
            waiting
                .clone()
                .or_else(|| (!recording).then(|| tr(cx, "shell-menu-settings-history-first"))),
        )
        .switch(
            history.clone(),
            SettingKey::HistoryPersist,
            tr(cx, "shell-menu-settings-history-persist"),
            None,
        )
        .disable_next(!known)
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
