//! Secondary windows (properties, dialogs, setup); their epics add `pub mod` lines.

pub use tl_ui_procs::windows::actions;
pub use tl_ui_procs::windows::affinity;
pub use tl_ui_procs::windows::app_props;
pub mod confirm_startup_delete;
pub mod connection_properties;
pub use tl_ui_perf_core::detached_graph;
pub use tl_ui_procs::windows::disk_in_use;
pub use tl_ui_procs::windows::frame;
pub use tl_ui_procs::windows::handle_search;
pub mod low_memory;
pub use tl_ui_perf::mini;
pub use tl_ui_procs::windows::notices;
pub use tl_ui_procs::windows::priority;
pub use tl_ui_procs::windows::props;
pub mod run;
pub mod run_history;
pub mod service_properties;
pub use tl_ui_procs::windows::signal;
pub mod smart;
pub mod startup_properties;
