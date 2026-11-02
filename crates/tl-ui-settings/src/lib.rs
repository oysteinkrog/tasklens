//! The window's settings: `settings.toml` and `state.toml` (`settings`) and the performance
//! layouts (`layout`, `layout_boxes`, `layout_pages`). tl-ui and tl-app use this crate; tl-engine,
//! tl-server and tl-remote do not, so an edit here does not rebuild them (ADR-020). `settings`
//! re-exports the engine's settings types from tl-model, so UI code imports every setting from
//! one path. No file I/O and no gpui-kit.

pub mod settings;

pub mod layout;

pub mod layout_boxes;

pub mod layout_pages;
