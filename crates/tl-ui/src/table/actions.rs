//! The shared table Actions and their key bindings (E18-007, UI-PRC-008, UI-PRC-009, canonical
//! table (b) Table rows, D-008, R-128, R-129).
//!
//! One command is one Action in the `tasklens` namespace, so a menu item and a key dispatch the
//! same one and the menu shows the key hint. `TlTreeTable` handles the selection and expand
//! Actions on its root. The rest bubble to the owner page (open item 16) or are handled by later
//! table beads (E18-009 to E18-A2).

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, KeyBinding, SharedString, gpui};
use serde::Deserialize;
use tl_model::ids::ColumnId;

use crate::table::columns::TableId;
use crate::table::tree_table::ColumnResizeMode;

/// Key context of every table root.
pub const TABLE_CONTEXT: &str = "Table";

gpui_kit::actions!(
    tasklens,
    [
        SelectAll,
        ExpandOrChild,
        CollapseOrParent,
        ToggleExpand,
        ToggleSelectFocus,
        OpenProperties,
        CopyRow,
        CopyRowsAsTable,
        CollapseAll,
        OpenContextMenu,
    ]
);

/// Up or Down by `delta` selectable rows. `extend`: Shift held.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MoveFocus {
    pub delta: i32,
    pub extend: bool,
}

/// PgUp or PgDn: by the viewport rows - 1.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MoveFocusPage {
    pub down: bool,
    pub extend: bool,
}

/// Home or End.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct MoveFocusEdge {
    pub end: bool,
    pub extend: bool,
}

/// Copies one cell of the focus row. Handled by the owner page.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct CopyCell {
    /// Serialized as the column id string.
    #[schemars(with = "String")]
    pub col: ColumnId,
}

/// Header menu and header click. Handled by E18-009 to E18-A2.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SortByColumn {
    /// The column key, `TableColumn::key()`. The table parses it into its own id set, so the
    /// process and the system tables share the action.
    #[schemars(with = "String")]
    pub id: SharedString,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct OpenColumnPicker {
    pub table: TableId,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct HideColumn {
    /// The column key, as on [`SortByColumn`].
    #[schemars(with = "String")]
    pub id: SharedString,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ResizeColumnsToFit {
    pub table: TableId,
}

/// Writes `tables.column_resize`. Handled by the owner page (open item 16).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SetColumnResize {
    #[schemars(with = "String")]
    pub mode: ColumnResizeMode,
}

/// Handled by the table once its owner called `set_default_layout` (E18-012, R-184).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ResetColumns {
    pub table: TableId,
}

/// Shows every column of `table` that exists on this system (R-180). The Debug menu dispatches
/// it with the active tab's table; a table with another id ignores it.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct EnableAllColumns {
    pub table: TableId,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SetColumnVisible {
    #[schemars(with = "String")]
    pub id: ColumnId,
    pub on: bool,
}

/// Binds the Table rows of canonical table (b) in the `Table` context. Runs once at init,
/// before any menu is built.
pub fn bind_keys_table(cx: &mut App) {
    let table = Some(TABLE_CONTEXT);
    let mv = |delta, extend| MoveFocus { delta, extend };
    let page = |down, extend| MoveFocusPage { down, extend };
    let edge = |end, extend| MoveFocusEdge { end, extend };
    cx.bind_keys([
        KeyBinding::new("up", mv(-1, false), table),
        KeyBinding::new("down", mv(1, false), table),
        KeyBinding::new("shift-up", mv(-1, true), table),
        KeyBinding::new("shift-down", mv(1, true), table),
        KeyBinding::new("pageup", page(false, false), table),
        KeyBinding::new("pagedown", page(true, false), table),
        KeyBinding::new("shift-pageup", page(false, true), table),
        KeyBinding::new("shift-pagedown", page(true, true), table),
        KeyBinding::new("home", edge(false, false), table),
        KeyBinding::new("end", edge(true, false), table),
        KeyBinding::new("shift-home", edge(false, true), table),
        KeyBinding::new("shift-end", edge(true, true), table),
        KeyBinding::new("ctrl-a", SelectAll, table),
        KeyBinding::new("right", ExpandOrChild, table),
        KeyBinding::new("left", CollapseOrParent, table),
        KeyBinding::new("space", ToggleExpand, table),
        KeyBinding::new("ctrl-space", ToggleSelectFocus, table),
        KeyBinding::new("enter", OpenProperties, table),
        KeyBinding::new("alt-enter", OpenProperties, table),
        KeyBinding::new("ctrl-i", OpenProperties, table),
        // The ProcessTable context rebinds it to `CopyDetails` (E22).
        KeyBinding::new("ctrl-c", CopyRow, table),
        KeyBinding::new("ctrl-shift-c", CopyRowsAsTable, table),
        KeyBinding::new("ctrl-l", CollapseAll, table),
        KeyBinding::new("shift-f10", OpenContextMenu, table),
        KeyBinding::new("menu", OpenContextMenu, table),
    ]);
    crate::table::columns_window::bind_keys_column_picker(cx);
}
