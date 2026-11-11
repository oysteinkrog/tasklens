//! tl-ui-perf integration tests in one test binary (ADR-020): the Overview and device page
//! widget tests that need the device sidebar (tasklens-zljr5.38), the device pages
//! (tasklens-zljr5.39), the OverviewPage and time range tests (tasklens-zljr5.41), and the
//! device_pages and net_page tests that need no shell (tasklens-zljr5.52), moved from the tl-ui
//! `ui` binary.
//!
//! Run with `cargo test -p tl-ui-perf --features test-hooks --test perf`.

mod battery_page;
mod cpu_page;
mod device_pages;
mod disk_page;
mod fan_page;
mod gpu_page;
mod memory_page;
mod net_page;
mod overview;
mod overview_cpu_widgets;
mod overview_disk_widgets;
mod overview_fixed_widgets;
mod overview_gpu_widgets;
mod overview_mem_widgets;
mod overview_net_widgets;
mod overview_range;
mod overview_widget_look;
mod page_agg_gpu_widgets;
mod page_battery_widgets;
mod page_cpu_widgets;
mod page_disk_widgets;
mod page_fan_widgets;
mod page_gpu_widgets;
mod page_look;
mod page_mem_widgets;
mod page_net_widgets;
mod page_range;
mod page_remote;
mod page_widgets;
mod performance;
mod sidebar;
