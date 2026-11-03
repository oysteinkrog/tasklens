//! tl-ui, the shell (ADR-020): the main window, app init, the Information page, the run,
//! low-memory and SMART windows, and the bench.
//!
//! The module re-exports below are the shell's public API on purpose: tl-app depends on tl-ui
//! only, and names these through it. ADR-020 lists them. Everything else is at its crate path.
//! Items such as `AdapterInfo` and `CrashReport` are at `tl_ui::app`.

pub mod app;
pub use tl_ui_base::demand;
pub use tl_ui_base::engine_link;
pub use tl_ui_base::host;
pub use tl_ui_base::host_settings;
pub use tl_ui_base::hosts_api;
pub use tl_ui_base::hub;
pub use tl_ui_base::i18n;
pub use tl_ui_chart::net_totals;
pub mod pages;
pub use tl_ui_base::settings_global;
pub use tl_ui_base::theme;
pub use tl_ui_table::table;
pub mod window;
pub mod windows;

#[cfg(feature = "bench")]
pub mod bench;

pub use app::{
    AppEnv, BuildInfo, ImportSummary, PersistTheme, RunHistoryEntry, RunHistoryStore, TasklensUi,
    UiInit,
};
pub use app::{OpenHooks, OpenMode, StartOptions, UiExit, UiLaunch, request_exit, run};
