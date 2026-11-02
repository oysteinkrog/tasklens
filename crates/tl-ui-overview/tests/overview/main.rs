//! tl-ui-overview integration tests in one test binary (ADR-020): the widget container, the
//! group widgets and the widget editor, moved from the tl-ui `ui` binary (tasklens-zljr5.38).
//!
//! Run with `cargo test -p tl-ui-overview --features test-hooks --test overview`.

mod overview_groups;
mod overview_widgets;
