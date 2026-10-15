//! History mode actions and key bindings (UI-PRC-030, UI-PRC-032, UI-PRC-035, R-128).
//!
//! All in the `tasklens` namespace. The scrubber keys are bound in the `History` key context,
//! which the page root sets. `Esc` is bound in `History && scrubbed`, so it runs before the
//! global Esc rule only while a scrub point is set.

use gpui_kit::{Action, App, KeyBinding};
use tl_model::ids::ProcessKey;
use tl_model::time::TimeNs;

/// The key context of the History page root.
pub const HISTORY_CONTEXT: &str = "History";
/// The context flag the page adds while a scrub point is set.
pub const SCRUBBED: &str = "scrubbed";

gpui_kit::actions!(
    tasklens,
    [
        HistoryStepBack,
        HistoryStepForward,
        HistoryStepBackMinute,
        HistoryStepForwardMinute,
        HistoryOldest,
        HistoryLive,
        HistoryMetricCpu,
        HistoryMetricMemory,
        HistoryMetricDisk,
        HistoryMetricNetwork,
        HistoryMetricGpu,
        DeleteFromHistory,
        HistoryShowInLiveTable,
    ]
);

/// Moves the scrub point to `t`. `no_json`: dispatched by the strip, never read from a keymap
/// file.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = tasklens, no_json)]
pub struct HistoryScrub {
    pub t: TimeNs,
}

/// Sets the selected range. `no_json` as above.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = tasklens, no_json)]
pub struct HistorySetRange {
    pub from: TimeNs,
    pub to: TimeNs,
}

/// Clears one process from the history. `no_json`: `ProcessKey` has no serde derive (ADR-002).
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = tasklens, no_json)]
pub struct HistoryClearProcess {
    pub key: ProcessKey,
}

/// Binds the History rows of canonical table (b).
pub fn bind_keys_history(cx: &mut App) {
    let ctx = Some(HISTORY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", HistoryStepBack, ctx),
        KeyBinding::new("right", HistoryStepForward, ctx),
        KeyBinding::new("shift-left", HistoryStepBackMinute, ctx),
        KeyBinding::new("shift-right", HistoryStepForwardMinute, ctx),
        KeyBinding::new("home", HistoryOldest, ctx),
        KeyBinding::new("end", HistoryLive, ctx),
        KeyBinding::new("1", HistoryMetricCpu, ctx),
        KeyBinding::new("2", HistoryMetricMemory, ctx),
        KeyBinding::new("3", HistoryMetricDisk, ctx),
        KeyBinding::new("4", HistoryMetricNetwork, ctx),
        KeyBinding::new("5", HistoryMetricGpu, ctx),
        KeyBinding::new("escape", HistoryLive, Some("History && scrubbed")),
    ]);
}
