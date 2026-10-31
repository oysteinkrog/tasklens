//! Settings window, Advanced page, P2 rows (E21-A2, UI-SHL-071, APP-023, APP-032, R-071).
//! The settings folder comes from `AppEnv`, and opening it goes through the action tracker.
//!
//! P4 row (E21-040, UI-SHL-071, R-237): "Rendering GPU" comes first, a dropdown of "Auto", the
//! adapters and "Software renderer". Each time a Settings window opens it runs the same adapter
//! probe as the Settings menu; the adapters it finds stay in [`ProbedAdapters`] until the next
//! probe. A change shows "Takes effect after restart" and the toast "Restart tasklens to use the
//! new GPU".

use std::path::Path;
use std::rc::Rc;

use gpui_kit::{App, Global, PathPromptOptions, SharedString, Window};
use tracing::warn;

use super::{
    PageBuilder, PageId, PickValue, SettingsWindow, info_toast, kind_toast, read, value_matches,
};
use crate::action_tracker::ToastTexts;
use crate::app::{AdapterInfo, AppEnv};
use crate::components::button::TlButtonKind;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::host::HostContext;
use crate::i18n::{tr, tr_args};
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::theme::ThemeStore;
use crate::window::about::{file_uri, send_open_uri};
use crate::window::confirm::ResetSettings;
use crate::window::menus_settings::{adapter_setting_value, probe_adapters};

gpui_kit::actions!(tasklens, [OpenSettingsFolder]);

const OPEN_FAILED: &str = "shell-about-logs-failed";

/// The adapters the last probe found, for the Rendering GPU dropdown.
#[derive(Default)]
struct ProbedAdapters(Vec<AdapterInfo>);

impl Global for ProbedAdapters {}

pub(super) fn init(cx: &mut App) {
    cx.observe_new(|_: &mut SettingsWindow, _, cx| {
        let probe = probe_adapters(cx);
        cx.spawn(async move |this, cx| {
            let adapters = probe.await;
            let _ = this.update(cx, |_, cx| {
                cx.set_global(ProbedAdapters(adapters));
                cx.notify();
            });
        })
        .detach();
    })
    .detach();
    cx.on_action(|_: &OpenSettingsFolder, cx| {
        let Some(window) = cx.active_window() else {
            warn!(target: "tasklens::ui", "no window; the settings folder was not opened");
            return;
        };
        let Some(dir) = cx.try_global::<AppEnv>().map(|env| env.config_dir.clone()) else {
            warn!(target: "tasklens::ui", "no AppEnv; the settings folder was not opened");
            return;
        };
        let failed = tr(cx, OPEN_FAILED);
        cx.defer(move |cx| {
            if window
                .update(cx, |_, window, cx| {
                    if ToastQueue::of_window(window, cx).is_none() {
                        warn!(target: "tasklens::ui", "no toast queue; the settings folder was not opened");
                        return;
                    }
                    send_open_uri(
                        file_uri(&dir),
                        ToastTexts::new("", failed),
                        window,
                        cx,
                    );
                })
                .is_err()
            {
                warn!(target: "tasklens::ui", "settings folder: the window is gone");
            }
        });
    });
}

