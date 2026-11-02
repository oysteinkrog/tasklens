//! tl-ui-procs integration tests in one test binary (ADR-020): the process table tests moved
//! from the tl-ui `ui` binary (tasklens-zljr5.40).
//!
//! Run with `cargo test -p tl-ui-procs --test procs`.

mod common;
mod dialogs;
mod dialogs_process;
mod handle_search;
mod helper_flow;
mod history;
mod process_cells;
mod process_menu;
mod process_priority_menu;
mod process_tooltips;
mod processes;
mod props;
