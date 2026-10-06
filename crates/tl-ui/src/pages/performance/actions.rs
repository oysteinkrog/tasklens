//! Performance tab actions. E24-002 (tasklens-crw.4) fills this file with every `Perf*` action;
//! `PerfShowMiniWindow` is declared ahead of it for the page frame (E24-006) and
//! `PerfSelectDevice` for the sidebar card (E24-003), and `PerfToggleCpuKernel` and
//! `PerfSetCpuGraphMode` for the CPU page (E24-009).

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, gpui};
use serde::Deserialize;
use tl_model::settings::CpuGraphMode;

use super::DeviceKey;

gpui_kit::actions!(
    tasklens,
    [
        /// Opens the Performance mini window (R-058). No handler until E26.
        PerfShowMiniWindow,
        /// Shows or hides the kernel time line on the CPU page (PERF-029).
        PerfToggleCpuKernel,
    ]
);

/// Selects the device page of `key` (a sidebar card click). No handler until E24-004.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSelectDevice {
    pub key: DeviceKey,
}

/// Sets `performance.cpu_graph_mode`. `no_json` because `CpuGraphMode` has no JSON schema.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = tasklens, no_json)]
pub struct PerfSetCpuGraphMode {
    pub mode: CpuGraphMode,
}
