//! tl-ui-perf integration tests in one test binary (ADR-020): the Overview and device page
//! widget tests that need the device sidebar, moved from the tl-ui `ui` binary
//! (tasklens-zljr5.38).
//!
//! Run with `cargo test -p tl-ui-perf --features test-hooks --test perf`.

mod battery_page;
mod cpu_page;
mod disk_page;
mod fan_page;
mod gpu_page;
mod memory_page;
mod overview_cpu_widgets;
mod overview_disk_widgets;
mod overview_fixed_widgets;
mod overview_mem_widgets;
mod overview_net_widgets;
mod overview_widget_look;
mod page_widgets;
