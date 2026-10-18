//! Performance tab actions and the Performance key bindings (E24-002, UI-PRF-010, UI-PRF-012,
//! canonical table (b), R-128, UI-SHL-120).
//!
//! Every menu item, button and key of the Performance tab and the E25 device pages dispatches
//! one Action declared here, all in the `tasklens` namespace with a `Perf` prefix. Later beads
//! add handlers, not Actions.
//!
//! Actions that carry a `tl_model::settings` enum keep the model type as the field, and read
//! and describe their JSON through a mirror enum declared here: tl-model has no JSON schema
//! derive (ADR-001).

use std::sync::Arc;

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, KeyBinding, gpui};
use serde::{Deserialize, Deserializer};
pub use tl_model::settings::{CpuGraphMode, CpuSecondaryGraph, GpuOptionalGraph};

use super::{DeviceKey, DeviceKind};

/// Key context of the Performance tab.
pub const PERFORMANCE_CONTEXT: &str = "Performance";
/// Key context while the sidebar page picker is open.
pub const PERFORMANCE_EDIT_CONTEXT: &str = "PerformanceEdit";
/// Key context of the Summary mode view (PERF-010).
pub const PERFORMANCE_SUMMARY_CONTEXT: &str = "PerformanceSummary";
/// Key context of the Overview page while Edit Layout mode is on (UI-PRF-018).
pub const PERFORMANCE_OVERVIEW_EDIT_CONTEXT: &str = "PerformanceOverviewEdit";

gpui_kit::actions!(
    tasklens,
    [
        PerfSelectPrevDevice,
        PerfSelectNextDevice,
        PerfSelectFirstDevice,
        PerfSelectLastDevice,
        PerfToggleSidebar,
        PerfToggleSummaryMode,
        PerfCopyDeviceInfo,
        PerfCopyValue,
        PerfEnterEditMode,
        PerfExitEditMode,
        PerfEnableAllPages,
        PerfDisableAllPages,
        PerfResetPagesToDefault,
        /// Opens the Performance mini window (R-058). No handler until E26.
        PerfShowMiniWindow,
        /// Shows or hides the kernel time line on the CPU page (PERF-029).
        PerfToggleCpuKernel,
        PerfToggleMemoryComposition,
        PerfToggleMemorySwapGraph,
        PerfOpenNetworkSettings,
        /// Bound to keys that Summary mode swallows (PERF-010). Has no handler.
        NoOp,
        /// Opens the card menu of the selected sidebar card (Shift+F10, Menu; UI-PRF-014).
        PerfOpenCardMenu,
        /// Turns on Edit Layout mode of the Overview page (UI-PRF-018).
        PerfOverviewEditLayout,
        /// Turns off Edit Layout mode (Done, Esc).
        PerfOverviewEditDone,
        /// Restores the default Overview layout and keeps Edit Layout mode on.
        PerfOverviewResetLayout,
        /// Hides the focused Overview box (Delete in Edit Layout mode).
        PerfOverviewHideBox,
    ]
);

/// Adds the Overview box of `kind` (its `[performance.overview]` name) to the layout.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfOverviewAddBox {
    pub kind: Arc<str>,
}

/// Moves the focused Overview box one slot left or right (`dx`), or to the row above or below
/// (`dy`).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfOverviewMoveBox {
    pub dx: i8,
    pub dy: i8,
}

/// Shrinks (−1) or grows (1) the focused Overview box's width (`dw`) or row height (`dh`).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfOverviewResizeBox {
    pub dw: i8,
    pub dh: i8,
}

/// Switches the Overview box of `kind` to its layout mode `mode` (the box menu's "Layout"
/// items, UI-PRF-019).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfOverviewSetBoxMode {
    pub kind: Arc<str>,
    pub mode: Arc<str>,
}

/// Gives the Overview box of `kind` its default mode and split (the box menu's "Reset Box").
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfOverviewResetBox {
    pub kind: Arc<str>,
}

/// Selects the device page of `key` (a sidebar card click).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSelectDevice {
    pub key: DeviceKey,
}

