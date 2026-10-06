//! Performance tab actions. E24-002 (tasklens-crw.4) fills this file with every `Perf*` action;
//! `PerfShowMiniWindow` is declared ahead of it for the page frame (E24-006) and
//! `PerfSelectDevice` for the sidebar card (E24-003).

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, gpui};
use serde::Deserialize;

use super::DeviceKey;

gpui_kit::actions!(
    tasklens,
    [
        /// Opens the Performance mini window (R-058). No handler until E26.
        PerfShowMiniWindow,
    ]
);

/// Selects the device page of `key` (a sidebar card click). No handler until E24-004.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSelectDevice {
    pub key: DeviceKey,
}
