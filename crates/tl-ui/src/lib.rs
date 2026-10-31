//! tl-ui: GPUI views, chart element, process table and theme.

pub mod action_tracker;
pub mod actions;
pub mod anim;
pub mod app;
pub mod chart;
pub mod components;
pub mod demand;
pub mod engine_link;
pub mod focus;
pub mod helper_flow;
pub mod hub;
pub mod i18n;
pub mod net_totals;
pub mod pages;
pub mod request_ends;
pub mod settings_global;
pub mod table;
pub mod text;
pub mod text_cache;
pub mod theme;
pub mod tray_bridge;
pub mod util;
pub mod window;
pub mod windows;

#[cfg(feature = "bench")]
pub mod bench;
#[cfg(any(test, feature = "test-hooks"))]
pub mod testing;

pub use app::{
    AdapterInfo, AppEnv, BuildInfo, CrashReport, GlobalShortcutApi, ImportSummary,
    ImportedSettings, PersistTheme, RunHistoryEntry, RunHistoryStore, TasklensUi, UiInit,
};
pub use app::{OpenHooks, OpenMode, StartOptions, UiExit, UiLaunch, request_exit, run};
