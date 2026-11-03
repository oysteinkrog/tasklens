//! tl-ui-perf-core: device keys, perf actions, details, core clocks, minigraphs, the tray bridge
//! and the detached graph window, moved from tl-ui (ADR-020). tl-ui re-exports them at their old
//! paths: `pages::performance::core`, `pages::minigraph`, `tray_bridge` and
//! `windows::detached_graph`.

pub mod core;
pub mod detached_graph;
pub mod minigraph;
pub mod tray_bridge;
