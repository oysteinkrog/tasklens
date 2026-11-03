//! Connections actions and key bindings (UI-SYS-060 to UI-SYS-067, R-128).

use gpui_kit::{App, KeyBinding};

use crate::pages::processes::actions::CopyDetails;
/// `GoToConnection` moved to the shared `actions` module, so the process windows can dispatch it
/// without naming this page. Re-exported here for the Connections page.
pub use tl_ui_kit::actions::GoToConnection;

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

/// Binds Ctrl+C to `CopyDetails` for the row menu hint. The table's own `Table` context binds
/// Ctrl+C to CopyRow; bound later at the same depth, this one wins the menu hint.
pub fn bind_keys_connections(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "ctrl-c",
        CopyDetails,
        Some("ConnectionTable > Table"),
    )]);
}
