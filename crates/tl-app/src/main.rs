//! tasklens: main, wiring, single instance, settings store and logging.
//!
//! `main` runs the UI-SHL-110 start-up sequence. `unsafe` is allowed in this file only
//! (ADR-001 rule 4).
#![allow(unsafe_code)]

#[allow(dead_code, reason = "later E33 beads call the rest")]
mod args;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod crash;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod logging;
#[allow(dead_code, reason = "E33-006 calls it")]
mod migrations;
#[allow(dead_code, reason = "later E33 beads call it")]
mod notify;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod paths;
#[allow(dead_code, reason = "E33-A2 calls load_settings and load_state")]
mod settings_store;
#[allow(dead_code, reason = "E33-A2 calls acquire")]
mod single_instance;

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use tl_engine::{Engine, EngineConfig, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::settings::{EngineSettings, Settings, StartTab, UiState};
use tl_ui::{AppEnv, BuildInfo, ImportSummary, OpenMode, StartOptions, UiInit, UiLaunch};
use tracing::{error, info};

use crate::args::TabArg;

const NV_X11_VAR: &str = "__NV_FORCE_ENABLE_X11_EGL_PLATFORM";
const NVIDIA_VERSION_FILE: &str = "/proc/driver/nvidia/version";
const HELPER_PATH: &str = "/usr/lib/tasklens/tl-helper";

fn main() -> ExitCode {
    tl_ui::app::perf_origin(Instant::now());
    let exec_to_main = exec_to_main_ms();
    // Step 1: paths, panic hook, logging.
    let paths = match paths::Paths::from_env() {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("tasklens: {e}");
            return ExitCode::from(1);
        }
    };
    let recent_cell = Arc::new(OnceLock::new());
    crash::install_hook(paths.crash_dir(), recent_cell.clone(), noop);
    // Dropped last, so buffered log lines are flushed.
    let _log_guard = match logging::init(&paths.logs_dir()) {
        Ok((guard, recent)) => {
            let _ = recent_cell.set(recent);
            Some(guard)
        }
        Err(e) => {
            eprintln!("tasklens: logging did not start: {e}");
            None
        }
    };
    if let Some(ms) = exec_to_main {
        info!(target: "tasklens::perf", exec_to_main_ms = ms, "process start");
    }
    tl_ui::app::perf_step("logging");

    // Step 2: arguments. clap exits 2 on an unknown argument.
    let args = args::parse();
    let debug = debug_enabled(args.debug, std::env::var_os("TASKLENS_DEBUG"));

    // Step 3: the NVIDIA X11 rule (ADR-010), before any thread starts.
    if nvidia_x11_rule(
        std::env::var_os("WAYLAND_DISPLAY"),
        Path::new(NVIDIA_VERSION_FILE).exists(),
        std::env::var_os(NV_X11_VAR),
    ) {
        // SAFETY: no other thread exists yet. The log writer thread from step 1 never reads
        // the environment.
        unsafe { std::env::set_var(NV_X11_VAR, "1") };
        info!(target: "tasklens::engine", "NVIDIA on X11: set __NV_FORCE_ENABLE_X11_EGL_PLATFORM=1");
    }

    // E32: --low-memory and --watch-memory arms go here (before step 4)
    // Step 4: E33-A2 inserts single instance here.
    // Step 5: E33-A2 inserts settings, state and themes loading here.
    let settings = Settings::default();
    let state = UiState::default();
    tl_ui::app::perf_step("args_settings");

    // E01: --bench and --gallery arms go here (before step 7)

    // Step 7: start the engine. Do not wait for any collector.
    let mut config = EngineConfig::new(EngineMode::Full, paths.state_dir.clone())
        .with_history_dir(paths.history_dir());
    if Path::new(HELPER_PATH).exists() {
        config = config.with_helper_path(HELPER_PATH.into());
    }
    let engine = match Engine::start(config, EngineHooks::default()) {
        Ok(engine) => engine,
        Err(e) => {
            error!(target: "tasklens::engine", "engine start failed: {e}");
            eprintln!("tasklens could not start the engine: {e}");
            return ExitCode::from(1);
        }
    };
    engine.set_settings(EngineSettings::from(&settings));
    info!(target: "tasklens::engine", "engine started");
    tl_ui::app::perf_step("engine_start");

    // Steps 8 to 10: tl-ui runs the GPUI application and opens the first window.
    // Kept alive until the UI stops; E33-A2 hands it to the single-instance handler.
    let (_app_requests_tx, app_requests) = async_channel::bounded(4);
    let env = AppEnv {
        debug,
        config_dir: paths.config_dir.clone(),
        state_dir: paths.state_dir.clone(),
        logs_dir: paths.logs_dir(),
        build: BuildInfo {
            version: env!("CARGO_PKG_VERSION"),
            git_hash: env!("TASKLENS_GIT_SHA"),
            ..BuildInfo::default()
        },
        pending_crash: None,
        mark_crash_seen: Arc::new(|| {}),
        write_settings_now: Arc::new(|_| Ok(())),
        set_autostart: Arc::new(|_, _| Ok(())),
        global_shortcut: None,
        import_taskslinger: Arc::new(|| Ok(ImportSummary::default())),
        list_adapters: Arc::new(Vec::new),
        run_history: None,
    };
    let exit = tl_ui::run(UiLaunch {
        client: Arc::new(engine),
        init: UiInit {
            settings,
            state,
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests,
            startup_toasts: Vec::new(),
            env,
        },
        open: OpenMode::Main,
        start: StartOptions {
            tab: args.tab.map(start_tab),
            select_pid: args.select_pid,
            start_minimized: args.start_minimized,
            shift_held: false,
            pending_crash_report: None,
        },
    });

    // E33-011 adds the 1500 ms engine shutdown here.
    info!(target: "tasklens::engine", ?exit, "ui stopped");
    ExitCode::SUCCESS
}

