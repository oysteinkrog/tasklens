//! The contract between `TlTreeTable` and the table owners (UI-PRC-001, UI-PRC-002, UI-PRC-003,
//! ADR-009, D-011, R-075, R-105).
//!
//! The delegate supplies finished rows, already sorted and filtered by the engine, and names
//! them by identity (`RowId`), never by index. The table draws them and reports gestures back.
//! Every geometry constant here is in logical px at 100% and is multiplied by `ui_scale` at use.

use std::hash::Hash;
use std::marker::PhantomData;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{AnyElement, AnyView, Context, SharedString, Window, gpui};
use serde::Deserialize;
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::table::SortSpec;

use crate::components::menu::TlMenu;
use crate::table::cells::CellOut;
use crate::table::columns::{ColumnLayout, ColumnSpec};

/// Indent per tree level.
pub const INDENT: f32 = 18.0;
/// Chevron glyph size.
pub const CHEVRON: f32 = 16.0;
/// Chevron hit width.
pub const CHEVRON_HIT_W: f32 = 20.0;
/// Gap between the icon and the text (sp3).
pub const ICON_GAP: f32 = 6.0;
/// Cell padding left and right.
pub const CELL_PAD: f32 = 8.0;
/// Header height with the totals line.
pub const HEADER_H_TWO: f32 = 44.0;
/// Header height without totals.
pub const HEADER_H_ONE: f32 = 28.0;
/// Scrollbar width (open item 14).
pub const SCROLLBAR_W: f32 = 16.0;

/// What one table owner supplies. Row indices are positions in the current flattened row list
/// and are only valid for the current `table_version`; anything kept across frames uses `RowId`.
pub trait TreeTableDelegate: 'static + Sized {
    /// Row identity. Never an index.
    type RowId: Copy + Eq + Hash + 'static;

    fn row_count(&self) -> usize;
    fn row_id(&self, ix: usize) -> Self::RowId;
    /// From an `FxHashMap` the delegate rebuilds per table version.
    fn row_index(&self, id: Self::RowId) -> Option<usize>;
    fn row_kind(&self, ix: usize) -> RowKind;
    fn row_depth(&self, ix: usize) -> u8;
    /// `None`: no chevron. `Some(expanded)` otherwise.
    fn row_expandable(&self, ix: usize) -> Option<bool>;
    fn row_flags(&self, ix: usize) -> RowFlags;
    /// Bumps when the rows change. Drives `row_index` and `CellTextCache`.
    fn table_version(&self) -> u64;
    fn columns(&self) -> &ColumnLayout;
    fn column_spec(&self, id: ColumnId) -> &ColumnSpec;
    fn cell(&self, ix: usize, col: ColumnId, out: &mut CellOut);
    fn name_cell(&self, ix: usize) -> NameCell;
    /// Fills the header totals line for `col`. False: no line 2 for this column.
    fn header_total(&self, col: ColumnId, out: &mut CellOut) -> bool;
    fn sort(&self) -> SortSpec;
    /// False for group headers.
    fn selectable(&self, ix: usize) -> bool;
    fn context_menu(
        &mut self,
        targets: &[Self::RowId],
        clicked_col: ColumnId,
        menu: TlMenu,
        window: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu;
    fn on_toggle_expand(&mut self, id: Self::RowId, cx: &mut Context<TlTreeTable<Self>>);
    fn on_sort(&mut self, col: ColumnId, cx: &mut Context<TlTreeTable<Self>>);
    /// The delegate persists the layout.
    fn on_columns_changed(&mut self, layout: &ColumnLayout, cx: &mut Context<TlTreeTable<Self>>);
    /// Demand for the visible rows, at most 512 ids.
    fn on_visible_range(&mut self, ids: &[Self::RowId], cx: &mut Context<TlTreeTable<Self>>);
    /// Enter or double-click.
    fn on_activate(&mut self, id: Self::RowId, cx: &mut Context<TlTreeTable<Self>>);
    fn row_tooltip(&self, ix: usize) -> Option<AnyView>;
    /// Text for an empty table (UI-PRC-024).
    fn empty_text(&self) -> Option<SharedString>;

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::Two
    }

    /// True: no column picker and no header-menu items 2, 3 and 6. Resize and sort still work.
    fn columns_fixed(&self) -> bool {
        false
    }

    /// `processes.tree_guide_lines` (PROC-017).
    fn guide_lines(&self) -> bool {
        false
    }

    /// True (Processes, R-216): double-click toggles a row with children, does nothing on a
    /// leaf, and never calls `on_activate`.
    fn double_click_expands(&self) -> bool {
        false
    }
}

