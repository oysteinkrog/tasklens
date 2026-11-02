//! tl-ui, the shell (ADR-020): the main window, app init, the Information page, the run,
//! low-memory and SMART windows, and the bench.
//!
//! The module re-exports below keep the old `tl_ui::x` paths for the shell's own code, tl-app
//! and the `ui` tests, and each one has a user (tasklens-zljr5.34). Items such as `AdapterInfo`
//! and `CrashReport` are at `tl_ui::app`. CS-J1 rewrites the users to direct crate paths.

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
pub use tl_ui_base::util;
pub use tl_ui_perf_core::tray_bridge;
pub use tl_ui_table::table;
pub use tl_ui_table::text_cache;
pub mod window;
pub mod windows;

#[cfg(feature = "bench")]
pub mod bench;

pub use app::{
    AppEnv, BuildInfo, ImportSummary, PersistTheme, RunHistoryEntry, RunHistoryStore, TasklensUi,
    UiInit,
};
pub use app::{OpenHooks, OpenMode, StartOptions, UiExit, UiLaunch, request_exit, run};
