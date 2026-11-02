//! The Services, Connections and Startup pages (tl-ui re-exports them under `pages`).
pub mod connections;
pub mod services;
pub mod startup;

pub(crate) use tl_ui_perf_core::minigraph;
pub(crate) use tl_ui_procs::pages::processes;
