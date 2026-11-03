//! All tl-ui integration tests in one test binary. Each module was its own `tests/*.rs` file;
//! one binary links GPUI and wgpu once instead of once per file.
//!
//! Run one area with `cargo test -p tl-ui --test ui -- overview::`.

mod action_links;
mod actions;
mod bench_run;
mod bench_scenes;
mod chart_cpu_grid_hover;
mod common;
mod components_words;
mod connections_shell;
mod demand;
mod detached;
mod device_pages_shell;
mod focus;
mod history_shell;
mod host_clock;
mod host_manage;
mod host_settings;
mod host_tabs;
mod hub;
mod information;
mod low_memory;
mod menus;
mod minigraphs;
mod net_page_shell;
mod overview;
mod overview_range;
mod overview_timeline;
mod performance;
mod processes_shell;
mod props_shell;
mod run;
mod selectors;
mod services_shell;
mod settings_window;
mod setup_window;
mod shell;
mod shell_windows;
mod shield;
mod smart_window;
mod startup_shell;
mod status_bar;
mod theme;
mod tray_bridge;