/// Selects the visible sidebar card at `index` (F1 to F8).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSelectVisibleDevice {
    pub index: u8,
}

/// Sets the graph time range in seconds.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSetGraphRange {
    pub secs: u16,
}

/// Selects the first device of `kind`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfJumpToType {
    pub kind: DeviceKind,
}

/// Shows or hides the sidebar card and page of `key`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSetPageVisible {
    pub key: DeviceKey,
    pub visible: bool,
}

/// Moves the card of `key` before `before`, or to the end when `before` is `None`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfMoveDevice {
    pub key: DeviceKey,
    pub before: Option<DeviceKey>,
}

/// Moves the focused card by `delta` places (Alt+Up -1, Alt+Down +1).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfMoveFocusedDevice {
    pub delta: i8,
}

/// Adds a tray graph that shows `source`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfAddTrayGraph {
    pub source: DeviceKey,
}

/// Sets `performance.cpu_graph_mode`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSetCpuGraphMode {
    #[serde(deserialize_with = "via::<_, CpuGraphModeJson, _>")]
    #[schemars(with = "CpuGraphModeJson")]
    pub mode: CpuGraphMode,
}

/// Sets `performance.cpu_secondary_graph`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSetCpuSecondaryGraph {
    #[serde(deserialize_with = "via::<_, CpuSecondaryGraphJson, _>")]
    #[schemars(with = "CpuSecondaryGraphJson")]
    pub choice: CpuSecondaryGraph,
}

/// Opens the SMART view of the disk `key`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfShowSmart {
    pub key: DeviceKey,
}

/// Reads the SMART data of the disk `key` again.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSmartRefresh {
    pub key: DeviceKey,
}

/// Reads the SMART data of the disk `key` through smartctl.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSmartReadSmartctl {
    pub key: DeviceKey,
}

/// Ejects the disk `key`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfEject {
    pub key: DeviceKey,
}

/// Shows or hides the graph of `engine` on the GPU page of `key`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfToggleGpuEngine {
    pub key: DeviceKey,
    pub engine: Arc<str>,
}

/// Shows or hides an optional graph on the GPU page of `key`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfToggleGpuOptionalGraph {
    pub key: DeviceKey,
    #[serde(deserialize_with = "via::<_, GpuOptionalGraphJson, _>")]
    #[schemars(with = "GpuOptionalGraphJson")]
    pub which: GpuOptionalGraph,
}

/// Starts renaming the fan `key`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfRenameFan {
    pub key: DeviceKey,
}

/// Sets the temperature sensor shown with the fan `key`, or none.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfSetFanTempSensor {
    pub key: DeviceKey,
    pub sensor: Option<Arc<str>>,
}

/// Looks up the public IP address of the interface `key`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PerfFetchPublicIp {
    pub key: DeviceKey,
}

/// Reads a model enum through its JSON mirror `M`.
fn via<'de, D: Deserializer<'de>, M: Deserialize<'de> + Into<T>, T>(d: D) -> Result<T, D::Error> {
    M::deserialize(d).map(Into::into)
}

