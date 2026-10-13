//! Startup Apps actions and key bindings (UI-SYS-040 to UI-SYS-050, R-128).

use gpui_kit::{App, KeyBinding};

gpui_kit::actions!(
    tasklens,
    [
        StartupEnable,
        StartupDisable,
        StartupRunNow,
        StartupCopyCommand,
        StartupOpenFileLocation,
        StartupDelete,
    ]
);

/// Binds the Startup Apps row keys from canonical table (b).
pub fn bind_keys_startup(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "delete",
        StartupDelete,
        Some("StartupTable"),
    )]);
}
