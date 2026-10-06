//! `TasklensUi::init`: the one entry point tl-app calls inside `Application::run` (ADR-009
//! "Entry point and the tl-app hand-off", UI-SHL-110 step 8, R-171).
//!
//! `UiInit` carries everything tl-app parsed or built: settings, state, the persist callbacks,
//! the `AppRequest` receiver, queued toasts and `AppEnv`. `init` installs the fonts, the kit,
//! the theme, the strings, the engine, hub and demand Globals, then starts the `AppRequest` loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::{App, AppContext, AsyncApp, Global, SharedString};
use tl_model::client::EngineClient;
use tl_model::command::AppRequest;
use tl_model::settings::{Settings, UiState};
use tracing::{debug, warn};

use crate::components::{checkbox, kv_grid};
use crate::demand::{DemandAggregator, GlobalDemand};
use crate::hub::{GlobalHub, SnapshotHub};
use crate::theme::ThemeDef;
use crate::{i18n, text, theme};

/// What the theme store asks tl-app to write (UI-FND-042). E17-008 moves this enum to
/// `theme/user_store.rs` and leaves a `pub use` here.
#[derive(Clone, Debug)]
pub enum PersistTheme {
    /// Write these user themes to themes.json.
    Save(Vec<ThemeDef>),
    /// Rename a themes.json that does not parse, with this suffix time.
    RenameCorrupt { unix_seconds: u64 },
}

/// Everything tl-app hands to the UI at start-up.
pub struct UiInit {
    /// Parsed settings.toml.
    pub settings: Settings,
    /// Parsed state.toml.
    pub state: UiState,
    /// Raw `$XDG_CONFIG_HOME/tasklens/themes.json` text, `None` when the file is missing.
    pub themes_json: Option<Arc<str>>,
    /// ADR-011 persist callback. tl-app debounces it by 500 ms.
    pub persist: Box<dyn Fn(&Settings, &UiState)>,
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

/// Reads and writes the Run dialog history file. tl-app builds it on `write_atomic`.
pub trait RunHistoryStore {
    fn load(&self) -> Vec<RunHistoryEntry>;
    fn save(&self, entries: &[RunHistoryEntry]) -> Result<(), String>;
}

/// The KDE global shortcut for showing the main window.
pub struct GlobalShortcutApi {
    pub get: Arc<dyn Fn() -> Option<String>>,
    pub set: Arc<dyn Fn(Option<&str>) -> Result<(), String>>,
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
    pub write_settings_now: Arc<dyn Fn(&Settings) -> Result<(), String>>,
    /// `(enabled, minimized)` (R-236).
    pub set_autostart: Arc<dyn Fn(bool, bool) -> Result<(), String>>,
    /// `None` outside KDE.
    pub global_shortcut: Option<GlobalShortcutApi>,
    pub import_taskslinger: Arc<dyn Fn() -> Result<ImportSummary, String>>,
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
            import_taskslinger: Arc::new(|| Ok(ImportSummary::default())),
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
    pub persist: Option<Box<dyn Fn(&Settings, &UiState)>>,
    /// Held here until E17-008 defines `ThemePersist`.
    pub persist_themes: Option<Box<dyn Fn(PersistTheme)>>,
    /// Shown by the shell after the first frame (UI-SHL-110 step 11).
    pub startup_toasts: Vec<(ToastKindTag, SharedString)>,
}

impl Global for BootSettings {}

/// Handles each `AppRequest` on the main thread. E20 replaces the default (UI-SHL-112).
pub struct AppRequestHandler(pub Box<dyn Fn(AppRequest, &mut App)>);

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
            startup_toasts,
            env,
        } = init;

        // 1. Fonts before any window. A failed face is logged and the UI uses the system font.
        let fonts = text::fonts::load_bundled_fonts(cx);
        if !fonts.ui_ok {
            warn!(target: "tasklens::ui", family = text::fonts::effective_ui_family(&fonts), "using fallback UI font");
        }

        // 2. Kit.
        gpui_kit::init(cx);

        // 3. Asset source: `App` has no setter in gpui-pre 0.3.7, so `tl_ui::run` passes
        // `TlAssets` to `Application::with_assets` (E16-012).

        // 4. Theme. E17-008 replaces `user` with the themes parsed from `themes_json`.
        if let Some(json) = &themes_json
            && let Err(err) = serde_json::from_str::<serde_json::Value>(json)
        {
            warn!(target: "tasklens::ui", error = %err, "themes.json does not parse, ignored");
        }
        let user: Vec<ThemeDef> = Vec::new();
        theme::store::init(cx, &settings.theme, &settings.appearance, user);

        // 5. Strings.
        i18n::init_strings(cx, &settings.general.language);

        // 6. Engine.
        cx.set_global(EngineGlobal(client.clone()));

        // 7. Hub and demand.
        let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        cx.set_global(GlobalHub(hub));
        cx.set_global(GlobalDemand(demand));

        // 8. Boot settings and the tl-app environment.
        cx.set_global(BootSettings {
            settings,
            state,
            persist: Some(persist),
            persist_themes: Some(persist_themes),
            startup_toasts,
        });
        cx.set_global(env);

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
        checkbox::bind_keys_components(cx);
        kv_grid::init(cx);

        // ActionTracker

        // open main window
    }
}