/// Declares a JSON mirror of a `tl_model::settings` enum with `From` both ways. The serde
/// renaming must match the model enum, so the keymap and settings spell values the same way.
macro_rules! json_mirror {
    ($(#[$meta:meta])* $mirror:ident <-> $model:ident, $case:literal, [$($v:ident),+ $(,)?]) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, JsonSchema)]
        #[serde(rename_all = $case)]
        #[schemars(crate = "gpui::private::schemars")]
        pub enum $mirror {
            $($v),+
        }

        impl From<$mirror> for $model {
            fn from(m: $mirror) -> Self {
                match m {
                    $($mirror::$v => Self::$v),+
                }
            }
        }

        impl From<$model> for $mirror {
            fn from(m: $model) -> Self {
                match m {
                    $($model::$v => Self::$v),+
                }
            }
        }
    };
}

json_mirror!(
    /// JSON form of [`CpuGraphMode`].
    CpuGraphModeJson <-> CpuGraphMode,
    "snake_case",
    [Overall, PerCore, AllCoresOverlaid, AllCoresStacked]
);
json_mirror!(
    /// JSON form of [`CpuSecondaryGraph`].
    CpuSecondaryGraphJson <-> CpuSecondaryGraph,
    "lowercase",
    [None, Power, Clock, Temperature]
);
json_mirror!(
    /// JSON form of [`GpuOptionalGraph`].
    GpuOptionalGraphJson <-> GpuOptionalGraph,
    "lowercase",
    [Power, Clocks]
);

/// Binds the Performance rows of canonical table (b).
pub fn bind_keys_performance(cx: &mut App) {
    let perf = Some(PERFORMANCE_CONTEXT);
    let edit = Some(PERFORMANCE_EDIT_CONTEXT);
    let summary = Some(PERFORMANCE_SUMMARY_CONTEXT);
    let overview_edit = Some(PERFORMANCE_OVERVIEW_EDIT_CONTEXT);
    let mut bindings = Vec::new();
    for i in 0..8u8 {
        bindings.push(KeyBinding::new(
            &format!("f{}", i + 1),
            PerfSelectVisibleDevice { index: i },
            perf,
        ));
    }
    for i in 1..=6 {
        bindings.push(KeyBinding::new(&format!("ctrl-{i}"), NoOp, summary));
        bindings.push(KeyBinding::new(&format!("alt-{i}"), NoOp, summary));
    }
    cx.bind_keys(bindings);

    cx.bind_keys([
        KeyBinding::new("up", PerfSelectPrevDevice, perf),
        KeyBinding::new("down", PerfSelectNextDevice, perf),
        KeyBinding::new("home", PerfSelectFirstDevice, perf),
        KeyBinding::new("end", PerfSelectLastDevice, perf),
        KeyBinding::new("f9", PerfToggleSidebar, perf),
        KeyBinding::new("ctrl-t", PerfToggleSidebar, perf),
        KeyBinding::new("ctrl-m", PerfToggleSummaryMode, perf),
        KeyBinding::new("ctrl-c", PerfCopyDeviceInfo, perf),
        KeyBinding::new("alt-up", PerfMoveFocusedDevice { delta: -1 }, perf),
        KeyBinding::new("alt-down", PerfMoveFocusedDevice { delta: 1 }, perf),
        KeyBinding::new("shift-f10", PerfOpenCardMenu, perf),
        KeyBinding::new("menu", PerfOpenCardMenu, perf),
        KeyBinding::new("escape", PerfExitEditMode, edit),
        KeyBinding::new("escape", PerfToggleSummaryMode, summary),
        KeyBinding::new("ctrl-m", PerfToggleSummaryMode, summary),
        KeyBinding::new("ctrl-tab", NoOp, summary),
        KeyBinding::new("ctrl-shift-tab", NoOp, summary),
        // The menus are hidden in summary mode, so their access keys do nothing.
        KeyBinding::new("alt-f", NoOp, summary),
        KeyBinding::new("alt-v", NoOp, summary),
        KeyBinding::new("alt-s", NoOp, summary),
        KeyBinding::new("alt-h", NoOp, summary),
        KeyBinding::new("alt-d", NoOp, summary),
    ]);

    let move_box = |dx, dy| PerfOverviewMoveBox { dx, dy };
    let resize_box = |dw, dh| PerfOverviewResizeBox { dw, dh };
    cx.bind_keys([
        KeyBinding::new("escape", PerfOverviewEditDone, overview_edit),
        KeyBinding::new("delete", PerfOverviewHideBox, overview_edit),
        KeyBinding::new("alt-left", move_box(-1, 0), overview_edit),
        KeyBinding::new("alt-right", move_box(1, 0), overview_edit),
        KeyBinding::new("alt-up", move_box(0, -1), overview_edit),
        KeyBinding::new("alt-down", move_box(0, 1), overview_edit),
        KeyBinding::new("alt-shift-left", resize_box(-1, 0), overview_edit),
        KeyBinding::new("alt-shift-right", resize_box(1, 0), overview_edit),
        KeyBinding::new("alt-shift-up", resize_box(0, -1), overview_edit),
        KeyBinding::new("alt-shift-down", resize_box(0, 1), overview_edit),
    ]);
}
