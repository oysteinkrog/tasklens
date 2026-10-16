//! Services actions and key bindings (UI-SYS-011, UI-SYS-012, UI-SYS-017 to UI-SYS-023,
//! UI-SYS-030, UI-SYS-036, UI-SYS-037, canonical table (b), R-120, R-128, R-129).
//!
//! Every Services command is one Action in the `tasklens` namespace. Shared table and Global
//! bindings are registered by their owners and are not repeated here.

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, KeyBinding, SharedString, gpui};
use serde::Deserialize;

/// Key context of the Services table.
pub const SERVICE_TABLE_CONTEXT: &str = "ServiceTable";
/// Key context of the Service Properties window.
pub const SERVICE_PROPS_CONTEXT: &str = "ServiceProps";
/// Key context of the Service Properties Logs list.
pub const JOURNAL_LIST_CONTEXT: &str = "JournalList";

gpui::actions!(
    tasklens,
    [
        ServiceStart,
        ServiceStop,
        ServiceRestart,
        ServiceReload,
        ServiceResetFailed,
        ServiceViewLogs,
        ServiceFileProperties,
        ServiceOpenFileLocation,
        ServiceOpenUnitFile,
        ServiceJumpToProcess,
        ServicesShowHidden,
        ServicesRemoveFilters,
        ServicesClearAllFilters,
        ServicesResetUnitTypes,
        ServicePropsNextTab,
        ServicePropsPrevTab,
        ServicePropsOk,
        ServiceLogsSelectAll,
        ServiceLogsCopySelected,
        ServiceLogsTop,
        ServiceLogsBottom,
    ]
);

/// Startup mode selected from the service row menu or Properties window.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[schemars(crate = "gpui::private::schemars")]
pub enum StartupChoice {
    Enabled,
    Disabled,
    Masked,
}

/// Changes the selected service's startup mode.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ServiceSetStartupType {
    pub startup: StartupChoice,
}

/// State represented by a Services filter-strip toggle.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[schemars(crate = "gpui::private::schemars")]
pub enum StateFilterArg {
    Running,
    Failed,
    Stopped,
    Disabled,
}

/// Toggles one Services state filter.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ServicesToggleStateFilter {
    pub state: StateFilterArg,
}

/// systemd manager represented by a Services filter toggle or jump target.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[schemars(crate = "gpui::private::schemars")]
pub enum ManagerArg {
    System,
    User,
}

/// Toggles one Services manager filter.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ServicesToggleManagerFilter {
    pub manager: ManagerArg,
}

/// systemd unit type represented by the Unit types menu.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[schemars(crate = "gpui::private::schemars")]
pub enum UnitTypeArg {
    Service,
    Socket,
    Timer,
    Mount,
    Automount,
    Path,
    Target,
    Swap,
    Slice,
    Scope,
}

impl UnitTypeArg {
    /// Returns the spelling stored in `services.unit_types`.
    pub fn as_setting(&self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Socket => "socket",
            Self::Timer => "timer",
            Self::Mount => "mount",
            Self::Automount => "automount",
            Self::Path => "path",
            Self::Target => "target",
            Self::Swap => "swap",
            Self::Slice => "slice",
            Self::Scope => "scope",
        }
    }

    /// Parses one value from `services.unit_types`.
    pub fn from_setting(value: &str) -> Option<Self> {
        match value {
            "service" => Some(Self::Service),
            "socket" => Some(Self::Socket),
            "timer" => Some(Self::Timer),
            "mount" => Some(Self::Mount),
            "automount" => Some(Self::Automount),
            "path" => Some(Self::Path),
            "target" => Some(Self::Target),
            "swap" => Some(Self::Swap),
            "slice" => Some(Self::Slice),
            "scope" => Some(Self::Scope),
            _ => None,
        }
    }
}

/// Toggles one Services unit-type filter.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ServicesToggleUnitType {
    pub kind: UnitTypeArg,
}

/// Selects a Service Properties tab by zero-based index.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ServicePropsSelectTab {
    pub index: u8,
}

/// Page from which a jump to a service originated.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[schemars(crate = "gpui::private::schemars")]
pub enum JumpOrigin {
    Processes,
    Connections,
}

/// Opens or reveals a service and records the page to return to.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct GoToService {
    pub manager: ManagerArg,
    pub name: SharedString,
    pub origin: JumpOrigin,
}

/// Binds the Services rows of canonical table (b).
pub fn bind_keys_services(cx: &mut App) {
    let table = Some(SERVICE_TABLE_CONTEXT);
    let props = Some(SERVICE_PROPS_CONTEXT);
    let journal = Some(JOURNAL_LIST_CONTEXT);

    let tabs: Vec<KeyBinding> = (1..=5u8)
        .map(|number| {
            KeyBinding::new(
                &format!("ctrl-{number}"),
                ServicePropsSelectTab { index: number - 1 },
                props,
            )
        })
        .collect();
    cx.bind_keys(tabs);

    cx.bind_keys([
        KeyBinding::new("ctrl-s", ServiceStart, table),
        KeyBinding::new("ctrl-e", ServiceStop, table),
        KeyBinding::new("ctrl-r", ServiceRestart, table),
        KeyBinding::new("ctrl-tab", ServicePropsNextTab, props),
        KeyBinding::new("ctrl-pagedown", ServicePropsNextTab, props),
        KeyBinding::new("ctrl-shift-tab", ServicePropsPrevTab, props),
        KeyBinding::new("ctrl-pageup", ServicePropsPrevTab, props),
        KeyBinding::new("enter", ServicePropsOk, props),
        KeyBinding::new("ctrl-a", ServiceLogsSelectAll, journal),
        KeyBinding::new("ctrl-c", ServiceLogsCopySelected, journal),
        KeyBinding::new("home", ServiceLogsTop, journal),
        KeyBinding::new("end", ServiceLogsBottom, journal),
    ]);
}
