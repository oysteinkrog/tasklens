//! The contract between `TlTreeTable` and the table owners (UI-PRC-001, UI-PRC-002, UI-PRC-003,
//! ADR-009, D-011, R-075, R-105).
//!
//! The delegate supplies finished rows, already sorted and filtered by the engine, and names
//! them by identity (`RowId`), never by index. The table draws them and reports gestures back.
//! Every geometry constant here is in logical px at 100% and is multiplied by `ui_scale` at use.

use std::cell::Cell;
use std::hash::{BuildHasher, Hash};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::ScrollbarHandle;

use gpui_kit::{
    AnyElement, AnyEntity, AnyView, App, Bounds, Context, EventEmitter, FocusHandle, FontWeight,
    Hsla, IntoElement, Pixels, Point, Render, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, WeakEntity, Window, point, size,
};
use rustc_hash::{FxBuildHasher, FxHashMap};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::table::{SortSpec, SysColumnId, SysSortSpec};
use tl_model::validity::Validity;

use crate::actions::ClearFilter;
use crate::components::menu::TlMenu;
use crate::components::tooltip::{DELAY_CONTROL, TlTooltip};
use crate::table::cells::CellOut;
use crate::table::columns::{ColumnError, ColumnKind, ColumnLayout, ColumnSpec, TableColumn};
use crate::table::columns_window::{self, ColumnHost, ColumnPicker, PickerColumn};
use crate::table::header::HeaderState;
use crate::table::selection::{Selection, SelectionView};
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
use crate::table::tree_core::QuadLogs;
use crate::table::tree_core::{
    Click, ColKey, CoreCall, DynRows, DynTreeDelegate, FrameLayout, Link, RowKey, RowMenu,
    TreeTableCore, row_fill,
};
use crate::table::tree_header::{
    self, ColCall, ColLink, ColumnSource, DynColumns, HeaderCore, Parts, live_layout,
};
use crate::theme::{TlTheme, current, metrics};

/// `tables.column_resize` (R-075). The one enum the settings parse.
pub use tl_ui_settings::settings::ColumnResize as ColumnResizeMode;

/// Name cell padding before tree level 0 (R-190).
pub const NAME_PAD: f32 = 20.0;
/// Indent per tree level (R-190).
pub const INDENT: f32 = 20.0;
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
/// Most row ids one visible-range demand report carries (ADR-008 `detail_keys`).
pub const DEMAND_MAX_IDS: usize = 512;
/// No scroll anchoring for this long after the last wheel event (UI-PRC-010).
pub const WHEEL_QUIET: Duration = Duration::from_millis(200);
/// How long the order stays held after the button is up, so the second press of a double-click
/// lands on the row of the first.
pub const PRESS_HOLD_AFTER_UP: Duration = Duration::from_millis(500);
/// The longest the order stays held when no release arrives (focus moved to another window or a
/// popup took the mouse-up).
pub const PRESS_HOLD_MAX: Duration = Duration::from_secs(3);

