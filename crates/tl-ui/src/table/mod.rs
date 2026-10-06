//! Table building blocks shared by every tasklens table (ADR-009).

pub mod actions;
pub mod cells;
pub mod columns;
pub mod columns_window;
pub mod header;
pub mod selection;
#[cfg(any(test, feature = "test-hooks"))]
pub mod test_delegate;
pub mod tree_table;

pub use tree_table::{
    ColumnResizeMode, HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind,
    TreeTableDelegate,
};
