//! The data and Global types tl-app hands to the UI (R-171, UI-SHL-110 step 8): `UiInit`,
//! `AppEnv`, `BootSettings`, `StartOptions`, the engine Global and the callback types.
//!
//! They live here, below the shell, so foundation modules can use them without reaching up into
//! `app.rs` (ADR-020, tasklens-zljr5.14). `app.rs` re-exports every item, so `tl_ui::app::X`
//! keeps working.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::{App, Global, SharedString};
use tl_model::client::EngineClient;
use tl_model::command::AppRequest;
use tl_ui_settings::settings::{Settings, StartTab, UiState};

use crate::theme::user_store::PersistTheme;

/// The Wayland app id and X11 class; matches the desktop file.
pub const APP_ID: &str = "io.github.oysteinkrog.Tasklens";

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
/// Reads a picked TaskSlinger `themes.json` and returns its text. tl-ui does no file I/O
/// (ADR-011). The text is capped at `MAX_IMPORT_BYTES`. The error is shown to the user.
pub type ReadTsThemesFn = Arc<dyn Fn(&Path) -> Result<String, String>>;
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
    /// Reads a picked TaskSlinger `themes.json` for "Import from TaskSlinger…".
    pub read_ts_themes: ReadTsThemesFn,
    /// Starts the wgpu probe in tl-app and returns at once. tl-app runs the probe on its own
    /// thread and sends the adapters on the channel; tl-ui waits up to 2 s (UI-SHL-040a).
    pub list_adapters: Arc<dyn Fn() -> async_channel::Receiver<Vec<AdapterInfo>>>,
    /// `None` in tests.
    pub run_history: Option<Box<dyn RunHistoryStore>>,
    /// Where the engine runs and the switch to the engine service (ADR-018). `None` for the
    /// rescue window and in tests.
    pub engine_link: Option<crate::engine_link::EngineLinkApi>,
    /// One engine client per enabled `[[hosts]]` entry (ADR-018). `None` for the rescue window
    /// and in tests.
    pub hosts: Option<crate::hosts_api::HostsApi>,
    /// Makes the next settings write first copy `settings.toml` to
    /// `settings.toml.pre-layouts-v2`, unless that copy exists. A GPU page calls it before it
    /// writes its converted layout (ADR-019 phase 3 "Report and backup"). tl-app wires it to
    /// `Store::keep_pre_layouts_copy`.
    pub keep_pre_layouts_copy: Arc<dyn Fn()>,
}

impl Global for AppEnv {}

impl AppEnv {
    /// Empty closures, `debug = false`, empty paths and no run history.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
            read_ts_themes: Arc::new(|_| Ok(String::new())),
            list_adapters: Arc::new(|| {
                let (tx, rx) = async_channel::bounded(1);
                let _ = tx.try_send(Vec::new());
                rx
            }),
            run_history: None,
            engine_link: None,
            hosts: None,
            keep_pre_layouts_copy: Arc::new(|| {}),
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

/// The local host's engine client, set once by `TasklensUi::init`. Read only through
/// [`crate::host::HostContext`].
pub struct EngineGlobal(pub Arc<dyn EngineClient>);

impl Global for EngineGlobal {}

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
    /// `--app-id`. `None` means `APP_ID`.
    pub app_id: Option<String>,
}

impl Global for StartOptions {}

/// The Wayland app_id for new windows: the `--app-id` value, or `APP_ID` when none was given.
pub fn window_app_id(cx: &App) -> String {
    cx.try_global::<StartOptions>()
        .and_then(|s| s.app_id.clone())
        .unwrap_or_else(|| APP_ID.to_owned())
}