/// What one table owner supplies. Row indices are positions in the current flattened row list
/// and are only valid for the current `table_version`; anything kept across frames uses `RowId`.
pub trait TreeTableDelegate: 'static + Sized {
    /// Row identity. Never an index.
    type RowId: Copy + Eq + Hash + 'static;
    /// Column ids: `ColumnId` for the process tables, `SysColumnId` for the system tables.
    type Column: TableColumn;
    /// Sort state: `SortSpec` for the process tables, `SysSortSpec` for the system tables.
    type Sort: Clone + PartialEq + TableSort<Self::Column> + 'static;

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
    fn columns(&self) -> &ColumnLayout<Self::Column>;
    fn column_spec(&self, id: Self::Column) -> &ColumnSpec<Self::Column>;
    fn cell(&self, ix: usize, col: Self::Column, out: &mut CellOut);
    fn name_cell(&self, ix: usize) -> NameCell;
    /// The element at the right edge of row `ix`'s Name cell, such as the process window
    /// button (UI-PRC-003). The table asks only for a hovered or selected row. `focused`: the
    /// row holds the keyboard focus. The default is none.
    fn name_trailing(
        &mut self,
        _ix: usize,
        _hovered: bool,
        _selected: bool,
        _focused: bool,
        _window: &mut Window,
        _cx: &mut Context<TlTreeTable<Self>>,
    ) -> Option<AnyElement> {
        None
    }
    /// Fills the header totals line for `col`. False: no line 2 for this column.
    fn header_total(&self, col: Self::Column, out: &mut CellOut) -> bool;
    fn sort(&self) -> Self::Sort;
    /// False for group headers.
    fn selectable(&self, ix: usize) -> bool;
    /// The row menu for `targets`. `clicked_col` is the column under the pointer on a
    /// right-click, `None` when Shift+F10 or the Menu key opened it.
    fn context_menu(
        &mut self,
        targets: &[Self::RowId],
        clicked_col: Option<Self::Column>,
        menu: TlMenu,
        window: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu;
    /// The row menu closed, after any item it ran has been dispatched.
    fn on_menu_closed(&mut self, _cx: &mut Context<TlTreeTable<Self>>) {}
    fn on_toggle_expand(&mut self, id: Self::RowId, cx: &mut Context<TlTreeTable<Self>>);
    fn on_sort(&mut self, col: Self::Column, cx: &mut Context<TlTreeTable<Self>>);
    /// The delegate persists the layout.
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<Self::Column>,
        cx: &mut Context<TlTreeTable<Self>>,
    );
    /// Demand for the visible rows, at most 512 ids.
    fn on_visible_range(&mut self, ids: &[Self::RowId], cx: &mut Context<TlTreeTable<Self>>);
    /// Enter or double-click.
    fn on_activate(&mut self, id: Self::RowId, cx: &mut Context<TlTreeTable<Self>>);
    fn row_tooltip(&self, ix: usize) -> Option<AnyView>;
    /// Weight of row `ix`'s Name text. Normal unless a table marks a row, such as the main
    /// thread in the Threads tab (500).
    fn name_weight(&self, _ix: usize) -> FontWeight {
        FontWeight::NORMAL
    }
    /// Text for an empty table (UI-PRC-024).
    fn empty_text(&self) -> Option<SharedString>;

    /// True: an empty table shows `empty_text` as a `TlEmptyState` with "Clear filter" 96 px
    /// below the body top, instead of the centred label (UI-PRC-024).
    fn empty_clear_filter(&self) -> bool {
        false
    }

    /// The action the "Clear filter" button of the empty state dispatches. The shell's
    /// `ClearFilter` unless the owner has its own filter box.
    fn empty_clear_action(&self) -> Box<dyn gpui_kit::Action> {
        Box::new(ClearFilter)
    }

    /// True for a flat table: the Name column reserves no chevron box and its text lines up with
    /// the header label.
    fn flat_names(&self) -> bool {
        false
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::Two
    }

    /// True: no column picker and no header-menu items 2, 3 and 6. Resize and sort still work.
    fn columns_fixed(&self) -> bool {
        false
    }

    /// False: the table is no Tab stop. A static table that never scrolls sets this.
    fn tab_stop(&self) -> bool {
        true
    }

    /// The column that shows the Name cell. The default is the column type's pinned first
    /// column (Name on the process tables, none on the system tables). On `None` the table uses
    /// the first visible column.
    fn name_column(&self) -> Option<Self::Column> {
        self.columns()
            .order
            .iter()
            .copied()
            .find(|c| c.pinned_first())
    }

    /// Hover delay of the explicit cell tooltips (`CellOut::tooltip`). The process table uses
    /// 600 ms (UI-PRC-004).
    fn cell_tip_delay(&self) -> Duration {
        DELAY_CONTROL
    }

    /// The tooltip content for the explicit tooltip `tip` of a `col` cell. The default wraps at
    /// 360 px.
    fn cell_tip_content(&self, col: Self::Column, tip: SharedString) -> TlTooltip {
        let _ = col;
        TlTooltip::text(tip)
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

    /// Texts "Resize columns to fit content" makes room for in `col` even when no visible row
    /// shows them, such as every word of a fixed set (the process Type column). The default is
    /// none.
    fn fit_samples(&self, _col: Self::Column) -> Vec<Arc<str>> {
        Vec::new()
    }

    /// The NEW or EXITED tint drawn over the zebra of row `ix`, or `None`. The default is the
    /// theme's tint for the `NEW` and `EXITED` row flags.
    fn row_tint(&self, ix: usize, theme: &TlTheme) -> Option<Hsla> {
        flag_tint(self.row_flags(ix), theme)
    }

    /// Called once per frame before the body rows of `range` are built. The process table
    /// reads the clock for its tint fades here and asks for the next frame while one runs.
    fn before_rows(
        &mut self,
        _range: Range<usize>,
        _window: &mut Window,
        _cx: &mut Context<TlTreeTable<Self>>,
    ) {
    }
}

/// The column and direction of a sort state, for the header's sort arrow.
pub trait TableSort<C> {
    fn column(&self) -> C;
    fn ascending(&self) -> bool;
}

