//! Pages of the main window; the page epics add `pub mod` lines.
pub use tl_ui_sys::pages::connections;
pub mod information;
pub use crate::table::{sys_copy, sys_status, sys_table};
pub use tl_ui_perf::performance;
pub use tl_ui_perf_core::minigraph;
pub use tl_ui_procs::pages::processes;
pub use tl_ui_sys::pages::services;
pub use tl_ui_sys::pages::startup;
