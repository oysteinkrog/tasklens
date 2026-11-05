//! Table building blocks shared by every tasklens table (ADR-009).

pub mod actions;
pub mod cells;
pub mod columns;
pub mod columns_window;
pub mod header;
mod row_paint;
pub mod selection;
pub mod sys_copy;
pub mod sys_status;
pub mod sys_table;
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
pub mod test_delegate;
pub mod tree_core;
pub mod tree_header;
pub mod tree_table;

pub use tree_table::{
    ColumnResizeMode, HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind,
    TreeTableDelegate,
};
