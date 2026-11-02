//! The window's side of tl-model: `settings.toml` and `state.toml` (`settings`) and the
//! performance layouts (`layout`, `layout_boxes`, `layout_pages`). tl-ui and tl-app use these;
//! tl-engine, tl-server and tl-remote do not. Code here may use the rest of tl-model; the rest
//! of tl-model never names a path in here, apart from the re-exports that keep the old paths.

pub mod settings;

pub mod layout;

pub mod layout_boxes;

pub mod layout_pages;