pub fn page(host: &HostContext, cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(host, PageId::Advanced);
    let rendering = tr(cx, "shell-settings-group-rendering");
    let network = tr(cx, "shell-settings-group-network");
    let debug = tr(cx, "shell-settings-group-debug");
    let files = tr(cx, "shell-settings-group-files");
    let toast = tr(cx, "shell-toast-gpu-restart");
    let key = SettingKey::RenderingGpu;
    // Only a change of value toasts: picking the GPU already in use needs no restart.
    let reader = host.clone();
    let write: PickValue = Rc::new(move |value, cx| {
        let changed = !value_matches(value, &read(&reader, cx, key));
        TlSettings::set(cx, key, value.clone());
        if changed {
            info_toast(toast.clone(), cx);
        }
    });
    let restart = page.restart_note(cx).take_restart();
    page.dropdown_with(
        rendering,
        key,
        tr(cx, "shell-menu-settings-rendering-gpu"),
        &gpu_options(cx),
        restart,
        write,
    );
    page.switch(
        network,
        SettingKey::NetworkPublicIpNoticeAccepted,
        tr(cx, "shell-settings-public-ip-notice"),
        None,
    )
    .switch(
        debug,
        SettingKey::DebugShowFps,
        tr(cx, "shell-settings-show-fps"),
        None,
    )
    .button_fn(
        files.clone(),
        tr(cx, "shell-settings-import-taskslinger"),
        TlButtonKind::Secondary,
        import_taskslinger,
    )
    .button(
        files.clone(),
        tr(cx, "shell-settings-open-folder"),
        Box::new(OpenSettingsFolder),
        TlButtonKind::Secondary,
    )
    .button(
        files,
        tr(cx, "shell-menu-settings-reset"),
        Box::new(ResetSettings),
        TlButtonKind::Danger,
    );
    page
}

/// "Import from TaskSlinger…" (APP-034): the portal file chooser, then the import of the picked
/// file. Cancelling the chooser does nothing.
fn import_taskslinger(_: &mut Window, cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        cx.update(|cx| import_picked(&path, cx));
    })
    .detach();
}

/// Imports the picked file and toasts the result. A `.json` file is a `themes.json`: only its
/// themes are imported. Any other file is a `UserSettings.ini`. Neither the path nor the file
/// content is logged.
fn import_picked(path: &Path, cx: &mut App) {
    let Some((read_themes, import)) = cx
        .try_global::<AppEnv>()
        .map(|env| (env.read_ts_themes.clone(), env.import_taskslinger.clone()))
    else {
        warn!(target: "tasklens::ui", "no AppEnv; the TaskSlinger import did not run");
        return;
    };
    let is_themes = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
    let counts = if is_themes {
        read_themes(path)
            .and_then(|text| {
                ThemeStore::import_taskslinger(&text, cx).map_err(|e| format!("{e:?}"))
            })
            .map(|themes| (0, u32::try_from(themes).unwrap_or(u32::MAX)))
    } else {
        let settings = TlSettings::settings(cx).clone();
        let state = TlSettings::state(cx).clone();
        import(path, &settings, &state).map(|imported| {
            TlSettings::update(cx, |d| {
                d.settings = imported.settings;
                d.state = imported.state;
            });
            (imported.summary.settings, imported.summary.themes)
        })
    };
    match counts {
        Ok((settings, themes)) => {
            let args = [("settings", settings.into()), ("themes", themes.into())];
            kind_toast(
                ToastKind::Success,
                tr_args(cx, "shell-toast-ts-import", &args),
                cx,
            );
        }
        Err(error) => {
            warn!(target: "tasklens::ui", %error, "TaskSlinger import failed");
            kind_toast(ToastKind::Error, tr(cx, "shell-toast-ts-import-failed"), cx);
        }
    }
}

/// "Auto", the probed adapters and "Software renderer", as `(label, rendering.gpu value)`.
fn gpu_options(cx: &App) -> Vec<(SharedString, SettingValue)> {
    let str_value = |v: String| SettingValue::Str(v.into());
    let mut options = vec![(
        tr(cx, "shell-menu-settings-gpu-auto"),
        str_value("auto".to_owned()),
    )];
    if let Some(probed) = cx.try_global::<ProbedAdapters>() {
        options.extend(probed.0.iter().map(|adapter| {
            (
                SharedString::from(adapter.name.clone()),
                str_value(adapter_setting_value(adapter)),
            )
        }));
    }
    options.push((
        tr(cx, "shell-menu-settings-gpu-software"),
        str_value("software".to_owned()),
    ));
    options
}
