//! Connections actions and key bindings (UI-SYS-060 to UI-SYS-067, R-128).

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, KeyBinding, gpui};
use serde::Deserialize;

use crate::pages::processes::actions::CopyDetails;

gpui_kit::actions!(
    tasklens,
    [
        ConnectionCopyLocal,
        ConnectionCopyForeign,
        ConnectionCloseTcp,
        ConnectionJumpToProcess,
        ConnectionGoToService,
        /// The Namespaces control: list sockets of this namespace only.
        ConnectionNetnsHost,
        /// The Namespaces control: list sockets of every namespace through the helper.
        ConnectionNetnsAll,
    ]
);

/// Selects the Connections row whose socket has `inode` and activates the main window.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct GoToConnection {
    pub inode: u64,
}

/// Binds Ctrl+C to `CopyDetails` for the row menu hint. The table's own `Table` context binds
/// Ctrl+C to CopyRow; bound later at the same depth, this one wins the menu hint.
pub fn bind_keys_connections(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "ctrl-c",
        CopyDetails,
        Some("ConnectionTable > Table"),
    )]);
}
