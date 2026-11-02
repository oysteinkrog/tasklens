//! tl-ui-pagehost: mounts one page with the globals it needs, without `MainWindow` (ADR-020).
//!
//! Dev only. Integration tests under `tests/` use it with the feature `test-support`; it never
//! depends on tl-ui (the shell) or a page crate, so every page crate can use it. Without the
//! feature the crate is empty, which keeps gpui-kit `test-support` out of
//! `cargo build --workspace`.
//!
//! [`install_theme`] is enough for component tests. [`PageHost::install`] adds the settings,
//! strings, engine, hub, demand, window registry, `AppEnv` and a recording [`ShellApi`] whose
//! calls a test reads from [`ShellLog`]. [`selectors`] counts the debug selectors a window paints.
//!
//! [`ShellApi`]: tl_ui_base::shell::ShellApi

#[cfg(any(test, feature = "test-support"))]
mod host;

#[cfg(any(test, feature = "test-support"))]
pub use host::{LANGUAGE, PageHost, PushedToast, ShellLog, install_theme};

#[cfg(any(test, feature = "test-support"))]
pub mod helpers;

#[cfg(feature = "render-tests")]
pub mod render;

// The selector tags exist in debug builds only (`tl_ui_base::selector`).
#[cfg(all(any(test, feature = "test-support"), debug_assertions))]
pub mod selectors;