impl TableSort<ColumnId> for SortSpec {
    fn column(&self) -> ColumnId {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

impl TableSort<SysColumnId> for SysSortSpec {
    fn column(&self) -> SysColumnId {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

/// Opacity of the metric cells of a `STALE` row (UI-PRC-002).
pub const STALE_OPACITY: f32 = 0.6;

/// Top of the filtered-empty state below the body top, at 100% scale (UI-PRC-024).
pub const EMPTY_STATE_TOP: f32 = 96.0;

/// What the table tells its owner page. The page turns these into toasts: "Copied to clipboard"
/// and "Failed to copy to clipboard" (APP-044).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeTableEvent {
    /// A leaf row was double-clicked. Owners that open a page-specific Properties window handle
    /// this event; keyboard activation continues through the shared `OpenProperties` action.
    Activate { row_index: usize },
    /// "Copy rows as table" put `rows` rows on the clipboard.
    CopiedRows { rows: usize },
    /// The clipboard write failed. gpui's `write_to_clipboard` reports no error, so the table
    /// cannot emit this yet.
    CopyFailed,
    /// The left button that went down on a row is up again. Owners that held new rows back
    /// while [`TlTreeTable::pressed`] was set install them now.
    PressReleased,
}

impl<D: TreeTableDelegate> EventEmitter<TreeTableEvent> for TlTreeTable<D> {}

/// The generic virtualised tree table (UI-PRC-001). One entity per table; the owner installs
/// rows into `delegate` and calls [`TlTreeTable::on_new_version`].
///
/// Rendering cost per frame is bounded by the visible rows: the body is a `uniform_list` that
/// builds only the requested range, each row is one `div` with absolutely positioned cells at
/// the layout's precomputed offsets, and every cell text comes from `CellTextCache`.
///
/// This type holds the state its owners read by type: the delegate, the selection, the header
/// state and the column layouts. The logic lives elsewhere, so it is not compiled once per
/// delegate type. The body rows, cells, keyboard and pointer handling are in the non-generic
/// `TreeTableCore`, which reaches the delegate through `DynTreeDelegate`. The header and column
/// logic is in `tree_header`, generic over the column type only, which reaches it through
/// `DynColumns`.
pub struct TlTreeTable<D: TreeTableDelegate> {
    pub delegate: D,
    pub selection: Selection<D::RowId>,
    /// Set by the page during Ctrl-pause (UI-PRC-017). No anchoring while true.
    pub paused: bool,
    /// True from a left press on a row until the button is up. The owner keeps the rows still
    /// meanwhile, so the row under the pointer is the row pressed (UI-PRC-008).
    pub pressed: bool,
    core: TreeTableCore,
    shell: Shell<D>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub rows_built_last_frame: Cell<u32>,
    /// The column separator quads painted since the last render, in window coordinates.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub separator_quads: Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    /// The tree guide line quads painted since the last render, in window coordinates.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub guide_quads: Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    /// Row line canvases (guides and separators) built since the last render.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub line_canvases: Cell<u32>,
}

/// The typed table state the core does not hold.
struct Shell<D: TreeTableDelegate> {
    /// The header and column state.
    head: HeaderCore<D::Column>,
    /// The top-most visible selectable row of the last frame: its identity, its rows from the
    /// top visible row and the top row's hidden part. A new version keeps it at the same y.
    anchor: Option<(D::RowId, usize, Pixels)>,
    /// Order-free hash of the last id set sent to `on_visible_range`.
    demand_hash: u64,
    /// Reused buffer for the demand ids.
    demand_ids: Vec<D::RowId>,
    /// The core's row keys.
    rows: RowKeys<D::RowId>,
    /// The core's column keys.
    cols: ColKeys<D::Column>,
}

/// Row keys the shell keeps beyond twice the row count before it drops the keys of rows that
/// left the view.
const ROW_KEY_SLACK: usize = 256;

/// The [`RowKey`] of every row id the core has asked about. Keys count up and are never
/// reused. Once there are more than twice as many keys as rows plus [`ROW_KEY_SLACK`], a new
/// table version drops the keys of rows that are not in the view. The core keeps keys only in
/// its cell text cache and in the row listeners of the last frame, and both treat a key whose
/// row left the view like the row itself: no text, no hit.
struct RowKeys<R> {
    keys: FxHashMap<R, RowKey>,
    ids: FxHashMap<RowKey, R>,
    next: u64,
}

impl<R: Copy + Eq + Hash> RowKeys<R> {
    fn new() -> Self {
        Self {
            keys: FxHashMap::default(),
            ids: FxHashMap::default(),
            next: 0,
        }
    }

    fn key(&mut self, id: R) -> RowKey {
        let Self { keys, ids, next } = self;
        *keys.entry(id).or_insert_with(|| {
            let key = RowKey(*next);
            *next += 1;
            ids.insert(key, id);
            key
        })
    }

    fn id(&self, key: RowKey) -> Option<R> {
        self.ids.get(&key).copied()
    }

    /// Drops the keys of rows `in_view` rejects, when there are too many keys for `rows` rows.
    fn prune(&mut self, rows: usize, in_view: impl Fn(R) -> bool) {
        if self.keys.len() <= rows.saturating_mul(2).saturating_add(ROW_KEY_SLACK) {
            return;
        }
        self.keys.retain(|id, _| in_view(*id));
        self.ids.clear();
        self.ids
            .extend(self.keys.iter().map(|(&id, &key)| (key, id)));
    }
}

/// The [`ColKey`] of every column id the core has seen: an append-only registry, so a key
/// names the same column for the table's life.
struct ColKeys<C> {
    cols: Vec<C>,
    slots: FxHashMap<C, u32>,
    /// The last visible list converted. Holding it keeps its address from being reused, so
    /// pointer equality means the same list.
    visible_source: Option<Arc<[C]>>,
    /// `visible_source` as keys.
    visible_keys: Arc<[ColKey]>,
}

impl<C: TableColumn> ColKeys<C> {
    fn new() -> Self {
        Self {
            cols: Vec::new(),
            slots: FxHashMap::default(),
            visible_source: None,
            visible_keys: Arc::from([]),
        }
    }

    fn key(&mut self, col: C) -> ColKey {
        let Self { cols, slots, .. } = self;
        let slot = *slots.entry(col).or_insert_with(|| {
            cols.push(col);
            (cols.len() - 1) as u32
        });
        ColKey {
            slot,
            name: col.key(),
        }
    }

    fn col(&self, key: ColKey) -> C {
        self.cols[key.slot as usize]
    }

    /// `visible` as keys. Converts only when the list is not the one converted last.
    fn visible(&mut self, visible: &Arc<[C]>) -> Arc<[ColKey]> {
        if let Some(source) = &self.visible_source
            && Arc::ptr_eq(source, visible)
        {
            return self.visible_keys.clone();
        }
        let keys: Arc<[ColKey]> = visible.iter().map(|&c| self.key(c)).collect();
        self.visible_source = Some(visible.clone());
        self.visible_keys = keys.clone();
        keys
    }
}

impl<D: TreeTableDelegate> TlTreeTable<D> {
    pub fn new(delegate: D, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        let cols: ColLink<D::Column> = Rc::new(move |cx: &mut App, f: ColCall<'_, D::Column>| {
            let _ = weak.update(cx, |t, cx| t.with_cols(cx, |core, h| f(core, h)));
        });
        let link: Link = {
            let cols = cols.clone();
            Rc::new(move |cx: &mut App, f: CoreCall<'_>| cols(cx, &mut |core, h| f(core, h)))
        };
        Self {
            delegate,
            selection: Selection::new(),
            paused: false,
            pressed: false,
            core: TreeTableCore::new(link, cx.focus_handle()),
            shell: Shell {
                head: HeaderCore::new(cols),
                anchor: None,
                demand_hash: 0,
                demand_ids: Vec::new(),
                rows: RowKeys::new(),
                cols: ColKeys::new(),
            },
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            rows_built_last_frame: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            separator_quads: Rc::default(),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            guide_quads: Rc::default(),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            line_canvases: Cell::new(0),
        }
    }

    /// Runs `f` on the core with a host over the rest of the table and `cx`.
    fn with_host<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut TreeTableCore, &mut dyn DynTreeDelegate) -> R,
    ) -> R {
        self.with_cols(cx, |core, h| f(core, h))
    }

    /// [`Self::with_host`] with the typed column calls.
    fn with_cols<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut TreeTableCore, &mut dyn DynColumns<D::Column>) -> R,
    ) -> R {
        let Self {
            delegate,
            selection,
            paused,
            pressed,
            core,
            shell,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            rows_built_last_frame,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            separator_quads,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            guide_quads,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            line_canvases,
            ..
        } = self;
        let mut host = Host {
            delegate,
            selection,
            paused: *paused,
            pressed,
            shell,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            hooks: Hooks {
                rows_built: rows_built_last_frame,
                line_canvases,
                separators: separator_quads,
                guides: guide_quads,
            },
            cx,
        };
        f(core, &mut host)
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.core.focus
    }

    pub fn delegate_mut(&mut self) -> &mut D {
        &mut self.delegate
    }

    /// The layout the table draws: the live copy during a boundary drag, else the delegate's.
    pub fn columns(&self) -> &ColumnLayout<D::Column> {
        live_layout(&self.shell.head, &self.delegate)
    }

    /// The owner passes the current `tables.column_resize` value; the table reads no settings.
    pub fn set_resize_mode(&mut self, mode: ColumnResizeMode) {
        self.shell.head.resize_mode = mode;
    }

    /// The owner passes the current `tables.column_separators` value: 1 px divider lines
    /// between the body's columns (UI-PRC-002, R-075).
    pub fn set_column_separators(&mut self, on: bool) {
        self.core.column_separators = on;
    }

    /// The owner's default order, visible set and widths. With them the table handles
    /// `ResetColumns` itself; without them the action goes on to the owner.
    pub fn set_default_layout(&mut self, layout: ColumnLayout<D::Column>) {
        self.shell.head.default_layout = Some(layout);
    }

    /// The owner calls this after installing new rows: starts a new `CellTextCache` version,
    /// keeps the anchor row at its y and re-renders.
    pub fn on_new_version(&mut self, cx: &mut Context<Self>) {
        let m = *metrics(cx);
        self.sync_versions(m.style_version);
        self.with_host(cx, |core, host| core.keep_anchor(m.row_height, host));
        cx.notify();
    }

    /// Scrolls to the first row and drops the scroll anchor, so the next view stays at the top.
    /// The owner calls this when a filter change replaces the rows.
    pub fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.shell.anchor = None;
        let scroll = &self.core.scroll;
        let x = ScrollbarHandle::offset(scroll).x;
        ScrollbarHandle::set_offset(scroll, point(x, Pixels::ZERO));
        cx.notify();
    }

    /// Scrolls row `id` to the viewport centre and selects it (PROC-071). False when `id` is not
    /// in the view; the delegate expands its path before calling.
    pub fn jump_to(&mut self, id: D::RowId, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.delegate.row_index(id) else {
            return false;
        };
        self.core
            .scroll
            .scroll_to_item_strict(ix, ScrollStrategy::Center);
        self.selection.click(id);
        cx.notify();
        true
    }

    /// The row at the top of the viewport: its id and its view index at capture time. Reads the
    /// range the last frame recorded, so it costs nothing per frame (UI-SHL-130 step 5). `None`
    /// when the table has no rows.
    pub fn first_visible_row(&self) -> Option<(D::RowId, usize)> {
        let last = self.delegate.row_count().checked_sub(1)?;
        let ix = self.core.visible_range.start.min(last);
        Some((self.delegate.row_id(ix), ix))
    }

    /// Scrolls so the captured row is the first visible row again. When its id left the view,
    /// the captured index is used, clamped to the last row. Strict, because after a row height
    /// change the row may still be visible without being at the top.
    pub fn restore_first_visible_row(
        &mut self,
        (id, captured): (D::RowId, usize),
        cx: &mut Context<Self>,
    ) {
        let Some(last) = self.delegate.row_count().checked_sub(1) else {
            return;
        };
        let ix = self
            .delegate
            .row_index(id)
            .unwrap_or_else(|| captured.min(last));
        self.core
            .scroll
            .scroll_to_item_strict_with_offset(ix, ScrollStrategy::Top, 0);
        cx.notify();
    }

    /// The body rows the last frame built.
    pub fn visible_range(&self) -> Range<usize> {
        self.core.visible_range.clone()
    }

    /// The file icon image sources built so far, and how many the cache holds now.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub fn icon_sources(&self) -> (u32, usize) {
        (self.core.icons.built, self.core.icons.len())
    }

    /// A clone of the body's scroll handle. The handle is `Rc`-backed, so scrolling through the
    /// clone moves this table (R-186).
    pub fn scroll_handle(&self) -> UniformListScrollHandle {
        self.core.scroll.clone()
    }

    /// Hits and misses of the cell text cache since the table was created.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub fn text_cache_stats(&self) -> (u64, u64) {
        self.core.text_cache.stats()
    }

    /// How far header and body are scrolled left.
    pub fn x_offset(&self) -> Pixels {
        self.core.x_offset
    }

    /// The row under the pointer, by flattened index.
    pub fn hover_row(&self) -> Option<usize> {
        self.core.hover_row
    }

    /// True when the columns are wider than the body, so the horizontal scrollbar shows. Uses
    /// the body bounds of the last frame.
    pub fn horizontal_overflow(&self) -> bool {
        self.core.horizontal_overflow(self.columns().total_width())
    }

    fn sync_versions(&mut self, style_version: u64) {
        let version = self.delegate.table_version();
        let core = &mut self.core;
        if version != core.last_version || style_version != core.last_style_version {
            core.text_cache.begin_version(version, style_version);
            self.shell
                .head
                .state
                .totals
                .begin_version(version, style_version);
            core.icons.begin_version(version);
            core.last_version = version;
            core.last_style_version = style_version;
            let delegate = &self.delegate;
            self.shell
                .rows
                .prune(delegate.row_count(), |id| delegate.row_index(id).is_some());
        }
    }

    /// The fill of row `ix` (UI-PRC-002): the group band, else selected, else hover, else the
    /// zebra on the flattened index with the NEW or EXITED tint over it.
    pub fn row_background(&self, ix: usize, theme: &TlTheme) -> Hsla {
        row_fill(
            ix,
            self.delegate.row_kind(ix) == RowKind::GroupHeader,
            || self.selection.contains(&self.delegate.row_id(ix)),
            self.core.hover_row == Some(ix),
            || self.delegate.row_tint(ix, theme),
            theme,
        )
    }

    /// The labels of the visible columns joined by tabs, then one line per row in `ids` with
    /// each visible column's text. Cells copy the cached display text, the Name column its
    /// primary text and glyph cells their copy text (UI-FND-026). Group headers are left out.
    pub fn rows_as_tsv(&mut self, ids: &[D::RowId]) -> String {
        let rows: Vec<usize> = ids
            .iter()
            .filter_map(|&id| self.delegate.row_index(id))
            .collect();
        let Self {
            delegate,
            selection,
            paused,
            pressed,
            core,
            shell,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            rows_built_last_frame,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            separator_quads,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            guide_quads,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            line_canvases,
            ..
        } = self;
        let mut host = Host {
            delegate,
            selection,
            paused: *paused,
            pressed,
            shell,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            hooks: Hooks {
                rows_built: rows_built_last_frame,
                line_canvases,
                separators: separator_quads,
                guides: guide_quads,
            },
            cx: (),
        };
        core.rows_as_tsv(&rows, &mut host)
    }

    /// Header hover, press, totals and the cells of the last frame.
    pub fn header_state(&self) -> &HeaderState<D::Column> {
        &self.shell.head.state
    }

    /// The header total of `col` as line 2 shows it: the cached text and its validity. `None`
    /// when the delegate has no total for `col`. Formats only when the value or validity changed.
    pub fn header_total_text(&mut self, col: D::Column) -> Option<(SharedString, Validity)> {
        let totals = &mut self.shell.head.state.totals;
        tree_header::header_total_text(totals, &mut self.core.out, &self.delegate, col)
    }

    /// Asks the delegate to sort by `col` (header click and `SortByColumn`). The delegate picks
    /// the direction with `next_sort`. Nothing on an unsortable column.
    pub fn sort_by_column(&mut self, col: D::Column, cx: &mut Context<Self>) {
        self.with_cols(cx, |_, h| tree_header::sort_by_column(h, col));
    }
}

