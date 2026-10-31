//! Settings window, Appearance page (E21-020, UI-SHL-062, UI-SHL-006, UI-SHL-080, APP-023,
//! APP-035, APP-036, APP-040, R-020, R-021, R-077, R-083): theme choice and sync, the size
//! sliders, the look switches and "Use system title bar".
//!
//! The Theme dropdown dispatches `ActivateTheme` like the View > Theme rows, so sync turns off
//! the same way. Its options are the menu's theme rows, read at render. A UI Scale release shows
//! the menu's keyed "UI scale changed to N%" toast. "Use system title bar" needs a restart: the
//! running window keeps its mode.

use gpui_kit::{App, SharedString};
use tracing::warn;

use super::{PageBuilder, PageId};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::host::HostContext;
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue};
use crate::theme::ActivateTheme;
use crate::window::main::UI_SCALE_TOAST;
use crate::window::menubar::scale_toast_text;
use crate::window::menus_view::theme_rows;

fn pct(v: i64) -> SharedString {
    format!("{v}%").into()
}

pub fn page(host: &HostContext, cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(host, PageId::Appearance);
    let theme = tr(cx, "shell-settings-group-theme");
    let size = tr(cx, "shell-settings-group-size");
    let look = tr(cx, "shell-settings-group-look");
    let window = tr(cx, "shell-settings-group-window");

    let rows = theme_rows(cx);
    let themes: Vec<(SharedString, SettingValue)> = rows
        .iter()
        .map(|r| (r.label.clone(), SettingValue::Str(r.name.clone())))
        .collect();
    let names: Vec<(SharedString, SettingValue)> = rows
        .iter()
        .map(|r| (r.label.clone(), SettingValue::Str(r.name.clone())))
        .collect();

    page.action_dropdown(
        theme.clone(),
        SettingKey::ThemeActive,
        tr(cx, "shell-menu-view-theme"),
        &themes,
        |name| {
            Box::new(ActivateTheme {
                name: name.to_string().into(),
            })
        },
    )
    .switch(
        theme.clone(),
        SettingKey::ThemeFollowSystem,
        tr(cx, "shell-menu-theme-sync"),
        None,
    )
    .dropdown(
        theme.clone(),
        SettingKey::ThemeLight,
        tr(cx, "shell-menu-theme-light"),
        &names,
    )
    .dropdown(
        theme,
        SettingKey::ThemeDark,
        tr(cx, "shell-menu-theme-dark"),
        &names,
    )
    .slider_on_commit(ui_scale_toast)
    .slider(
        size.clone(),
        SettingKey::AppearanceUiScalePct,
        tr(cx, "shell-menu-appearance-ui-scale"),
        75..=200,
        5,
        pct,
    )
    .slider(
        size,
        SettingKey::AppearanceRowHeightPct,
        tr(cx, "shell-menu-appearance-row-height"),
        80..=150,
        5,
        pct,
    )
    .switch(
        look.clone(),
        SettingKey::AppearanceRoundedCorners,
        tr(cx, "shell-menu-appearance-rounded-corners"),
        None,
    )
    .switch(
        look.clone(),
        SettingKey::AppearanceHeatColoring,
        tr(cx, "shell-menu-appearance-heat-coloring"),
        None,
    )
    .slider(
        look.clone(),
        SettingKey::AppearanceHeatIntensityPct,
        tr(cx, "shell-menu-appearance-heat-intensity"),
        0..=100,
        5,
        pct,
    )
    .slider(
        look,
        SettingKey::AppearanceShadowIntensityPct,
        tr(cx, "shell-menu-appearance-shadow-intensity"),
        0..=100,
        5,
        pct,
    )
    .restart_note_toast(cx, tr(cx, "shell-toast-decorations-restart"))
    .str_switch(
        window,
        SettingKey::AppearanceDecorations,
        tr(cx, "shell-settings-decorations"),
        "server",
        "client",
    );
    page
}

/// "UI scale changed to N%" in the active window, keyed like the menu's so repeats replace it.
/// Deferred: the slider's window is busy with the release.
fn ui_scale_toast(value: i64, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        return;
    };
    let text = scale_toast_text(value.clamp(0, u16::MAX as i64) as u16, cx);
    cx.defer(move |cx| {
        let pushed = window.update(cx, |_, window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| {
                    q.push(ToastKind::Info, text, Some(UI_SCALE_TOAST), None, cx)
                });
            }
        });
        if pushed.is_err() {
            warn!(target: "tasklens::ui", "UI scale toast: the window is gone");
        }
    });
}
