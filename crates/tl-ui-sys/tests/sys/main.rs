//! tl-ui-sys integration tests in one test binary (ADR-020): the system page tests moved from
//! the tl-ui `ui` binary (tasklens-zljr5.43).
//!
//! Run with `cargo test -p tl-ui-sys --test sys`.

mod common;
mod connections;
mod service_props;
mod services;
mod startup;
