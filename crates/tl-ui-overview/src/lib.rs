//! tl-ui-overview: the Overview page and the widget container, moved from tl-ui (ADR-020).
//! tl-ui re-exports them at their old paths, `pages::performance::overview` and
//! `pages::performance::widget`.

pub mod overview;
pub mod widget;

use tl_ui_base::demand;
use tl_ui_base::host_settings;
use tl_ui_base::hub;
use tl_ui_base::i18n;
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
use tl_ui_perf_core::core;
