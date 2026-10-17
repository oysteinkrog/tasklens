//! Connections actions and key bindings (UI-SYS-060 to UI-SYS-067, R-128).

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, gpui};
use serde::Deserialize;

gpui_kit::actions!(
    tasklens,
    [
        ConnectionCopyLocal,
        ConnectionCopyForeign,
        ConnectionCloseTcp,
        ConnectionJumpToProcess,
        ConnectionGoToService,
    ]
);

/// Selects the Connections row whose socket has `inode` and activates the main window.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct GoToConnection {
    pub inode: u64,
}

/// Connections has no page-specific P2 key bindings. Shared table bindings still apply.
pub fn bind_keys_connections(_: &mut App) {}
