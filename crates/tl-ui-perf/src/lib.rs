//! tl-ui-perf: the Performance page, its device pages and the mini window, moved from tl-ui
//! (ADR-020). tl-ui re-exports them at their old paths, `pages::performance` and `windows::mini`.
//! `performance::core`, `performance::overview` and `performance::widget` stay reachable through
//! re-exports in `performance`.

pub mod mini;
pub mod performance;

use tl_ui_base::action_tracker;
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
use tl_ui_base::util;
use tl_ui_chart::chart;
use tl_ui_kit::actions;
use tl_ui_kit::components;
use tl_ui_perf_core::minigraph;
use tl_ui_perf_core::tray_bridge;
use tl_ui_table::table;