/// The column picker's view of a table (R-181). Every call on a dropped table does nothing.
impl<D: TreeTableDelegate> ColumnHost for WeakEntity<TlTreeTable<D>> {
    fn table_name(&self, cx: &App) -> SharedString {
        match self.upgrade() {
            Some(t) => columns_window::table_name(t.read(cx).delegate.columns().table, cx),
            None => SharedString::default(),
        }
    }

    fn columns(&self, cx: &App) -> Vec<PickerColumn> {
        match self.upgrade() {
            Some(t) => tree_header::picker_columns(&t.read(cx).delegate),
            None => Vec::new(),
        }
    }

    fn set_visible(
        &self,
        key: &str,
        on: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Result<(), ColumnError> {
        self.update(cx, |t, cx| {
            t.with_cols(cx, |_, h| tree_header::set_column_visible(h, key, on))
        })
        .unwrap_or(Err(ColumnError::Unknown))
    }

    fn reset(&self, window: &mut Window, cx: &mut App) {
        let reset = self.update(cx, |t, cx| {
            t.with_cols(cx, |_, h| tree_header::reset_columns(h, window))
        });
        if !matches!(reset, Ok(true)) {
            tracing::debug!(target: "tasklens::ui", "picker reset: the table has no defaults");
        }
    }

    fn observe(&self, picker: &mut Context<ColumnPicker>) -> Option<Subscription> {
        let table = self.upgrade()?;
        Some(picker.observe(&table, |_, _, cx| cx.notify()))
    }
}

/// The test-hook fields of the table, for the host.
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
struct Hooks<'a> {
    rows_built: &'a Cell<u32>,
    line_canvases: &'a Cell<u32>,
    separators: &'a Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    guides: &'a Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
}

