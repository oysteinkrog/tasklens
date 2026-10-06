//! A minimal `TreeTableDelegate` over a list of named rows, for tests (E18-005).
//!
//! Every row is `Plain`, depth 0, not expandable and selectable. The only column is Name.

use std::fmt::Write as _;
use std::hash::Hash;

use gpui_kit::{AnyView, Context, SharedString, Window, px};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::table::SortSpec;

use crate::components::menu::TlMenu;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
};
use crate::table::tree_table::{
    IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
};

pub struct VecDelegate<R: Copy + Eq + Hash + 'static> {
    pub rows: Vec<(R, SharedString)>,
    pub index: FxHashMap<R, usize>,
    pub version: u64,
    pub layout: ColumnLayout,
    pub spec: ColumnSpec,
    pub sort: SortSpec,
    /// Every gesture the table reported, oldest first.
    pub activated: Vec<R>,
    pub toggled: Vec<R>,
    pub visible: Vec<R>,
}

impl<R: Copy + Eq + Hash + 'static> VecDelegate<R> {
    pub fn new(rows: &[(R, &'static str)]) -> Self {
        let label = SharedString::new_static("Name");
        let spec = ColumnSpec {
            id: ColumnId::Name,
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
            TableId::Processes,
            std::slice::from_ref(&spec),
            &[ColumnId::Name],
            &[ColumnId::Name],
        );
        let mut d = Self {
            rows: Vec::new(),
            index: FxHashMap::default(),
            version: 0,
            layout,
            spec,
            sort: SortSpec {
                column: ColumnId::Name,
                ascending: true,
            },
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

impl<R: Copy + Eq + Hash + 'static> TreeTableDelegate for VecDelegate<R> {
    type RowId = R;

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
    fn columns(&self) -> &ColumnLayout {
        &self.layout
    }
    fn column_spec(&self, _: ColumnId) -> &ColumnSpec {
        &self.spec
    }
    fn cell(&self, ix: usize, _: ColumnId, out: &mut CellOut) {
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
    fn header_total(&self, _: ColumnId, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> SortSpec {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }
    fn context_menu(
        &mut self,
        _: &[R],
        _: ColumnId,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }
    fn on_toggle_expand(&mut self, id: R, _: &mut Context<TlTreeTable<Self>>) {
        self.toggled.push(id);
    }
    fn on_sort(&mut self, col: ColumnId, _: &mut Context<TlTreeTable<Self>>) {
        self.sort.ascending = !(self.sort.column == col && self.sort.ascending);
        self.sort.column = col;
    }
    fn on_columns_changed(&mut self, layout: &ColumnLayout, _: &mut Context<TlTreeTable<Self>>) {
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
