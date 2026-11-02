//! tl-ui-perf-core: device keys, perf actions, details, core clocks, minigraphs, the tray bridge
//! and the detached graph window, moved from tl-ui (ADR-020). tl-ui re-exports them at their old
//! paths: `pages::performance::core`, `pages::minigraph`, `tray_bridge` and
//! `windows::detached_graph`.

pub mod core;
pub mod detached_graph;
pub mod minigraph;
pub mod tray_bridge;

use tl_ui_base::anim;
use tl_ui_base::app_env;
use tl_ui_base::demand;
use tl_ui_base::host;
use tl_ui_base::hub;
use tl_ui_base::i18n;
use tl_ui_base::registry;
use tl_ui_base::selector;
use tl_ui_base::settings_global;
use tl_ui_base::shell;
use tl_ui_base::template;
use tl_ui_base::text;
use tl_ui_base::theme;
use tl_ui_chart::chart;
use tl_ui_chart::net_totals;
use tl_ui_kit::actions;
use tl_ui_kit::components;