/// The core's view of one table: the delegate, the selection and the typed shell state, and
/// `cx`, the table's `Context` (or `()` where the caller has none). Built per call by
/// [`TlTreeTable::with_host`].
struct Host<'a, D: TreeTableDelegate, X> {
    delegate: &'a mut D,
    selection: &'a mut Selection<D::RowId>,
    paused: bool,
    pressed: &'a mut bool,
    shell: &'a mut Shell<D>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    hooks: Hooks<'a>,
    cx: X,
}

impl<D: TreeTableDelegate, X> DynRows for Host<'_, D, X> {
    fn row_count(&self) -> usize {
        self.delegate.row_count()
    }
    fn row_key(&mut self, ix: usize) -> RowKey {
        let id = self.delegate.row_id(ix);
        self.shell.rows.key(id)
    }
    fn key_index(&self, key: RowKey) -> Option<usize> {
        self.shell
            .rows
            .id(key)
            .and_then(|id| self.delegate.row_index(id))
    }
    fn row_hash(&self, ix: usize) -> u64 {
        FxBuildHasher.hash_one(self.delegate.row_id(ix))
    }
    fn row_kind(&self, ix: usize) -> RowKind {
        self.delegate.row_kind(ix)
    }
    fn row_depth(&self, ix: usize) -> u8 {
        self.delegate.row_depth(ix)
    }
    fn row_expandable(&self, ix: usize) -> Option<bool> {
        self.delegate.row_expandable(ix)
    }
    fn row_flags(&self, ix: usize) -> RowFlags {
        self.delegate.row_flags(ix)
    }
    fn selectable(&self, ix: usize) -> bool {
        self.delegate.selectable(ix)
    }
    fn row_tint(&self, ix: usize, theme: &TlTheme) -> Option<Hsla> {
        self.delegate.row_tint(ix, theme)
    }
    fn row_tooltip(&self, ix: usize) -> Option<AnyView> {
        self.delegate.row_tooltip(ix)
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        self.delegate.name_cell(ix)
    }
    fn name_weight(&self, ix: usize) -> FontWeight {
        self.delegate.name_weight(ix)
    }
    fn cell(&self, ix: usize, col: ColKey, out: &mut CellOut) {
        self.delegate.cell(ix, self.shell.cols.col(col), out);
    }

    fn layout(&mut self) -> FrameLayout {
        let Shell { head, cols, .. } = &mut *self.shell;
        let layout = live_layout(head, &*self.delegate);
        FrameLayout {
            visible: cols.visible(layout.visible_columns()),
            offsets: layout.offsets().clone(),
        }
    }
    fn total_width(&self) -> Pixels {
        live_layout(&self.shell.head, &*self.delegate).total_width()
    }
    fn name_column(&mut self) -> Option<ColKey> {
        name_column(&*self.delegate).map(|c| self.shell.cols.key(c))
    }
    fn col_kind(&self, col: ColKey) -> ColumnKind {
        self.delegate.column_spec(self.shell.cols.col(col)).kind
    }
    fn col_label(&self, col: ColKey) -> SharedString {
        self.delegate
            .column_spec(self.shell.cols.col(col))
            .label
            .clone()
    }
    fn fit_samples(&self, col: ColKey) -> Vec<Arc<str>> {
        self.delegate.fit_samples(self.shell.cols.col(col))
    }
    fn cell_tip_delay(&self) -> Duration {
        self.delegate.cell_tip_delay()
    }
    fn cell_tip_content(&self, col: ColKey, tip: SharedString) -> TlTooltip {
        self.delegate
            .cell_tip_content(self.shell.cols.col(col), tip)
    }
    fn flat_names(&self) -> bool {
        self.delegate.flat_names()
    }
    fn guide_lines(&self) -> bool {
        self.delegate.guide_lines()
    }
    fn double_click_expands(&self) -> bool {
        self.delegate.double_click_expands()
    }
    fn empty_text(&self) -> Option<SharedString> {
        self.delegate.empty_text()
    }
    fn empty_clear_filter(&self) -> bool {
        self.delegate.empty_clear_filter()
    }
    fn empty_clear_action(&self) -> Box<dyn gpui_kit::Action> {
        self.delegate.empty_clear_action()
    }
    fn tab_stop(&self) -> bool {
        self.delegate.tab_stop()
    }

    fn is_selected(&self, ix: usize) -> bool {
        self.selection.contains(&self.delegate.row_id(ix))
    }
    fn is_focus(&self, ix: usize) -> bool {
        self.selection.focus() == Some(self.delegate.row_id(ix))
    }
    fn focus_index(&self) -> Option<usize> {
        self.selection
            .focus()
            .and_then(|id| self.delegate.row_index(id))
    }
    fn selection_empty(&self) -> bool {
        self.selection.is_empty()
    }
    fn has_focus_row(&self) -> bool {
        self.selection.focus().is_some()
    }
    fn click(&mut self, ix: usize, click: Click) {
        let id = self.delegate.row_id(ix);
        let v = DelegateView(&*self.delegate);
        match click {
            Click::Plain => self.selection.click(id),
            Click::Ctrl => self.selection.ctrl_click(id),
            Click::Shift => self.selection.shift_click(id, &v),
            Click::CtrlShift => self.selection.ctrl_shift_click(id, &v),
        }
    }
    fn right_click(&mut self, ix: usize) -> bool {
        let id = self.delegate.row_id(ix);
        self.selection.right_click(id)
    }
    fn clear_selection(&mut self) {
        self.selection.clear();
    }
    fn move_focus(&mut self, delta: i32, extend: bool) {
        let v = DelegateView(&*self.delegate);
        self.selection.move_focus(delta, extend, &v);
    }
    fn move_page(&mut self, rows: usize, down: bool, extend: bool) {
        let v = DelegateView(&*self.delegate);
        self.selection.move_page(rows, down, extend, &v);
    }
    fn move_edge(&mut self, end: bool, extend: bool) {
        let v = DelegateView(&*self.delegate);
        self.selection.move_edge(end, extend, &v);
    }
    fn select_all(&mut self) {
        let v = DelegateView(&*self.delegate);
        self.selection.select_all(&v);
    }
    fn toggle_focus(&mut self) {
        self.selection.toggle_focus();
    }
    fn selected_in_view(&self) -> SmallVec<[usize; 8]> {
        let v = DelegateView(&*self.delegate);
        self.selection
            .in_view_order(&v)
            .into_iter()
            .filter_map(|id| self.delegate.row_index(id))
            .collect()
    }

    fn paused(&self) -> bool {
        self.paused
    }
    fn pressed(&self) -> bool {
        *self.pressed
    }
    fn set_pressed(&mut self) {
        *self.pressed = true;
    }
    fn take_pressed(&mut self) -> bool {
        std::mem::take(self.pressed)
    }
    fn set_anchor(&mut self, anchor: Option<(usize, usize, Pixels)>) {
        self.shell.anchor =
            anchor.map(|(ix, from_top, sub)| (self.delegate.row_id(ix), from_top, sub));
    }
    fn anchor(&self) -> Option<(usize, usize, Pixels)> {
        let (id, from_top, sub) = self.shell.anchor?;
        Some((self.delegate.row_index(id)?, from_top, sub))
    }

    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    fn set_rows_built(&self, rows: u32) {
        self.hooks.rows_built.set(rows);
    }
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    fn count_line_canvas(&self) {
        let n = self.hooks.line_canvases;
        n.set(n.get() + 1);
    }
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    fn quad_logs(&self) -> QuadLogs {
        (self.hooks.guides.clone(), self.hooks.separators.clone())
    }
}

