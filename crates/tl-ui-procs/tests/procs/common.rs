//! The setup the process tests share. `TasklensUi::init` installs more than `PageHost::install`
//! does; [`shell_free_init`] adds the part a process page or window needs, without a main window.

use std::sync::Arc;

use gpui_kit::{App, TestAppContext};
use tl_ui_base::app_env::AppEnv;
use tl_ui_base::{action_tracker, request_ends};
use tl_ui_kit::components::toast;
use tl_ui_kit::{actions, helper_flow};
use tl_ui_pagehost::PageHost;
use tl_ui_procs::pages::processes::bind_keys_processes;
use tl_ui_procs::pages::processes::history::actions::bind_keys_history;
use tl_ui_procs::windows::actions::bind_keys_process_windows;
use tl_ui_procs::windows::handle_search;
use tl_ui_settings::settings::{Settings, UiState};
use tl_ui_table::table::actions::bind_keys_table;
use tl_ui_testkit::fake_engine::FakeEngine;

/// What `TasklensUi::init` installs for the process windows and the Processes page, on top of
/// the page host: the toast queues, the key bindings, the action tracker, the request ends, the
/// helper flow and the handle search open hook.
pub fn install_shell_services(host: &PageHost, cx: &mut App) {
    actions::bind_keys_global(cx);
    bind_keys_table(cx);
    bind_keys_process_windows(cx);
    bind_keys_processes(cx);
    bind_keys_history(cx);
    actions::register_global_handlers(cx);
    toast::init(cx);
    action_tracker::install(Arc::clone(&host.client), Some(host.hub.clone()), cx);
    request_ends::install(Arc::clone(&host.client), cx);
    helper_flow::install(cx);
    handle_search::init(cx);
}

/// A new `FakeEngine` with the page host and the shell services installed.
pub fn shell_free_init(cx: &mut TestAppContext, settings: Settings) -> (Arc<FakeEngine>, PageHost) {
    shell_free_init_with(cx, settings, UiState::default(), AppEnv::test())
}

/// [`shell_free_init`] with a persisted `state` and an `env`.
pub fn shell_free_init_with(
    cx: &mut TestAppContext,
    settings: Settings,
    state: UiState,
    env: AppEnv,
) -> (Arc<FakeEngine>, PageHost) {
    let engine = FakeEngine::new();
    let host = PageHost::install_with(cx, Arc::clone(&engine), settings, state, env);
    cx.update(|cx| {
        install_shell_services(&host, cx);
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    (engine, host)
}
