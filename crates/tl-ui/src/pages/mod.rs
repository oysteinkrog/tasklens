//! Pages of the main window; the page epics add `pub mod` lines.
pub mod connections;
pub mod information;
pub use tl_ui_perf::performance;
pub use tl_ui_perf_core::minigraph;
pub use tl_ui_procs::pages::processes;
pub mod services;
pub mod startup;
pub use crate::table::{sys_copy, sys_status, sys_table};
