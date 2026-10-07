//! `TasklensUi::init`: the one entry point tl-app calls inside `Application::run` (ADR-009
//! "Entry point and the tl-app hand-off", UI-SHL-110 step 8, R-171).
//!
//! `UiInit` carries everything tl-app parsed or built: settings, state, the persist callbacks,
//! the `AppRequest` receiver, queued toasts and `AppEnv`. `init` installs the fonts, the kit,
//! the theme, the strings, the engine, hub and demand Globals, then starts the `AppRequest` loop.
//!
//! `run` owns the GPUI application loop, because tl-app cannot depend on gpui-kit (ADR-001,
//! E16-012). tl-app builds a `UiLaunch`, calls `run` and acts on the returned `UiExit`.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::{App, AppContext, AsyncApp, Global, QuitMode, SharedString};
use tl_model::client::EngineClient;
use tl_model::command::AppRequest;
use tl_model::settings::{Settings, StartTab, UiState};
use tracing::{debug, error, info, warn};

use crate::components::icons::TlAssets;
use crate::components::{kv_grid, toast};
use crate::demand::{DemandAggregator, GlobalDemand};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged, SnapshotHub};
use crate::theme::user_store::ThemePersist;
use crate::{actions, i18n, settings_global, text, theme};

pub use crate::theme::user_store::PersistTheme;

/// The ADR-011 persist callback.
pub type PersistFn = Box<dyn Fn(&Settings, &UiState)>;
/// Writes settings.toml at once. The error is shown to the user.
pub type WriteSettingsFn = Arc<dyn Fn(&Settings) -> Result<(), String>>;
/// Sets or clears the global shortcut. The error is shown to the user.
pub type SetShortcutFn = Arc<dyn Fn(Option<&str>) -> Result<(), String>>;
/// Reads a TaskSlinger `UserSettings.ini` and imports it onto the current settings and state.
/// The error is shown to the user.
pub type ImportTaskslingerFn =
    Arc<dyn Fn(&Path, &Settings, &UiState) -> Result<ImportedSettings, String>>;
/// Handles one `AppRequest` on the main thread.
pub type AppRequestFn = Box<dyn Fn(AppRequest, &mut App)>;

/// Everything tl-app hands to the UI at start-up.
pub struct UiInit {
    /// Parsed settings.toml.
    pub settings: Settings,
    /// Parsed state.toml.
    pub state: UiState,
    /// Raw `$XDG_CONFIG_HOME/tasklens/themes.json` text, `None` when the file is missing.
    pub themes_json: Option<Arc<str>>,
    /// ADR-011 persist callback. tl-app debounces it by 500 ms.
    pub persist: PersistFn,
    /// UI-FND-042 theme file writes.
    pub persist_themes: Box<dyn Fn(PersistTheme)>,
    /// From tl-app's single-instance handler, `bounded(4)` (UI-SHL-112).
    pub app_requests: async_channel::Receiver<AppRequest>,
    /// Settings load warnings queued by tl-app (UI-SHL-110 step 5).
    pub startup_toasts: Vec<(ToastKindTag, SharedString)>,
    /// Everything only tl-app has (R-171).
    pub env: AppEnv,
}

/// Build facts embedded by tl-app.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuildInfo {
    pub version: &'static str,
    pub git_hash: &'static str,
    pub build_date: &'static str,
    pub gpui_rev: &'static str,
    /// cargo-about output.
    pub licenses_json: &'static str,
}

/// A crash report the user has not seen yet (UI-SHL-093).
#[derive(Clone, Debug)]
pub struct CrashReport {
    pub path: PathBuf,
    pub text: Arc<str>,
    pub at: SystemTime,
}

/// One GPU adapter from the wgpu probe in tl-app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterInfo {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    /// `None` when the backend does not report it (R-237).
    pub pci_bus_id: Option<String>,
}

/// One Run dialog history entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunHistoryEntry {
    pub command: String,
    /// RFC 3339.
    pub executed_at: String,
    pub elevated: bool,
}

