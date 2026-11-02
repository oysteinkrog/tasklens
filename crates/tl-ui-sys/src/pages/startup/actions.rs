//! Startup Apps actions and key bindings (UI-SYS-040 to UI-SYS-050, R-128).

use gpui_kit::{App, KeyBinding};

use crate::pages::processes::actions::CopyDetails;
use crate::table::actions::OpenProperties;

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
    cx.bind_keys([
        KeyBinding::new("delete", StartupDelete, Some("StartupTable")),
        // The table's own `Table` context binds Ctrl+C to CopyRow and Enter also has two
        // other keys. Bound later at the same depth, these win the menu hints.
        KeyBinding::new("ctrl-c", CopyDetails, Some("StartupTable > Table")),
        KeyBinding::new("enter", OpenProperties, Some("StartupTable > Table")),
    ]);
}
