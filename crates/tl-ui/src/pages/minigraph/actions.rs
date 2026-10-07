//! Minigraph actions (E26, UI-PRF-040 to UI-PRF-045, UI-PRC-020, APP-021, R-128).
//!
//! Every minigraph command is one Action in the `tasklens` namespace, declared here for the
//! minigraph panel, the View > Graphs menu (E21-003 dispatches `ResetMinigraphLayout`) and the
//! process row menu (E26-013). There are no key bindings: every minigraph command is
//! menu-driven (table (b) has no minigraph rows). `ToggleMiniWindow` and `PerfShowMiniWindow`
//! are declared elsewhere (E20-A1, E24-002). Later beads add handlers, not Actions.

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, gpui};
use serde::{Deserialize, Serialize};

use super::GraphColor;
use super::sources::{GraphColumn, MinigraphId, MinigraphSource};

gpui_kit::actions!(
    tasklens,
    [
        /// Restores the default cards and slots (UI-PRF-044).
        ResetMinigraphLayout,
        /// Attaches every detached card again (E26-016, E26-018).
        MinigraphAttachAll,
        ToggleMiniAlwaysOnTop,
        CloseMiniWindow,
    ]
);

/// Hides a system card; its `graphs.minigraph_*` setting turns off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphHide {
    pub id: MinigraphId,
}

/// Removes a process, name or app card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphRemove {
    pub id: MinigraphId,
}

/// Sets a card's time range in seconds (PERF-081).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphSetRange {
    pub id: MinigraphId,
    pub secs: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphSetColor {
    pub id: MinigraphId,
    pub color: GraphColor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphMoveToSlot {
    pub id: MinigraphId,
    pub slot: u16,
}

/// Moves a card into its own window (R-084).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphDetach {
    pub id: MinigraphId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphAttach {
    pub id: MinigraphId,
}

/// Adds a card for `source` to detached window `window`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphAddToWindow {
    pub window: u32,
    pub source: MinigraphSource,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MinigraphCloseDetachedWindow {
    pub window: u32,
}

/// Adds a card for the selected process and `column` (UI-PRC-020).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct AddMinigraph {
    pub column: GraphColumn,
}

/// Adds a card for every process with the selected process's image name (UI-PRC-020).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct AddNameGraph {
    pub column: GraphColumn,
}

/// Adds a card for every process of the selected process's app (UI-PRC-020).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct AddAppGraph {
    pub column: GraphColumn,
}