/// What a TaskSlinger import brought over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub settings: u32,
    pub themes: u32,
}

/// A TaskSlinger import: the full settings and state with the imported values applied.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedSettings {
    pub settings: Settings,
    pub state: UiState,
    pub summary: ImportSummary,
}

/// Reads and writes the Run dialog history file. tl-app builds it on `write_atomic`.
pub trait RunHistoryStore {
    fn load(&self) -> Vec<RunHistoryEntry>;
    fn save(&self, entries: &[RunHistoryEntry]) -> Result<(), String>;
}

/// The KDE global shortcut for showing the main window.
pub struct GlobalShortcutApi {
    pub get: Arc<dyn Fn() -> Option<String>>,
    pub set: SetShortcutFn,
}

/// Everything only tl-app has (R-171). Every closure runs on the main thread; none is `Send`.
pub struct AppEnv {
    /// `--debug` or `TASKLENS_DEBUG` (table c).
    pub debug: bool,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub build: BuildInfo,
    /// `None` when the `.seen` marker exists (UI-SHL-093).
    pub pending_crash: Option<CrashReport>,
    pub mark_crash_seen: Arc<dyn Fn()>,
    /// Synchronous, for Reset settings.
    pub write_settings_now: WriteSettingsFn,
    /// `(enabled, minimized)` (R-236).
    pub set_autostart: Arc<dyn Fn(bool, bool) -> Result<(), String>>,
    /// `None` outside KDE.
    pub global_shortcut: Option<GlobalShortcutApi>,
    /// Writes nothing. The caller must apply the result through `TlSettings::update`, so the UI
    /// and the settings store stay in sync.
    pub import_taskslinger: ImportTaskslingerFn,
    /// The wgpu probe runs in tl-app.
    pub list_adapters: Arc<dyn Fn() -> Vec<AdapterInfo>>,
    /// `None` in tests.
    pub run_history: Option<Box<dyn RunHistoryStore>>,
}

impl Global for AppEnv {}

impl AppEnv {
    /// Empty closures, `debug = false`, empty paths and no run history.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn test() -> Self {
        Self {
            debug: false,
            config_dir: PathBuf::new(),
            state_dir: PathBuf::new(),
            logs_dir: PathBuf::new(),
            build: BuildInfo::default(),
            pending_crash: None,
            mark_crash_seen: Arc::new(|| {}),
            write_settings_now: Arc::new(|_| Ok(())),
            set_autostart: Arc::new(|_, _| Ok(())),
            global_shortcut: None,
            import_taskslinger: Arc::new(|_, settings, state| {
                Ok(ImportedSettings {
                    settings: settings.clone(),
                    state: state.clone(),
                    summary: ImportSummary::default(),
                })
            }),
            list_adapters: Arc::new(Vec::new),
            run_history: None,
        }
    }
}

/// The kind of a queued start-up toast. Maps 1:1 to `ToastKind` (E16-006).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKindTag {
    Info,
    Success,
    Warning,
    Error,
}

/// The settings, state and callbacks from `UiInit`. E20-A2 converts it into `TlSettings`.
pub struct BootSettings {
    pub settings: Settings,
    pub state: UiState,
    pub persist: Option<PersistFn>,
    /// Shown by the shell after the first frame (UI-SHL-110 step 11).
    pub startup_toasts: Vec<(ToastKindTag, SharedString)>,
}

impl Global for BootSettings {}

/// Handles each `AppRequest` on the main thread. E20 replaces the default (UI-SHL-112).
pub struct AppRequestHandler(pub AppRequestFn);

impl Global for AppRequestHandler {}

/// The engine client, for code that has no hub at hand.
pub struct EngineGlobal(pub Arc<dyn EngineClient>);

impl Global for EngineGlobal {}

/// The UI entry point.
pub struct TasklensUi;