/// The time from `exec` to `main` (the dynamic loader and static initialisers), at the 10 ms
/// resolution of `/proc/uptime`.
fn exec_to_main_ms() -> Option<u64> {
    std::fs::read_to_string("/proc/self/stat")
        .ok()
        .zip(std::fs::read_to_string("/proc/uptime").ok())
        .and_then(|(stat, uptime)| tl_ui::window::main::first_frame_ms_from(&stat, &uptime))
}

/// The panic hook's main-thread callback. E33-011 fills it.
fn noop() {}

/// True when `--debug` is given or `TASKLENS_DEBUG` is `1`.
fn debug_enabled(flag: bool, env: Option<OsString>) -> bool {
    flag || env.is_some_and(|v| v == "1")
}

/// True when tasklens must set `__NV_FORCE_ENABLE_X11_EGL_PLATFORM=1` (ADR-010): the session
/// is X11 (`WAYLAND_DISPLAY` unset or empty), the NVIDIA driver is loaded and the user has not
/// set the variable.
fn nvidia_x11_rule(
    wayland: Option<OsString>,
    nvidia_present: bool,
    current: Option<OsString>,
) -> bool {
    let x11 = wayland.is_none_or(|w| w.is_empty());
    x11 && nvidia_present && current.is_none()
}

/// The `--tab` value as a start tab.
fn start_tab(tab: TabArg) -> StartTab {
    use tl_model::command::Tab;
    match tab.to_model() {
        Tab::Processes => StartTab::Processes,
        Tab::Performance => StartTab::Performance,
        Tab::Services => StartTab::Services,
        Tab::Startup => StartTab::Startup,
        Tab::Connections => StartTab::Connections,
        Tab::Information => StartTab::Information,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    #[test]
    fn x11_rule_sets_when_x11_nvidia_unset() {
        assert!(nvidia_x11_rule(None, true, None));
    }

    #[test]
    fn x11_rule_skips_on_wayland() {
        assert!(!nvidia_x11_rule(os("wayland-0"), true, None));
    }

    #[test]
    fn x11_rule_skips_without_nvidia() {
        assert!(!nvidia_x11_rule(None, false, None));
    }

    #[test]
    fn x11_rule_respects_user_value() {
        assert!(!nvidia_x11_rule(None, true, os("0")));
    }

    #[test]
    fn x11_rule_empty_wayland_counts_as_unset() {
        assert!(nvidia_x11_rule(os(""), true, None));
    }

    #[test]
    fn debug_from_env() {
        assert!(debug_enabled(false, os("1")));
        assert!(!debug_enabled(false, os("0")));
        assert!(!debug_enabled(false, None));
        assert!(debug_enabled(true, None));
    }

    #[test]
    fn start_tab_maps_every_tab() {
        assert_eq!(start_tab(TabArg::Processes), StartTab::Processes);
        assert_eq!(start_tab(TabArg::Performance), StartTab::Performance);
        assert_eq!(start_tab(TabArg::Information), StartTab::Information);
    }
}