impl<D: TreeTableDelegate> DynTreeDelegate for Host<'_, D, &mut Context<'_, TlTreeTable<D>>> {
    fn app(&mut self) -> &mut App {
        self.cx
    }
    fn notify(&mut self) {
        self.cx.notify();
    }
    fn emit(&mut self, event: TreeTableEvent) {
        self.cx.emit(event);
    }
    fn keep_alive(&self) -> AnyEntity {
        self.cx.entity().into_any()
    }
    fn name_trailing(
        &mut self,
        ix: usize,
        hovered: bool,
        selected: bool,
        focused: bool,
        window: &mut Window,
    ) -> Option<AnyElement> {
        self.delegate
            .name_trailing(ix, hovered, selected, focused, window, self.cx)
    }
    fn before_rows(&mut self, range: Range<usize>, window: &mut Window) {
        self.delegate.before_rows(range, window, self.cx);
    }
    fn toggle_expand(&mut self, ix: usize) {
        let id = self.delegate.row_id(ix);
        self.delegate.on_toggle_expand(id, self.cx);
    }
    fn toggle_expand_key(&mut self, key: RowKey) {
        if let Some(id) = self.shell.rows.id(key) {
            self.delegate.on_toggle_expand(id, self.cx);
        }
    }
    fn activate(&mut self, ix: usize) {
        let id = self.delegate.row_id(ix);
        self.delegate.on_activate(id, self.cx);
    }
    fn row_menu(&mut self, targets: &[usize], col: Option<ColKey>) -> RowMenu {
        let targets: SmallVec<[D::RowId; 8]> =
            targets.iter().map(|&ix| self.delegate.row_id(ix)).collect();
        let col = col.map(|c| self.shell.cols.col(c));
        let table = self.cx.entity();
        let table_closed = table.clone();
        RowMenu {
            build: Box::new(move |menu, window, cx| {
                table.update(cx, |this, cx| {
                    this.delegate.context_menu(&targets, col, menu, window, cx)
                })
            }),
            closed: Rc::new(move |cx| {
                table_closed.update(cx, |this, cx| this.delegate.on_menu_closed(cx));
            }),
        }
    }
    /// Reports the visible rows plus one viewport above and below, at most
    /// [`DEMAND_MAX_IDS`], to the delegate when the id set changed (ADR-009 "Visible demand").
    fn report_demand(&mut self, visible: Range<usize>) {
        let count = self.delegate.row_count();
        let range = visible.start.min(count)..visible.end.min(count);
        let ids = demand_window(range, count);
        let shell = &mut *self.shell;
        shell.demand_ids.clear();
        let mut hash = ids.len() as u64;
        for ix in ids {
            let id = self.delegate.row_id(ix);
            hash = hash.wrapping_add(FxBuildHasher.hash_one(id));
            shell.demand_ids.push(id);
        }
        if hash == shell.demand_hash {
            return;
        }
        shell.demand_hash = hash;
        let ids = std::mem::take(&mut shell.demand_ids);
        self.delegate.on_visible_range(&ids, self.cx);
        self.shell.demand_ids = ids;
    }
}

