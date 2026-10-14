//! Connections actions and key bindings (UI-SYS-060 to UI-SYS-067, R-128).

use gpui_kit::App;

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

/// Connections has no page-specific P2 key bindings. Shared table bindings still apply.
pub fn bind_keys_connections(_: &mut App) {}