impl TasklensUi {
    /// Sets up the UI in UI-SHL-110 step 8 order. Opens no window and creates no page entity.
    pub fn init(cx: &mut App, client: Arc<dyn EngineClient>, init: UiInit) {
        let UiInit {
            settings,
            state,
            themes_json,
            persist,
            persist_themes,
            app_requests,
            mut startup_toasts,
            env,
        } = init;

        // 1. Fonts before any window. A failed face is logged and the UI uses the system font.
        let fonts = text::fonts::load_bundled_fonts(cx);
        if !fonts.ui_ok {
            warn!(target: "tasklens::ui", family = text::fonts::effective_ui_family(&fonts), "using fallback UI font");
        }
        perf_step("fonts");

        // 2. Kit.
        gpui_kit::init(cx);
        perf_step("kit");

        // 3. Asset source: `App` has no setter in gpui-pre 0.3.7, so `tl_ui::run` passes
        // `TlAssets` to `Application::with_assets` (E16-012).

        // 4. Theme. A themes.json that does not parse is renamed by tl-app and reported in a
        // Warning toast after the first frame.
        cx.set_global(ThemePersist(persist_themes));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Some(text) = theme::store::init_with_file(
            cx,
            &settings.theme,
            &settings.appearance,
            themes_json.as_deref(),
            now,
        ) {
            startup_toasts.push((ToastKindTag::Warning, text));
        }

        // 5. Strings.
        i18n::init_strings(cx, &settings.general.language);

        // 6. Engine.
        cx.set_global(EngineGlobal(client.clone()));

        // 7. Hub and demand.
        let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        cx.set_global(GlobalHub(hub));
        cx.set_global(GlobalDemand(demand));
        crate::window::registry::GlobalRegistry::init(cx);

        // 8. Boot settings and the tl-app environment.
        cx.set_global(BootSettings {
            settings,
            state,
            persist: Some(persist),
            startup_toasts,
        });
        settings_global::TlSettings::init(cx);
        cx.set_global(env);

        // Follow system light and dark (E17-007). Follow-system switches show no toast.
        let hub = cx.global::<GlobalHub>().0.clone();
        cx.subscribe(&hub, |hub, ev: &SnapshotChanged, cx| {
            if !ev.mask.contains(ComponentMask::SYSTEM) {
                return;
            }
            let scheme = hub.read(cx).current().system.color_scheme;
            if scheme == cx.global::<theme::ThemeStore>().system_scheme {
                return;
            }
            let theme = settings_global::TlSettings::settings(cx).theme.clone();
            if let Some(Err(e)) = theme::store::on_system_scheme(scheme, &theme, cx) {
                warn!(target: "tasklens::ui", ?e, "follow-system theme not applied");
            }
        })
        .detach();

        // 9. AppRequest loop. E20 replaces the handler Global.
        cx.set_global(AppRequestHandler(Box::new(|req, _| {
            debug!(target: "tasklens::ui", ?req, "app request");
        })));
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(req) = app_requests.recv().await {
                cx.update_global::<AppRequestHandler, _>(|h, cx| (h.0)(req, cx));
            }
        })
        .detach();

        // bind_keys_*
        actions::bind_keys_global(cx);
        crate::table::actions::bind_keys_table(cx);
        crate::pages::performance::actions::bind_keys_performance(cx);
        crate::windows::actions::bind_keys_process_windows(cx);
        actions::register_global_handlers(cx);
        crate::window::menubar::init(cx);
        kv_grid::init(cx);
        // Before any window, so every Root mounts the toast layer.
        toast::init(cx);

        // ActionTracker
        crate::action_tracker::install(hub.read(cx).client().clone(), cx);

        // open hooks
        crate::window::main::init(cx);
        crate::window::share_theme::init(cx);
        crate::window::collector_health::init(cx);
        crate::window::shortcuts::init(cx);
        crate::window::about::init(cx);
        #[cfg(feature = "bench")]
        install_gallery_hook(cx);
        perf_step("ui_init");
    }
}