impl<D: TreeTableDelegate> DynColumns<D::Column> for Host<'_, D, &mut Context<'_, TlTreeTable<D>>> {
    fn parts(&mut self) -> Parts<'_, D::Column> {
        Parts {
            head: &mut self.shell.head,
            src: &*self.delegate,
            app: self.cx,
        }
    }
    fn head_ref(&self) -> &HeaderCore<D::Column> {
        &self.shell.head
    }
    fn source(&self) -> &dyn ColumnSource<D::Column> {
        &*self.delegate
    }
    fn set_columns(&mut self, layout: &ColumnLayout<D::Column>) {
        self.delegate.on_columns_changed(layout, self.cx);
    }
    fn sort_by(&mut self, col: D::Column) {
        self.delegate.on_sort(col, self.cx);
    }
    fn col_key(&mut self, col: D::Column) -> ColKey {
        self.shell.cols.key(col)
    }
    fn picker_host(&self) -> Rc<dyn ColumnHost> {
        Rc::new(self.cx.weak_entity())
    }
}

/// The delegate as the header and column logic reads it.
impl<D: TreeTableDelegate> ColumnSource<D::Column> for D {
    fn col_layout(&self) -> &ColumnLayout<D::Column> {
        self.columns()
    }
    fn col_spec(&self, col: D::Column) -> &ColumnSpec<D::Column> {
        self.column_spec(col)
    }
    fn col_sort(&self) -> (D::Column, bool) {
        let sort = self.sort();
        (sort.column(), sort.ascending())
    }
    fn cols_fixed(&self) -> bool {
        self.columns_fixed()
    }
    fn col_lines(&self) -> HeaderLines {
        self.header_lines()
    }
    fn col_total(&self, col: D::Column, out: &mut CellOut) -> bool {
        self.header_total(col, out)
    }
}

