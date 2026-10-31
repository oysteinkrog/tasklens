//! tasklens: main, wiring, single instance, settings store and logging.
//!
//! `main` runs the UI-SHL-110 start-up sequence. `unsafe` is allowed in this file only
//! (ADR-001 rule 4).
#![allow(unsafe_code)]

mod adapters;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod args;
mod atomic;
mod autostart;
#[cfg(feature = "bench")]
mod bench_main;
mod bridge;
mod build_info;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod crash;
mod engine_link;
#[cfg(feature = "bench")]
mod gallery_main;
mod host_file;
mod import_ts;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod logging;
#[allow(dead_code, reason = "E32-004 calls it")]
mod lowmem_unit;
#[allow(dead_code, reason = "E33-006 calls it")]
mod migrations;
#[allow(dead_code, reason = "only tests call `Fatal::key`")]
mod notify;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod paths;
mod rendering_env;
#[allow(dead_code, reason = "later E33 beads call the rest")]
mod settings_store;
#[allow(dead_code, reason = "tests call the rest")]
mod single_instance;
mod switch_client;

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::remote_proto::{HelloTail, Want};
use tl_model::settings::StartTab;
use tl_ui::app::ToastKindTag;
use tl_ui::engine_link::{EngineLinkApi, EngineLinkState};
use tl_ui::{AppEnv, OpenMode, StartOptions, UiExit, UiInit, UiLaunch};
use tracing::{error, info, warn};

use crate::args::TabArg;
use crate::engine_link::{EngineLink, LinkEnv};
use crate::settings_store::{LoadNotice, Store};
use crate::single_instance::{Forward, Outcome};

const NV_X11_VAR: &str = "__NV_FORCE_ENABLE_X11_EGL_PLATFORM";
const NVIDIA_VERSION_FILE: &str = "/proc/driver/nvidia/version";
const HELPER_PATH: &str = "/usr/lib/tasklens/tl-helper";
const NO_DISPLAY_MSG: &str = "tasklens needs a Wayland or X11 display";
/// Set on the one automatic relaunch after device loss (ADR-015, R-153).
const RELAUNCHED_VAR: &str = "TASKLENS_RELAUNCHED";

/// Replaces this process with `tasklens-watch` (tasklens-m1f.19). Returns only on failure.
fn exec_watcher() -> ExitCode {
    use std::os::unix::process::CommandExt;
    let Some(exe) = lowmem_unit::watcher_exe() else {
        eprintln!(
            "tasklens: {} not found next to tasklens or on PATH",
            lowmem_unit::WATCHER_NAME
        );
        return ExitCode::from(1);
    };
    let err = Command::new(&exe).exec();
    eprintln!("tasklens: starting {} failed: {err}", exe.display());
    ExitCode::from(1)
}

