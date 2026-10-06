//! tl-ui: GPUI views, chart element, process table and theme.

pub mod actions;
pub mod anim;
pub mod app;
pub mod chart;
pub mod components;
pub mod demand;
pub mod hub;
pub mod i18n;
pub mod pages;
pub mod settings_global;
pub mod table;
pub mod text;
pub mod text_cache;
pub mod theme;
pub mod window;
pub mod windows;

#[cfg(feature = "bench")]
pub mod bench;
#[cfg(any(test, feature = "test-hooks"))]
pub mod testing;

pub use app::{
    AdapterInfo, AppEnv, BuildInfo, CrashReport, GlobalShortcutApi, ImportSummary, PersistTheme,
    RunHistoryEntry, RunHistoryStore, TasklensUi, UiInit,
};
pub use app::{OpenHooks, OpenMode, StartOptions, UiExit, UiLaunch, request_exit, run};