/// Placeholder until E18-006 adds the entity.
pub struct TlTreeTable<D: TreeTableDelegate> {
    _delegate: PhantomData<D>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKind {
    GroupHeader,
    App,
    Unit,
    Process,
    KernelThread,
    Pseudo,
    Plain,
}

/// Row state flags. A plain newtype: the bitflags crate is not in the tl-ui dependency row
/// (ADR-001).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RowFlags {
    pub bits: u16,
    /// Bit k: the ancestor at depth k has a later sibling, so its guide line continues.
    pub ancestor_continues: u64,
}

impl RowFlags {
    pub const NEW: u16 = 1;
    pub const EXITED: u16 = 2;
    pub const STALE: u16 = 4;
    pub const RESTRICTED: u16 = 8;
    pub const SUSPENDED: u16 = 16;
    pub const FROZEN: u16 = 32;
    pub const LAST_SIBLING: u16 = 64;

    pub fn has(self, f: u16) -> bool {
        self.bits & f != 0
    }

    /// True when the ancestor at `depth` has a later sibling. False past depth 63.
    pub fn ancestor_continues_at(self, depth: u8) -> bool {
        depth < 64 && (self.ancestor_continues >> depth) & 1 != 0
    }
}

pub enum NameIcon {
    File(Arc<str>),
    Lucide(IconName),
    None,
}

/// The parts of a Name cell (UI-PRC-003). Holds `SharedString` clones only, no owned `String`.
pub struct NameCell {
    pub icon: NameIcon,
    pub icon_size: IconSlot,
    /// Includes the count "(N)" and qualifiers such as "(Flatpak)", "(Snap)", "(Wine)".
    pub primary: SharedString,
    /// Byte range of a live count inside `primary`, drawn with tnum (UI-FND-051 rule 5).
    pub count_run: Option<Range<usize>>,
    /// App window title (P2), in `text_secondary`.
    pub secondary: Option<SharedString>,
    /// Status glyphs with their tooltips, e.g. (pause, "Suspended"), (snowflake, "Frozen").
    pub glyphs: SmallVec<[(IconName, SharedString); 2]>,
    /// True for Name cells: U+FFFD is drawn in `status.warning` (UI-FND-053 rule 2).
    pub highlight_fffd: bool,
    /// Element at the cell's right edge (process window button, P2).
    pub trailing: Option<AnyElement>,
}

/// `Large` is 24 px (app rows), `Small` 16 px (all other rows).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconSlot {
    Large,
    Small,
}

/// `One`: header 28 px tall, no totals line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeaderLines {
    One,
    Two,
}

/// `tables.column_resize` (R-075).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "gpui::private::schemars")]
pub enum ColumnResizeMode {
    Proportional,
    LastColumn,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::test_delegate::VecDelegate;

    #[test]
    fn row_flags_bits_and_ancestors() {
        let f = RowFlags {
            bits: RowFlags::NEW | RowFlags::SUSPENDED,
            ancestor_continues: 0b101,
        };
        assert!(f.has(RowFlags::NEW));
        assert!(f.has(RowFlags::SUSPENDED));
        assert!(!f.has(RowFlags::EXITED));
        assert!(f.ancestor_continues_at(0));
        assert!(!f.ancestor_continues_at(1));
        assert!(f.ancestor_continues_at(2));
        assert!(!f.ancestor_continues_at(200));
    }

    #[test]
    fn delegate_defaults() {
        let d = VecDelegate::new(&[(1u32, "one"), (2, "two"), (3, "three")]);
        assert_eq!(d.header_lines(), HeaderLines::Two);
        assert!(!d.columns_fixed());
        assert!(!d.guide_lines());
        assert!(!d.double_click_expands());
        assert_eq!(d.row_count(), 3);
        assert_eq!(d.row_kind(0), RowKind::Plain);
        assert_eq!(d.row_index(2), Some(1));
        assert_eq!(d.row_index(9), None);
        assert_eq!(d.name_cell(2).primary.as_ref(), "three");

        let mut out = CellOut {
            want_text: true,
            ..CellOut::default()
        };
        d.cell(0, ColumnId::Name, &mut out);
        assert_eq!(out.text, "one");
    }

    fn assert_delegate<D: TreeTableDelegate>(d: &D) -> D::RowId {
        d.row_id(0)
    }

    #[test]
    fn tuple_row_id_compiles() {
        let d = VecDelegate::new(&[((1u32, 7u32), "a"), ((1, 8), "b")]);
        assert_eq!(assert_delegate(&d), (1, 7));
        assert_eq!(d.row_index((1, 8)), Some(1));
    }
}
