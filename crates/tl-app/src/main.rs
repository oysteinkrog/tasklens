//! tasklens: main, wiring, single instance, settings store and logging.
//!
//! `main` runs the UI-SHL-110 start-up sequence. `unsafe` is allowed in this file only
//! (ADR-001 rule 4).
#![allow(unsafe_code)]

#[allow(dead_code, reason = "later E33 beads call the rest")]
mod args;
mod atomic;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod crash;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod logging;
#[allow(dead_code, reason = "E33-006 calls it")]
mod migrations;
#[allow(dead_code, reason = "E33-012 sends the renderer notices")]
mod notify;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod paths;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod settings_store;
#[allow(dead_code, reason = "tests call the rest")]
mod single_instance;

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::settings::{EngineSettings, StartTab};
use tl_ui::app::ToastKindTag;
use tl_ui::{AppEnv, BuildInfo, ImportSummary, OpenMode, StartOptions, UiInit, UiLaunch};
use tracing::{error, info, warn};

use crate::args::TabArg;
use crate::settings_store::{LoadNotice, Store};
use crate::single_instance::{Forward, Outcome};

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
    crash::install_hook(paths.crash_dir(), recent_cell.clone(), main_panic_notify);
    // Dropped last (in `finish`), so buffered log lines are flushed.
    let log_guard = match logging::init(&paths.logs_dir()) {
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
    // Step 4: single instance. A second copy forwards its arguments and exits 0.
    let forward = Forward {
        tab: args.tab.map(TabArg::to_model),
        select_pid: args.select_pid,
    };
    let activation_token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
    // `_bus` keeps the bus name until `main` returns.
    let (_bus, app_requests) = match single_instance::acquire(&forward, activation_token) {
        Outcome::Forwarded => {
            info!("another tasklens is running; forwarded");
            return ExitCode::SUCCESS;
        }
        Outcome::NoBus => (None, None),
        Outcome::Primary { conn, requests } => (Some(conn), Some(requests)),
    };
    tl_ui::app::perf_step("single_instance");

    // Step 5: settings, state and themes.
    let now = unix_now();
    let loaded_settings = settings_store::load_settings(&paths.settings_file(), now);
    let loaded_state = settings_store::load_state(&paths.state_file(), now);
    let store = Arc::new(Store::new(
        &paths,
        loaded_settings.read_only,
        loaded_state.read_only,
        (loaded_settings.value.clone(), loaded_state.value.clone()),
    ));
    let startup_toasts = queued_toasts(&loaded_settings.notices, &loaded_state.notices)
        .into_iter()
        .map(|text| (ToastKindTag::Warning, text.into()))
        .collect();
    let settings = loaded_settings.value;
    let state = loaded_state.value;
    let themes_json = read_themes(&paths.themes_file());
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
    // Without a bus nothing sends requests; the closed channel ends tl-ui's request loop.
    let app_requests = app_requests.unwrap_or_else(|| async_channel::bounded(1).1);
    let write_store = store.clone();
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
        write_settings_now: Arc::new(move |s| write_store.write_settings_now(s)),
        set_autostart: Arc::new(|_, _| Ok(())),
        global_shortcut: None,
        import_taskslinger: Arc::new(|| Ok(ImportSummary::default())),
        list_adapters: Arc::new(Vec::new),
        run_history: None,
    };
    let exit = tl_ui::run(UiLaunch {
        client: Arc::new(engine.clone()),
        init: UiInit {
            settings,
            state,
            themes_json,
            persist: store.persist_callback(),
            persist_themes: store.persist_themes_callback(),
            app_requests,
            startup_toasts,
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

    // U-2: `Application::run` returns on Linux after `cx.quit()`, so this is the only path.
    info!(target: "tasklens::engine", ?exit, "shutdown after run returned");
    let once = ExitOnce::new((engine, log_guard));
    match once.take() {
        Some((engine, log_guard)) => finish(engine, &store, log_guard),
        None => ExitCode::SUCCESS,
    }
}

/// Holds a value that only the first `take` gets, so the exit steps run once even when two
/// paths race to run them.
struct ExitOnce<T>(Mutex<Option<T>>);

impl<T> ExitOnce<T> {
    fn new(value: T) -> Self {
        Self(Mutex::new(Some(value)))
    }

    fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The exit steps after the UI stops (ADR-010 Close behaviour, ADR-004 Shutdown): settings,
/// then the engine within 1500 ms, then the log file. Exits 0 without running destructors, so a
/// stuck engine thread cannot hold the process open.
fn finish(engine: EngineHandle, store: &Store, log: Option<logging::LogGuard>) -> ! {
    // Pending changes are written before anything else on exit (ADR-011).
    store.flush();
    let report = engine.shutdown(Duration::from_millis(1500));
    if !report.clean() {
        warn!(target: "tasklens::engine", drained = report.drained, torn_down = report.torn_down,
            "engine shutdown timed out");
    }
    // The non-blocking writer loses lines unless its guard is dropped before `exit`.
    drop(log);
    std::process::exit(0)
}

/// The time from `exec` to `main` (the dynamic loader and static initialisers), at the 10 ms
/// resolution of `/proc/uptime`.
fn exec_to_main_ms() -> Option<u64> {
    std::fs::read_to_string("/proc/self/stat")
        .ok()
        .zip(std::fs::read_to_string("/proc/uptime").ok())
        .and_then(|(stat, uptime)| tl_ui::window::main::first_frame_ms_from(&stat, &uptime))
}

/// The panic hook's main-thread callback: the desktop notification (APP-055).
fn main_panic_notify() {
    notify::send(notify::Fatal::Crashed);
}

/// Seconds since the Unix epoch, 0 if the clock is before it.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The load notices as toast texts, settings before state (UI-SHL-110 step 5).
fn queued_toasts(settings: &[LoadNotice], state: &[LoadNotice]) -> Vec<String> {
    settings
        .iter()
        .chain(state)
        .map(LoadNotice::toast_text)
        .collect()
}

/// The raw `themes.json` text, `None` when missing or unreadable. tl-ui parses it (UI-FND-042).
/// Bytes that are not UTF-8 are passed on lossily, so the parser reports them.
fn read_themes(path: &Path) -> Option<Arc<str>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(Arc::from(String::from_utf8_lossy(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warn!(target: "tasklens::settings", "themes.json: could not be read: {e}");
            None
        }
    }
}

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
    fn queued_toast_order() {
        let corrupt = |name: &str| LoadNotice::Corrupt {
            kept_as: name.to_owned(),
        };
        let texts = queued_toasts(
            &[corrupt("settings.toml.corrupt-1")],
            &[LoadNotice::Newer, corrupt("state.toml.corrupt-2")],
        );
        assert_eq!(
            texts,
            vec![
                corrupt("settings.toml.corrupt-1").toast_text(),
                LoadNotice::Newer.toast_text(),
                corrupt("state.toml.corrupt-2").toast_text(),
            ]
        );
        assert!(queued_toasts(&[], &[]).is_empty());
    }

    #[test]
    fn read_themes_missing_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read_themes(&dir.path().join("themes.json")).is_none());
        let path = dir.path().join("present.json");
        std::fs::write(&path, br#"{"themes":[]}"#).expect("write");
        assert_eq!(read_themes(&path).as_deref(), Some(r#"{"themes":[]}"#));
    }

    #[test]
    fn finish_runs_once() {
        let once = Arc::new(ExitOnce::new(7_u32));
        let other = once.clone();
        let thread = std::thread::spawn(move || other.take());
        let here = once.take();
        let there = thread.join().expect("join");
        assert_eq!([here, there].iter().flatten().collect::<Vec<_>>(), vec![&7]);
        assert_eq!(once.take(), None);
    }

    #[test]
    fn start_tab_maps_every_tab() {
        assert_eq!(start_tab(TabArg::Processes), StartTab::Processes);
        assert_eq!(start_tab(TabArg::Performance), StartTab::Performance);
        assert_eq!(start_tab(TabArg::Information), StartTab::Information);
    }
}
