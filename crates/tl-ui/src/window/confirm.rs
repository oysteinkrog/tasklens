//! Shell confirmations (E21-005, UI-SHL-072, UI-SHL-091, APP-022, UI-FND-021, R-104).
//!
//! This file owns the texts of the shell's confirmations: reset settings (here), delete theme
//! (E21-013) and skip password prompts (E21-042). Each opens a `TlDialog` modal to the window it
//! was started from.
//!
//! Reset settings writes `settings.toml` with the defaults first, through tl-app's synchronous
//! `AppEnv::write_settings_now` (R-171). Only when that write works does it replace the settings
//! in `TlSettings` and re-apply the default theme. `state.toml`, `themes.json` and history are
//! never touched. No setting value is logged.

use gpui_kit::{App, AppContext as _, Window};
use tl_model::settings::Settings;
use tracing::warn;

use crate::app::AppEnv;
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::tr;
use crate::settings_global::TlSettings;
use crate::theme::store::{ActivateSource, activate};

gpui_kit::actions!(tasklens, [ResetSettings]);

/// Registers the `ResetSettings` handler. Called once from `TasklensUi::init`.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &ResetSettings, cx| {
        let Some(window) = cx.active_window() else {
            warn!(target: "tasklens::ui", "no window for the reset settings confirmation");
            return;
        };
        // The window that dispatched the action may be updating now.
        cx.defer(move |cx| {
            if window
                .update(cx, |_, window, cx| confirm_reset_settings(window, cx))
                .is_err()
            {
                warn!(target: "tasklens::ui", "reset settings: the window is gone");
            }
        });
    });
}

/// Opens "Reset all settings to their defaults? Themes and history are kept." with Cancel and a
/// Danger "Reset". Focus starts on Cancel, so Enter cancels; Esc cancels too.
pub fn confirm_reset_settings(window: &mut Window, cx: &mut App) {
    let dialog = TlDialog::confirm(
        tr(cx, "shell-confirm-reset-text"),
        tr(cx, "shell-confirm-reset-button"),
    )
    .title(tr(cx, "shell-confirm-title"))
    .destructive();
    let opened = dialog.open(window, cx, |result, window, cx| {
        if let DialogResult::Confirmed { .. } = result {
            reset_settings(window, cx);
        }
    });
    if opened.is_err() {
        warn!(target: "tasklens::ui", "reset settings confirmation did not open");
    }
}

/// Writes the default settings, then applies them and the default theme, and shows the result
/// as a toast in `window`. On a write failure nothing changes.
pub fn reset_settings(window: &mut Window, cx: &mut App) {
    let defaults = Settings::default();
    let written = match cx.try_global::<AppEnv>() {
        Some(env) => (env.write_settings_now.clone())(&defaults),
        None => Err("no AppEnv".to_owned()),
    };
    if written.is_err() {
        warn!(target: "tasklens::ui", "reset settings: settings.toml was not written");
        toast(
            window,
            cx,
            ToastKind::Error,
            "shell-toast-settings-reset-failed",
        );
        return;
    }
    let theme = defaults.theme.active.clone();
    TlSettings::update(cx, |d| d.settings = defaults);
    // Scale and row height follow through the `appearance` section observer (UI-SHL-130).
    if let Err(e) = activate(&theme, ActivateSource::User, cx) {
        warn!(target: "tasklens::ui", ?e, "reset settings: the default theme was not activated");
    }
    toast(window, cx, ToastKind::Success, "shell-toast-settings-reset");
}

fn toast(window: &Window, cx: &mut App, kind: ToastKind, key: &str) {
    let text = tr(cx, key);
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast queue for the reset settings result"),
    }
}
