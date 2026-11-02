//! tl-ui: GPUI views, chart element, process table and theme.

pub use tl_ui_base::action_tracker;
pub use tl_ui_base::anim;
pub use tl_ui_kit::actions;
pub mod app;
pub use tl_ui_base::app_env;
pub use tl_ui_base::demand;
pub use tl_ui_base::engine_link;
pub use tl_ui_base::focus;
pub use tl_ui_base::host;
pub use tl_ui_base::host_settings;
pub use tl_ui_base::hosts_api;
pub use tl_ui_base::hosts_edit;
pub use tl_ui_base::hub;
pub use tl_ui_base::i18n;
pub use tl_ui_base::motion;
pub use tl_ui_chart::chart;
pub use tl_ui_chart::net_totals;
pub use tl_ui_kit::components;
pub use tl_ui_kit::helper_flow;
pub mod pages;
pub use tl_ui_base::registry;
pub use tl_ui_base::request_ends;
pub use tl_ui_base::selector;
pub use tl_ui_base::settings_global;
pub use tl_ui_base::shell;
pub use tl_ui_base::template;
pub use tl_ui_base::text;
pub use tl_ui_base::theme;
pub use tl_ui_base::toast_types;
pub use tl_ui_table::table;
pub use tl_ui_table::text_cache;
pub mod tray_bridge;
pub use tl_ui_base::util;
pub mod window;
pub mod windows;

#[cfg(feature = "bench")]
pub mod bench;

pub use app::{
    AdapterInfo, AppEnv, BuildInfo, CrashReport, GlobalShortcutApi, ImportSummary,
    ImportedSettings, PersistTheme, RunHistoryEntry, RunHistoryStore, TasklensUi, UiInit,
};
pub use app::{OpenHooks, OpenMode, StartOptions, UiExit, UiLaunch, request_exit, run};
