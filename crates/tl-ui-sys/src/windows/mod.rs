//! The service, connection and startup windows (tl-ui re-exports them under `windows`).
pub mod confirm_startup_delete;
pub mod connection_properties;
pub mod service_properties;
pub mod startup_properties;

pub(crate) use tl_ui_procs::windows::actions;
pub(crate) use tl_ui_procs::windows::props;
