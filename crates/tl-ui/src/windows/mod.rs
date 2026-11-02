//! Secondary windows (properties, dialogs, setup); their epics add `pub mod` lines.

pub use tl_ui_perf_core::detached_graph;
pub use tl_ui_procs::windows::actions;
pub use tl_ui_procs::windows::affinity;
pub use tl_ui_procs::windows::app_props;
pub use tl_ui_procs::windows::disk_in_use;
pub use tl_ui_procs::windows::frame;
pub use tl_ui_procs::windows::handle_search;
pub use tl_ui_sys::windows::confirm_startup_delete;
pub use tl_ui_sys::windows::connection_properties;
pub mod low_memory;
pub use tl_ui_perf::mini;
pub use tl_ui_procs::windows::notices;
pub use tl_ui_procs::windows::priority;
pub use tl_ui_procs::windows::props;
pub mod run;
pub mod run_history;
pub use tl_ui_procs::windows::signal;
pub use tl_ui_sys::windows::service_properties;
pub mod smart;
pub use tl_ui_sys::windows::startup_properties;
