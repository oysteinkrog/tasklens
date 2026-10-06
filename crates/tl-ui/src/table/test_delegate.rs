//! A minimal `TreeTableDelegate` over a list of named rows, for tests (E18-005).
//!
//! Every row is `Plain`, depth 0, not expandable and selectable. The only column is Name.
//! `C` picks the column id set: `ColumnId` (the default) or `SysColumnId`.

use std::fmt::Write as _;
use std::hash::Hash;

use gpui_kit::{AnyView, Context, SharedString, Window, px};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::table::{ServiceColumn, SortSpec, SysColumnId, SysSortSpec};

use crate::components::menu::TlMenu;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
};

/// A column id set `VecDelegate` can run on, with its sort type.
pub trait VecColumn: TableColumn {
    type Sort: Copy + PartialEq + 'static;
    /// The table the layout is built for.
    const TABLE: TableId;
    /// The Name column of this id set.
    const NAME: Self;
    fn sort_spec(column: Self, ascending: bool) -> Self::Sort;
    fn sort_parts(sort: Self::Sort) -> (Self, bool);
}

impl VecColumn for ColumnId {
    type Sort = SortSpec;
    const TABLE: TableId = TableId::Processes;
    const NAME: Self = ColumnId::Name;
    fn sort_spec(column: Self, ascending: bool) -> SortSpec {
        SortSpec { column, ascending }
    }
    fn sort_parts(sort: SortSpec) -> (Self, bool) {
        (sort.column, sort.ascending)
    }
}

impl VecColumn for SysColumnId {
    type Sort = SysSortSpec;
    const TABLE: TableId = TableId::Services;
    const NAME: Self = SysColumnId::Services(ServiceColumn::Name);
    fn sort_spec(column: Self, ascending: bool) -> SysSortSpec {
        SysSortSpec { column, ascending }
    }
    fn sort_parts(sort: SysSortSpec) -> (Self, bool) {
        (sort.column, sort.ascending)
    }
}

pub struct VecDelegate<R: Copy + Eq + Hash + 'static, C: VecColumn = ColumnId> {
    pub rows: Vec<(R, SharedString)>,
    pub index: FxHashMap<R, usize>,
    pub version: u64,
    pub layout: ColumnLayout<C>,
    pub spec: ColumnSpec<C>,
    pub sort: C::Sort,
    /// Every gesture the table reported, oldest first.
    pub activated: Vec<R>,
    pub toggled: Vec<R>,
    pub visible: Vec<R>,
}

impl<R: Copy + Eq + Hash + 'static> VecDelegate<R> {
    /// A process-table delegate over `ColumnId`.
    pub fn new(rows: &[(R, &'static str)]) -> Self {
        Self::with_columns(rows)
    }
}

impl<R: Copy + Eq + Hash + 'static, C: VecColumn> VecDelegate<R, C> {
    /// A delegate over the id set `C`, e.g. `VecDelegate::<u32, SysColumnId>::with_columns`.
    pub fn with_columns(rows: &[(R, &'static str)]) -> Self {
        let label = SharedString::new_static("Name");
        let spec = ColumnSpec {
            id: C::NAME,
            label: label.clone(),
            full_name: label.clone(),
            group: label,
            align: ColumnAlign::Left,
            kind: ColumnKind::Text,
            sortable: true,
            template: None,
            default_width: px(248.0),
            tooltip: None,
            availability: Availability::Available,
            hideable: false,
        };
        let layout = ColumnLayout::defaults(
            C::TABLE,
            std::slice::from_ref(&spec),
            &[C::NAME],
            &[C::NAME],
        );
        let mut d = Self {
            rows: Vec::new(),
            index: FxHashMap::default(),
            version: 0,
            layout,
            spec,
            sort: C::sort_spec(C::NAME, true),
            activated: Vec::new(),
            toggled: Vec::new(),
            visible: Vec::new(),
        };
        d.set_rows(rows);
        d
    }

    /// Replaces the rows, rebuilds the index and bumps the version.
    pub fn set_rows(&mut self, rows: &[(R, &'static str)]) {
        self.rows = rows
            .iter()
            .map(|&(id, name)| (id, SharedString::new_static(name)))
            .collect();
        self.index = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, r)| (r.0, ix))
            .collect();
        self.version += 1;
    }
}

impl<R: Copy + Eq + Hash + 'static, C: VecColumn> TreeTableDelegate for VecDelegate<R, C> {
    type RowId = R;
    type Column = C;
    type Sort = C::Sort;

    fn row_count(&self) -> usize {
        self.rows.len()
    }
    fn row_id(&self, ix: usize) -> R {
        self.rows[ix].0
    }
    fn row_index(&self, id: R) -> Option<usize> {
        self.index.get(&id).copied()
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Plain
    }
    fn row_depth(&self, _: usize) -> u8 {
        0
    }
    fn row_expandable(&self, _: usize) -> Option<bool> {
        None
    }
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<C> {
        &self.layout
    }
    fn column_spec(&self, _: C) -> &ColumnSpec<C> {
        &self.spec
    }
    fn cell(&self, ix: usize, _: C, out: &mut CellOut) {
        out.bits = ix as u64;
        if out.want_text {
            let _ = write!(out.text, "{}", self.rows[ix].1);
        }
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: self.rows[ix].1.clone(),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn header_total(&self, _: C, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> C::Sort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }
    fn context_menu(
        &mut self,
        _: &[R],
        _: C,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }
    fn on_toggle_expand(&mut self, id: R, _: &mut Context<TlTreeTable<Self>>) {
        self.toggled.push(id);
    }
    fn on_sort(&mut self, col: C, _: &mut Context<TlTreeTable<Self>>) {
        let (column, ascending) = C::sort_parts(self.sort);
        self.sort = C::sort_spec(col, !(column == col && ascending));
    }
    fn on_columns_changed(&mut self, layout: &ColumnLayout<C>, _: &mut Context<TlTreeTable<Self>>) {
        self.layout = layout.clone();
    }
    fn on_visible_range(&mut self, ids: &[R], _: &mut Context<TlTreeTable<Self>>) {
        self.visible.clear();
        self.visible.extend_from_slice(ids);
    }
    fn on_activate(&mut self, id: R, _: &mut Context<TlTreeTable<Self>>) {
        self.activated.push(id);
    }
    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }
    fn empty_text(&self) -> Option<SharedString> {
        Some(SharedString::new_static("No rows"))
    }
}
