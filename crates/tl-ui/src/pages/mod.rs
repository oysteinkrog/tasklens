//! Pages of the main window. The re-exports are the pages tl-app and the `ui` tests name through
//! the shell (ADR-020); everything else is at its crate path.
pub use tl_ui_procs::pages::processes;
pub use tl_ui_sys::pages::services;
pub use tl_ui_sys::pages::startup;
pub use tl_ui_table::table::sys_table;
pub mod information;