/// The column separators of one body row at `origin`: a `line`-wide quad on the right edge of
/// every visible column, inside it, where the header draws its boundaries. `offsets` are the
/// visible columns' left edges.
pub fn separator_quads(
    offsets: &[Pixels],
    total_width: Pixels,
    origin: Point<Pixels>,
    row_h: Pixels,
    line: Pixels,
) -> impl Iterator<Item = Bounds<Pixels>> {
    offsets
        .iter()
        .skip(1)
        .copied()
        .chain((!offsets.is_empty()).then_some(total_width))
        .map(move |right| Bounds::new(point(origin.x + right - line, origin.y), size(line, row_h)))
}

/// The rows a demand report covers: `visible` widened by its own length above and below, cut to
/// [`DEMAND_MAX_IDS`] around the centre of `visible` when longer.
pub fn demand_window(visible: Range<usize>, count: usize) -> Range<usize> {
    let rows = visible.len();
    let start = visible.start.saturating_sub(rows);
    let end = visible.end.saturating_add(rows).min(count);
    if end.saturating_sub(start) <= DEMAND_MAX_IDS {
        return start..end;
    }
    let centre = visible.start + rows / 2;
    let first = centre
        .saturating_sub(DEMAND_MAX_IDS / 2)
        .clamp(start, end - DEMAND_MAX_IDS);
    first..first + DEMAND_MAX_IDS
}

/// The delegate's rows in display order, for the `Selection` gestures.
struct DelegateView<'a, D>(&'a D);

impl<D: TreeTableDelegate> SelectionView<D::RowId> for DelegateView<'_, D> {
    fn len(&self) -> usize {
        self.0.row_count()
    }
    fn id(&self, ix: usize) -> D::RowId {
        self.0.row_id(ix)
    }
    fn index_of(&self, id: D::RowId) -> Option<usize> {
        self.0.row_index(id)
    }
    fn selectable(&self, ix: usize) -> bool {
        self.0.selectable(ix)
    }
}

/// Opacity of a metric cell (every cell but the Name cell): [`STALE_OPACITY`] on a `STALE` row,
/// else 1. The Name cell is always drawn at full opacity.
pub fn metric_cell_opacity(flags: RowFlags) -> f32 {
    if flags.has(RowFlags::STALE) {
        STALE_OPACITY
    } else {
        1.0
    }
}

/// The theme's NEW or EXITED tint for a row's flags, if any (UI-PRC-002).
pub fn flag_tint(flags: RowFlags, theme: &TlTheme) -> Option<Hsla> {
    if flags.has(RowFlags::NEW) {
        Some(theme.derived.new_row_tint)
    } else if flags.has(RowFlags::EXITED) {
        Some(theme.derived.exited_row_tint)
    } else {
        None
    }
}

/// The column that shows the Name cell: `delegate.name_column()`, else the first visible column.
pub fn name_column<D: TreeTableDelegate>(delegate: &D) -> Option<D::Column> {
    delegate
        .name_column()
        .or_else(|| delegate.columns().visible_columns().first().copied())
}

impl<D: TreeTableDelegate> Render for TlTreeTable<D> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        self.sync_versions(m.style_version);
        self.with_cols(cx, |_, h| tree_header::follow_ui_scale(h, m.ui_scale));
        self.core.list_calls = 0;
        self.core.probe_gutter = None;
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        {
            self.separator_quads.borrow_mut().clear();
            self.guide_quads.borrow_mut().clear();
            self.line_canvases.set(0);
        }
        self.core.x_offset = -ScrollbarHandle::offset(&self.core.scroll).x;
        self.with_cols(cx, |core, h| {
            tree_header::render_table(h, core, &m, &theme, window)
        })
    }
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
    /// Bit k: the guide line at level k continues through this row. That line hangs from the
    /// ancestor at depth k to its children, so the bit is set when the ancestor at depth k + 1
    /// has a later sibling. Only bits below `depth - 1` are set.
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
    /// The row sits under a group header: the level-0 guide line hangs from the header's
    /// chevron centre, x 16 (UI-PRC-003).
    pub const IN_GROUP: u16 = 128;
    /// The Name cell's primary text uses the secondary text colour.
    pub const SECONDARY_NAME: u16 = 256;

    pub fn has(self, f: u16) -> bool {
        self.bits & f != 0
    }

    /// True when the guide line at level `depth` continues through this row. False past
    /// depth 63.
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

#[cfg(test)]
mod tests {
    use tl_model::ids::ColumnId;
    use tl_model::table::{ServiceColumn, SortSpec, SysColumnId, SysSortSpec};

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

    fn delegate_sort<D: TreeTableDelegate>(d: &D) -> D::Sort {
        d.sort()
    }

    #[test]
    fn process_delegate_sorts_by_sort_spec() {
        let d = VecDelegate::new(&[(1u32, "one")]);
        let sort: SortSpec = delegate_sort(&d);
        assert_eq!(sort.column, ColumnId::Name);
    }

    #[test]
    fn sys_column_delegate_compiles() {
        let mut d = VecDelegate::<u32, SysColumnId>::with_columns(&[(1, "a"), (2, "b")]);
        let name = SysColumnId::Services(ServiceColumn::Name);
        let sort: SysSortSpec = delegate_sort(&d);
        assert_eq!(
            sort,
            SysSortSpec {
                column: name,
                ascending: true
            }
        );
        assert_eq!(d.columns().order, vec![name]);
        assert_eq!(d.column_spec(name).id, name);
        let status = SysColumnId::Services(ServiceColumn::Status);
        d.sort = SysSortSpec {
            column: status,
            ascending: false,
        };
        assert_eq!(delegate_sort(&d).column, status);
        assert!(!delegate_sort(&d).ascending);
        let mut out = CellOut {
            want_text: true,
            ..CellOut::default()
        };
        d.cell(1, name, &mut out);
        assert_eq!(out.text, "b");
    }
}