fn main() -> ExitCode {
    tl_ui::app::perf_origin(Instant::now());
    let exec_to_main = exec_to_main_ms();
    // Step 2 runs first: `--app-id` decides the paths. clap exits 2 on an unknown argument.
    let args = args::parse();
    if let Some(id) = args
        .app_id
        .as_deref()
        .filter(|id| !single_instance::valid_app_id(id))
    {
        eprintln!("Invalid --app-id: {id}");
        return ExitCode::from(2);
    }
    // E32: `--watch-memory` is kept for old unit files. It execs `tasklens-watch` before paths
    // and logging, so the watcher starts with no file logger and no writer thread (R-152 OI-4).
    if args.watch_memory {
        return exec_watcher();
    }
    // `tasklens bridge` runs over ssh: no display, and stdout carries only protocol bytes, so
    // it starts before paths and logging (ADR-018 Remote transport).
    if args.command == Some(args::Cmd::Bridge) {
        return bridge::main(args.app_id.as_deref());
    }

    // Step 1: paths, panic hook, logging.
    let paths = match paths::Paths::from_env_for(args.app_id.as_deref()) {
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

    // Without a display gpui falls back to its headless platform and tasklens runs with no
    // window (tasklens-cr0.29). Autostart (`--start-minimized`) and `--low-memory` exit here too.
    if !has_display(
        std::env::var_os("WAYLAND_DISPLAY"),
        std::env::var_os("DISPLAY"),
    ) {
        error!(target: "tasklens::ui", "{NO_DISPLAY_MSG}");
        eprintln!("{NO_DISPLAY_MSG}");
        return ExitCode::from(1);
    }

    // Step 2: arguments (parsed above).
    let debug = debug_enabled(args.debug, std::env::var_os("TASKLENS_DEBUG"));

    // settings.toml is read before step 3, which needs `rendering.gpu`.
    let now = unix_now();
    let loaded_settings = settings_store::load_settings(&paths.settings_file(), now);

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

    let gpu_env = rendering_env::env_for(
        &loaded_settings.value.rendering.gpu,
        std::env::var_os("ZED_DEVICE_ID").as_deref(),
        rendering_env::pci_device_id,
    );
    for (name, value) in gpu_env {
        // SAFETY: as above, no other thread reads the environment yet.
        unsafe { std::env::set_var(name, &value) };
        info!(target: "tasklens::ui", "rendering.gpu: set {name}={value}");
    }

    // E32: --low-memory skips single instance (APP-011) and opens the rescue window.
    // E01: the bench path skips single instance, so a bench run can start while tasklens is open.
    #[cfg(feature = "bench")]
    if let Some(scene) = args.bench {
        let scene = clap::ValueEnum::to_possible_value(&scene)
            .map(|v| v.get_name().to_owned())
            .unwrap_or_default();
        let code = bench_main::run(bench_main::BenchArgs {
            scene,
            duration_s: args.duration.unwrap_or(30),
            warmup_s: args.warmup.unwrap_or(5),
            json: args.json.clone(),
        });
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    // E01: the gallery skips single instance too; `--bench` is checked first.
    #[cfg(feature = "bench")]
    if args.gallery {
        let code = gallery_main::run();
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    // Step 4: single instance. A second copy forwards its arguments and exits 0.
    let forward = Forward {
        tab: args.tab.map(TabArg::to_model),
        select_pid: args.select_pid,
    };
    let activation_token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
    // `bus` keeps the bus name until the process exits, or until a relaunch closes it.
    let (bus, app_requests) = if args.low_memory {
        (None, None)
    } else {
        match single_instance::acquire(args.app_id.as_deref(), &forward, activation_token) {
            Outcome::Forwarded => {
                info!("another tasklens is running; forwarded");
                return ExitCode::SUCCESS;
            }
            Outcome::NoBus => (None, None),
            Outcome::Primary {
                conn,
                requests,
                sender,
            } => (Some(conn), Some((sender, requests))),
        }
    };
    tl_ui::app::perf_step("single_instance");

    // Step 5: state and themes (settings were loaded before step 3).
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

    // Step 7: the engine. The main window uses the engine service, or its own engine when the
    // service does not answer (ADR-018). The rescue window keeps its own engine, with no history
    // and no lock. Do not wait for any collector.
    let (engine, link_api) = match start_engine(&paths, &args) {
        Ok(started) => started,
        Err(e) => {
            error!(target: "tasklens::engine", "engine start failed: {e}");
            eprintln!("tasklens could not start the engine: {e}");
            return ExitCode::from(1);
        }
    };
    let client = engine.client();
    info!(target: "tasklens::engine", "engine started");
    tl_ui::app::perf_step("engine_start");

    // Steps 8 to 10: tl-ui runs the GPUI application and opens the first window.
    // Without a bus only tl-app itself sends requests (the lowmem unit).
    let (request_tx, app_requests) =
        app_requests.unwrap_or_else(|| async_channel::bounded(single_instance::QUEUE));
    let write_store = store.clone();
    let env = AppEnv {
        debug,
        config_dir: paths.config_dir.clone(),
        state_dir: paths.state_dir.clone(),
        logs_dir: paths.logs_dir(),
        build: build_info::build_info(),
        pending_crash: None,
        mark_crash_seen: Arc::new(|| {}),
        write_settings_now: Arc::new(move |s| write_store.write_settings_now(s)),
        set_autostart: Arc::new(|_, _| Ok(())),
        global_shortcut: None,
        import_taskslinger: Arc::new(import_ts::import_file),
        read_ts_themes: Arc::new(import_ts::read_themes_file),
        list_adapters: Arc::new(adapters::list_adapters),
        run_history: None,
        engine_link: link_api,
    };
    let persist = lowmem_unit::wrap_persist(
        store.persist_callback(),
        settings.lowmem.enabled,
        request_tx,
    );
    // PROC-098 as amended by R-243: current pages only. A GPUI process is far above the default
    // RLIMIT_MEMLOCK, so a failure is expected on some machines and is not fatal.
    if args.low_memory
        && let Err(e) = rustix::mm::mlockall(rustix::mm::MlockAllFlags::CURRENT)
    {
        warn!(target: "tasklens::engine", "mlockall failed: {e}");
    }
    // No tray in the low-memory rescue window (R-243).
    let tray = if args.low_memory { None } else { start_tray() };
    let exit = tl_ui::run(UiLaunch {
        client,
        init: UiInit {
            settings,
            state,
            themes_json,
            persist,
            persist_themes: store.persist_themes_callback(),
            app_requests,
            startup_toasts,
            env,
        },
        open: if args.low_memory {
            OpenMode::LowMemory
        } else {
            OpenMode::Main
        },
        start: StartOptions {
            tab: args.tab.map(start_tab),
            select_pid: args.select_pid,
            start_minimized: args.start_minimized,
            shift_held: false,
            pending_crash_report: None,
            app_id: args.app_id.clone(),
        },
        tray,
    });

    // U-2: `Application::run` returns on Linux after `cx.quit()`, so this is the only path.
    info!(target: "tasklens::engine", ?exit, "shutdown after run returned");
    let once = ExitOnce::new((engine, log_guard));
    let Some((engine, log_guard)) = once.take() else {
        return ExitCode::SUCCESS;
    };
    if let Some(kind) = exit_notice(&exit) {
        notify::send(kind);
    }
    match exit {
        UiExit::Quit => finish(engine, &store, log_guard, 0, || {}),
        UiExit::RendererFailed(msg) => {
            error!(target: "tasklens::ui", "renderer start failed: {msg}");
            eprintln!("tasklens could not start the renderer");
            finish(engine, &store, log_guard, 1, || {})
        }
        UiExit::DeviceLost => {
            let again = should_relaunch(std::env::var_os(RELAUNCHED_VAR));
            finish(engine, &store, log_guard, 0, move || {
                if again {
                    relaunch(bus);
                } else {
                    warn!(target: "tasklens::ui", "device lost after a relaunch; not relaunching again");
                }
            })
        }
    }
}

/// The notification `exit` sends, if any. A main window that did not open is `RendererFailed`.
fn exit_notice(exit: &UiExit) -> Option<notify::Fatal> {
    match exit {
        UiExit::Quit => None,
        UiExit::RendererFailed(_) => Some(notify::Fatal::RendererFailed),
        UiExit::DeviceLost => Some(notify::Fatal::DeviceLost),
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

/// The window's engine.
enum WindowEngine {
    /// The rescue window's own engine (`--low-memory`): no history, no lock.
    Rescue(EngineHandle),
    /// The engine service, or the window's own engine when the service does not answer.
    Link(Arc<EngineLink>),
}

impl WindowEngine {
    /// The client the UI uses for the whole run.
    fn client(&self) -> Arc<dyn EngineClient> {
        match self {
            WindowEngine::Rescue(engine) => Arc::new(engine.clone()),
            WindowEngine::Link(link) => link.client(),
        }
    }

    /// Stops the window's own engine within 1500 ms. The engine service keeps running.
    fn shutdown(self) {
        match self {
            WindowEngine::Rescue(engine) => stop_engine(engine),
            WindowEngine::Link(link) => link.shutdown(),
        }
    }
}

/// Stops an in-process engine (ADR-004 Shutdown), which flushes history.
fn stop_engine(engine: EngineHandle) {
    let report = engine.shutdown();
    if !report.clean() {
        warn!(target: "tasklens::engine", drained = report.drained, torn_down = report.torn_down,
            "engine shutdown timed out");
    }
}

/// The in-process engine's config. The rescue window (`--low-memory`) gets no history directory,
/// also with `--force-full-memory-mode`, so it writes no history and needs no engine lock
/// (ADR-018 Rescue mode).
fn engine_config(paths: &paths::Paths, low_memory: bool, force_full: bool) -> EngineConfig {
    let mut config =
        EngineConfig::new(engine_mode(low_memory, force_full), paths.state_dir.clone());
    if !low_memory {
        config = config.with_history_dir(paths.history_dir());
    }
    if Path::new(HELPER_PATH).exists() {
        config = config.with_helper_path(HELPER_PATH.into());
    }
    config
}

/// Starts the window's engine, and for the main window the API the status chip uses.
fn start_engine(
    paths: &paths::Paths,
    args: &args::Args,
) -> Result<(WindowEngine, Option<EngineLinkApi>), String> {
    if args.low_memory {
        // The rescue window holds no lock, so it only reads engine.toml (ADR-018 Rescue mode).
        let host = host_file::read_only(&paths.config_dir);
        let config = engine_config(paths, true, args.force_full_memory_mode);
        let engine = Engine::start(config, EngineHooks::default()).map_err(|e| e.to_string())?;
        engine.install_host_settings(host);
        return Ok((WindowEngine::Rescue(engine), None));
    }
    if let Err(e) = paths::ensure_dir(&paths.state_dir) {
        warn!(target: "tasklens::engine", "state directory: {e}");
    }
    let app_dir = args.app_id.as_deref().unwrap_or(paths::APP_DIR);
    let (states_tx, states) = async_channel::unbounded();
    let engine_paths = paths.clone();
    let env = LinkEnv {
        socket: engine_link::socket_path(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), app_dir),
        lock_file: paths.state_dir.join(engine_link::LOCK_FILE),
        // The unit serves the default paths only; an `--app-id` instance stays on its own
        // (D-016).
        start_unit: args.app_id.is_none().then(|| {
            Box::new(engine_link::start_unit)
                as Box<dyn Fn(std::time::Duration) -> bool + Send + Sync>
        }),
        // Runs only while the window holds the engine lock, so it owns engine.toml, as the
        // service does (ADR-018 Settings, tasklens-q4x76.24).
        start_engine: Box::new(move || {
            let file = host_file::open(&engine_paths.config_dir);
            let config = engine_config(&engine_paths, false, false);
            let engine =
                Engine::start(config, EngineHooks::default()).map_err(|e| e.to_string())?;
            let applier = engine.clone();
            Ok(host_file::local_engine(
                file,
                Arc::new(engine.clone()),
                Box::new(move |current| applier.install_host_settings(current.clone())),
                Box::new(move || stop_engine(engine)),
            ))
        }),
        hello: HelloTail {
            pid: std::process::id(),
            app_id: Arc::from(
                args.app_id
                    .as_deref()
                    .unwrap_or(tl_ui::window::main::APP_ID),
            ),
            bridge: false,
            want: Want { snapshots: true },
        },
        uid: tl_remote::server::own_uid(),
        notify: Box::new(move |state: EngineLinkState| {
            let _ = states_tx.try_send(state);
        }),
        deadline: engine_link::CONNECT_DEADLINE,
        retry: engine_link::FALLBACK_RETRY,
    };
    let link = EngineLink::open(env)?;
    let weak = Arc::downgrade(&link);
    let api = EngineLinkApi {
        states,
        switch: Arc::new(move || {
            if let Some(link) = weak.upgrade() {
                link.switch_to_service();
            }
        }),
    };
    Ok((WindowEngine::Link(link), Some(api)))
}

/// The exit steps after the UI stops (ADR-010 Close behaviour, ADR-004 Shutdown): settings,
/// then the window's own engine within 1500 ms, then `before_exit`, then the log file. Exits
/// with `code` without running destructors, so a stuck engine thread cannot hold the process
/// open.
fn finish(
    engine: WindowEngine,
    store: &Store,
    log: Option<logging::LogGuard>,
    code: i32,
    before_exit: impl FnOnce(),
) -> ! {
    // Pending changes are written before anything else on exit (ADR-011).
    store.flush();
    engine.shutdown();
    // Runs while the log writer is still alive, so its lines reach the log file.
    before_exit();
    // The non-blocking writer loses lines unless its guard is dropped before `exit`.
    drop(log);
    std::process::exit(code)
}

/// The main tray icon pixels, pre-rendered from `assets/icons/tasklens.svg` (R-161 item 4).
fn main_tray_icon() -> tl_tray::MainIcon {
    tl_tray::MainIcon {
        argb_22: include_bytes!("../../../assets/icons/tray-22.argb").to_vec(),
        argb_44: include_bytes!("../../../assets/icons/tray-44.argb").to_vec(),
    }
}

/// Starts the tray thread. Returns the UI end for `UiLaunch`, or `None` when the thread did not
/// start, so no tray entry point is enabled.
fn start_tray() -> Option<tl_model::tray::TrayUiEnd> {
    let (tray_ui, tray_service) = tl_model::tray::tray_channels();
    match tl_tray::spawn(tray_service, main_tray_icon()) {
        Ok(_join) => Some(tray_ui),
        Err(e) => {
            warn!(target: "tasklens::tray", error = %e, "tray thread did not start");
            None
        }
    }
}

/// The engine mode for the flags. `--force-full-memory-mode` only has an effect together with
/// `--low-memory` (D-015): the rescue window stays, the engine runs in full mode.
fn engine_mode(low_memory: bool, force_full: bool) -> EngineMode {
    if low_memory && !force_full {
        EngineMode::LowMemory
    } else {
        EngineMode::Full
    }
}

/// True unless this process is itself the relaunch: one automatic relaunch (ADR-015).
fn should_relaunch(relaunched: Option<OsString>) -> bool {
    relaunched.is_none_or(|v| v != "1")
}

/// The arguments for the relaunch: `args` without argv[0] and without `--start-minimized`,
/// so the restarted window is shown.
fn relaunch_args(args: &[OsString]) -> Vec<OsString> {
    args.iter()
        .skip(1)
        .filter(|a| *a != "--start-minimized")
        .cloned()
        .collect()
}

/// The relaunch command: `exe` itself, never a shell, with `TASKLENS_RELAUNCHED=1`.
fn relaunch_command(exe: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(exe);
    command.args(relaunch_args(args)).env(RELAUNCHED_VAR, "1");
    command
}

/// Starts a new tasklens after device loss and does not wait for it. The bus name is released
/// first, so the new process becomes the primary instead of forwarding to this one.
/// Arguments are not logged.
fn relaunch(bus: Option<zbus::blocking::Connection>) {
    if let Some(conn) = bus
        && let Err(e) = conn.close()
    {
        warn!(target: "tasklens::ui", "could not close the session bus before relaunch: {e}");
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            error!(target: "tasklens::ui", "relaunch failed: no executable path: {e}");
            return;
        }
    };
    let args: Vec<OsString> = std::env::args_os().collect();
    match relaunch_command(&exe, &args).spawn() {
        Ok(child) => {
            info!(target: "tasklens::ui", pid = child.id(), "relaunched after device loss")
        }
        Err(e) => error!(target: "tasklens::ui", "relaunch failed: {e}"),
    }
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

/// True when `WAYLAND_DISPLAY` or `DISPLAY` is set and not empty.
fn has_display(wayland: Option<OsString>, x11: Option<OsString>) -> bool {
    [wayland, x11].iter().flatten().any(|v| !v.is_empty())
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

    #[test]
    fn main_tray_icon_sizes() {
        let icon = main_tray_icon();
        assert_eq!(icon.argb_22.len(), 1_936);
        assert_eq!(icon.argb_44.len(), 7_744);
    }

    /// ADR-018 Rescue mode: the rescue window writes no history, also with
    /// `--force-full-memory-mode`, so it needs no engine lock.
    #[test]
    fn rescue_engine_writes_no_history() {
        let paths = paths::Paths::from_vars(|name| match name {
            "HOME" => Some(OsString::from("/home/u")),
            _ => None,
        })
        .expect("paths");
        let rescue = engine_config(&paths, true, false);
        assert_eq!(rescue.mode, EngineMode::LowMemory);
        assert_eq!(rescue.history_dir, None);
        let forced = engine_config(&paths, true, true);
        assert_eq!(forced.mode, EngineMode::Full);
        assert_eq!(forced.history_dir, None);
        let main = engine_config(&paths, false, false);
        assert_eq!(main.history_dir, Some(paths.history_dir()));
    }

    #[test]
    fn engine_mode_choice() {
        assert_eq!(engine_mode(true, false), EngineMode::LowMemory);
        assert_eq!(engine_mode(true, true), EngineMode::Full);
        assert_eq!(engine_mode(false, false), EngineMode::Full);
        assert_eq!(engine_mode(false, true), EngineMode::Full);
    }

    #[test]
    fn exit_notice_kinds() {
        assert_eq!(exit_notice(&UiExit::Quit), None);
        assert_eq!(
            exit_notice(&UiExit::RendererFailed("no adapter".to_owned())),
            Some(notify::Fatal::RendererFailed)
        );
        assert_eq!(
            exit_notice(&UiExit::DeviceLost),
            Some(notify::Fatal::DeviceLost)
        );
    }

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
    fn display_needs_wayland_or_x11() {
        assert!(has_display(os("wayland-0"), None));
        assert!(has_display(None, os(":0")));
        assert!(has_display(os(""), os(":0")));
        assert!(!has_display(None, None));
        assert!(!has_display(os(""), os("")));
        assert!(!has_display(None, os("")));
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
    fn relaunch_drops_start_minimized() {
        let args: Vec<OsString> = [
            "/usr/bin/tasklens",
            "--start-minimized",
            "--tab",
            "services",
        ]
        .map(OsString::from)
        .into();
        assert_eq!(
            relaunch_args(&args),
            ["--tab", "services"].map(OsString::from).to_vec()
        );
        assert!(relaunch_args(&args[..1]).is_empty());
        assert!(relaunch_args(&[]).is_empty());
    }

    #[test]
    fn relaunch_only_once() {
        assert!(should_relaunch(None));
        assert!(!should_relaunch(os("1")));
        assert!(should_relaunch(os("0")));
    }

    #[test]
    fn relaunch_command_sets_marker() {
        let args: Vec<OsString> = ["tasklens", "--start-minimized", "--debug"]
            .map(OsString::from)
            .into();
        let command = relaunch_command(Path::new("/usr/bin/tasklens"), &args);
        assert_eq!(command.get_program(), "/usr/bin/tasklens");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--debug"]);
        let envs: Vec<_> = command.get_envs().collect();
        assert_eq!(envs, [(RELAUNCHED_VAR.as_ref(), Some("1".as_ref()))]);
    }

    #[test]
    fn relaunched_process_sees_marker() {
        // `env` prints its environment, standing in for the relaunched tasklens.
        let output = relaunch_command(Path::new("/usr/bin/env"), &[])
            .env_remove("TASKLENS_DEBUG")
            .output()
            .expect("run env");
        let printed = String::from_utf8_lossy(&output.stdout);
        let marker = printed
            .lines()
            .find_map(|l| l.strip_prefix("TASKLENS_RELAUNCHED="));
        assert_eq!(marker, Some("1"));
        assert!(!should_relaunch(marker.map(OsString::from)));
    }

    #[test]
    fn start_tab_maps_every_tab() {
        assert_eq!(start_tab(TabArg::Processes), StartTab::Processes);
        assert_eq!(start_tab(TabArg::Performance), StartTab::Performance);
        assert_eq!(start_tab(TabArg::Information), StartTab::Information);
    }
}
