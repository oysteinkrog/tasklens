//! tl-ui-sys: the Services, Connections and Startup pages and their windows, moved from tl-ui
//! (ADR-020). The files keep their paths under `src/`, so tl-ui re-exports them at their old paths.

pub mod pages;
pub mod windows;

use tl_ui_base::action_tracker;
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
use tl_ui_kit::actions;
use tl_ui_kit::components;
use tl_ui_kit::helper_flow;
use tl_ui_table::table;
