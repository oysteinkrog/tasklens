//! tl-ui-overview integration tests in one test binary (ADR-020): the widget container, the
//! group widgets and the widget editor (tasklens-zljr5.38), and the Overview drawing parts and
//! boxes (tasklens-zljr5.41), moved from the tl-ui `ui` binary.
//!
//! Run with `cargo test -p tl-ui-overview --features test-hooks --test overview`.

mod cpu_apps;
mod gpu_apps;
mod overview;
mod overview_groups;
mod overview_widgets;
mod remote_remap;