/// The `--gallery` window (E01-013): "tasklens Gallery". Closing it ends the process.
#[cfg(feature = "bench")]
fn install_gallery_hook(cx: &mut App) {
    cx.default_global::<OpenHooks>().gallery = Some(Box::new(|cx| {
        let handle = crate::components::gallery::open_gallery(cx);
        handle
            .update(cx, |_, window, _| {
                window.set_window_title("tasklens Gallery")
            })
            .ok();
        let id = handle.window_id();
        cx.on_window_closed(move |cx, closed| {
            if closed == id {
                request_exit(cx, UiExit::Quit);
            }
        })
        .detach();
    }));
}

/// The `main` start and the end of the last step, for [`perf_step`].
static PERF_CLOCK: Mutex<Option<(Instant, Instant)>> = Mutex::new(None);

/// Starts the `tasklens::perf` start-up clock. tl-app calls it first in `main`.
pub fn perf_origin(origin: Instant) {
    *PERF_CLOCK.lock().unwrap_or_else(PoisonError::into_inner) = Some((origin, origin));
}

/// Logs one start-up step under `tasklens::perf` (ADR-014): `ms` is the time since the previous
/// step ended, `at_ms` the time since `main` began. Does nothing before [`perf_origin`].
pub fn perf_step(step: &'static str) {
    let now = Instant::now();
    let mut clock = PERF_CLOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let Some((origin, last)) = clock.as_mut() else {
        return;
    };
    let ms = |d: std::time::Duration| (d.as_secs_f64() * 10_000.).round() / 10.;
    info!(target: "tasklens::perf", step, ms = ms(now - *last), at_ms = ms(now - *origin), "startup step");
    *last = now;
}

/// Everything `run` needs from tl-app (UI-SHL-110 step 8).
pub struct UiLaunch {
    pub client: Arc<dyn EngineClient>,
    pub init: UiInit,
    pub open: OpenMode,
    pub start: StartOptions,
}

/// Which window opens first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    Main,
    Gallery,
    LowMemory,
}

/// Command-line start options parsed by tl-app.
#[derive(Clone, Debug, Default)]
pub struct StartOptions {
    /// `--tab`.
    pub tab: Option<StartTab>,
    /// `--select-pid`.
    pub select_pid: Option<u32>,
    /// `--start-minimized`.
    pub start_minimized: bool,
    /// Shift held at start (UI-SHL-110 step 6). tl-app reads it on X11; always false on Wayland.
    pub shift_held: bool,
    /// UI-SHL-093.
    pub pending_crash_report: Option<PathBuf>,
}

impl Global for StartOptions {}

/// Why the UI stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiExit {
    Quit,
    /// The renderer failed before the first window opened. Holds the panic message.
    RendererFailed(String),
    DeviceLost,
}

/// The exit reason. The first `request_exit` wins.
#[derive(Debug, Default)]
pub struct UiExitSlot(pub Option<UiExit>);

impl Global for UiExitSlot {}

/// The cell `run` reads after the App is gone.
struct ExitCell(Rc<Cell<Option<UiExit>>>);

impl Global for ExitCell {}

/// Counts the `cx.quit` calls `request_exit` makes, for the exit tests.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Default)]
pub struct QuitCalls(pub u32);

#[cfg(any(test, feature = "test-hooks"))]
impl Global for QuitCalls {}

/// Stores `exit` unless an earlier exit is stored, then quits the app.
///
/// The first request also quits again from a spawned task. A request made before the event
/// loop starts (an open hook, `open_first`) is otherwise lost, because calloop's
/// `EventLoop::run` clears the stop flag when it starts. The task runs once the loop runs.
pub fn request_exit(cx: &mut App, exit: UiExit) {
    let slot = cx.default_global::<UiExitSlot>();
    let first = slot.0.is_none();
    if first {
        slot.0 = Some(exit.clone());
        if let Some(cell) = cx.try_global::<ExitCell>() {
            cell.0.set(Some(exit));
        }
    }
    quit(cx);
    if first {
        cx.spawn(async move |cx| cx.update(quit)).detach();
    }
}

fn quit(cx: &mut App) {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        cx.default_global::<QuitCalls>().0 += 1;
    }
    cx.quit();
}

type OpenHook = Box<dyn Fn(&mut App)>;

/// Opens the first window for each `OpenMode`. Owners install their hook at the
/// `// open hooks` line in `TasklensUi::init`, through `cx.default_global::<OpenHooks>()`.
#[derive(Default)]
pub struct OpenHooks {
    pub main: Option<OpenHook>,
    pub gallery: Option<OpenHook>,
    pub low_memory: Option<OpenHook>,
}

impl Global for OpenHooks {}

/// Runs the GPUI application on the calling thread (the main thread).
/// Returns after the application quits.
pub fn run(launch: UiLaunch) -> UiExit {
    let exit = Rc::new(Cell::new(None));
    let opened = Rc::new(Cell::new(false));
    let result = {
        let exit = Rc::clone(&exit);
        let opened = Rc::clone(&opened);
        // The platform is built inside the guard too: a failed display connection panics there.
        panic::catch_unwind(AssertUnwindSafe(move || {
            let app = gpui_kit::application()
                .with_assets(TlAssets)
                .with_quit_mode(QuitMode::Explicit);
            perf_step("platform");
            app.run(move |cx| {
                perf_step("app_run");
                start(cx, launch, exit);
                opened.set(true);
            });
        }))
    };
    if let Err(payload) = result {
        if opened.get() {
            panic::resume_unwind(payload);
        }
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown renderer error".to_owned());
        return UiExit::RendererFailed(message);
    }
    // U-2: if `Application::run` does not return on Linux, `request_exit` and this Quit path
    // call `std::process::exit` after `UiInit`'s persist callback instead (E33-011).
    exit.take().unwrap_or(UiExit::Quit)
}

/// The body of the `Application::run` closure.
fn start(cx: &mut App, launch: UiLaunch, exit: Rc<Cell<Option<UiExit>>>) {
    let UiLaunch {
        client,
        init,
        open,
        start,
    } = launch;
    cx.set_global(start);
    cx.set_global(UiExitSlot(None));
    cx.set_global(ExitCell(exit));
    cx.set_global(OpenHooks::default());
    // E32-013: low-memory init
    if open == OpenMode::LowMemory {
        crate::windows::low_memory::init_low_memory(cx, client, init);
    } else {
        TasklensUi::init(cx, client, init);
    }
    open_first(cx, open);
    perf_step("window_open");
}

/// Calls the hook for `mode`. A missing hook logs an error and quits.
pub(crate) fn open_first(cx: &mut App, mode: OpenMode) {
    let hook = cx.try_global::<OpenHooks>().is_some().then(|| {
        let hooks = cx.global_mut::<OpenHooks>();
        match mode {
            OpenMode::Main => hooks.main.take(),
            OpenMode::Gallery => hooks.gallery.take(),
            OpenMode::LowMemory => hooks.low_memory.take(),
        }
    });
    match hook.flatten() {
        Some(hook) => hook(cx),
        None => {
            error!(target: "tasklens::ui", ?mode, "no window registered");
            request_exit(cx, UiExit::Quit);
        }
    }
}

/// `open_first` for integration tests; `run` blocks, so tests never call it.
#[cfg(any(test, feature = "test-hooks"))]
pub fn open_first_for_test(cx: &mut App, mode: OpenMode) {
    open_first(cx, mode);
}

/// The `run` closure body for integration tests. Returns the cell `run` would read.
#[cfg(any(test, feature = "test-hooks"))]
pub fn start_for_test(cx: &mut App, launch: UiLaunch) -> Rc<Cell<Option<UiExit>>> {
    let exit = Rc::new(Cell::new(None));
    start(cx, launch, Rc::clone(&exit));
    exit
}
