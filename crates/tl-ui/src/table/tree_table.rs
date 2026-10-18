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
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::base::{
    Scrollbar, ScrollbarAxis, ScrollbarHandle, ScrollbarMode, ScrollbarThumbStyle, v_flex,
};

use gpui_kit::{
    AnyElement, AnyView, App, BorrowAppContext as _, Bounds, ClipboardItem, ContentMask, Context,
    DismissEvent, Div, ElementId, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, ScrollStrategy, ScrollWheelEvent, SharedString, Size,
    StatefulInteractiveElement as _, Styled, Subscription, TextRun, UniformListScrollHandle,
    WeakEntity, Window, canvas, div, fill, font, point, px, size, uniform_list,
};
use rustc_hash::{FxBuildHasher, FxHashMap};
use smallvec::{SmallVec, smallvec};
use tl_model::ids::ColumnId;
use tl_model::table::{SortSpec, SysColumnId, SysSortSpec};
use tl_model::validity::Validity;

use crate::actions::ClearFilter;
use crate::components::empty::TlEmptyState;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_CONTROL, DELAY_ROW_NAME, HoverTooltip, TlTooltip};
use crate::components::validity::copy_text;
use crate::i18n::tr;
use crate::table::actions::{
    CollapseOrParent, CopyRowsAsTable, EnableAllColumns, ExpandOrChild, HideColumn, MoveFocus,
    MoveFocusEdge, MoveFocusPage, OpenColumnPicker, OpenContextMenu, OpenProperties, ResetColumns,
    ResizeColumnsToFit, SelectAll, SortByColumn, TABLE_CONTEXT, ToggleExpand, ToggleSelectFocus,
};
use crate::table::cells::{
    CellCtx, CellOut, IconSourceCache, attach_cell_tooltip, cell_background, cell_text_color,
    guide_quads, icon_px, name_indent, paint_group_header_cell, paint_name_cell, value_cell,
};
use crate::table::columns::{
    Availability, ColumnAlign, ColumnError, ColumnKind, ColumnLayout, ColumnSpec, TableColumn,
    fit_width, min_width,
};
use crate::table::columns_window::{self, ColumnHost, ColumnPicker, PickerColumn};
use crate::table::header::{
    self, HeaderCell, HeaderEvent, HeaderListener, HeaderPress, HeaderState, ReorderDrag,
    ResizeDrag, SORT_ARROW, TlTreeTableHeader, beyond_slop, header_menu, header_tooltip,
    insert_line, insertion_slot, sort_arrow_x,
};
use crate::table::selection::{Selection, SelectionView};
use crate::text::ellipsis::middle_ellipsis;
use crate::text::fonts::UI_FAMILY;
use crate::text::metrics::TextMetricsCache;
use crate::text::style::t_base;
use crate::text_cache::CellTextCache;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// `tables.column_resize` (R-075). The one enum the settings parse.
pub use tl_model::settings::ColumnResize as ColumnResizeMode;

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
}

impl<D: TreeTableDelegate> EventEmitter<TreeTableEvent> for TlTreeTable<D> {}

/// The generic virtualised tree table (UI-PRC-001). One entity per table; the owner installs
/// rows into `delegate` and calls [`TlTreeTable::on_new_version`].
///
/// Rendering cost per frame is bounded by the visible rows: the body is a `uniform_list` that
/// builds only the requested range, each row is one `div` with absolutely positioned cells at
/// the layout's precomputed offsets, and every cell text comes from `CellTextCache`.
pub struct TlTreeTable<D: TreeTableDelegate> {
    pub delegate: D,
    pub selection: Selection<D::RowId>,
    scroll: UniformListScrollHandle,
    /// Shared by header and body: how far the content is scrolled left. Read from `scroll` on
    /// every render.
    x_offset: Pixels,
    /// Created in `new`, never in render.
    focus: FocusHandle,
    text_cache: CellTextCache<D::RowId, D::Column>,
    /// Image sources of the Name cells' file icons, reused across frames.
    icons: IconSourceCache,
    /// Header hover, press, totals and this frame's header cells.
    header_state: HeaderState<D::Column>,
    /// Routes the header's pointer events to [`Self::header_event`]. Created once in `new`.
    header_listener: HeaderListener,
    /// `tables.column_resize`, passed in by the owner.
    resize_mode: ColumnResizeMode,
    /// `tables.column_separators`, passed in by the owner.
    column_separators: bool,
    /// The owner's default layout for "Reset to default". `None`: `ResetColumns` goes on to the
    /// owner.
    default_layout: Option<ColumnLayout<D::Column>>,
    /// Catches Esc before any key binding while a column reorder drag is in progress.
    reorder_esc: Option<Subscription>,
    hover_row: Option<usize>,
    /// Set by the page during Ctrl-pause (UI-PRC-017). No anchoring while true.
    pub paused: bool,
    /// The body rows the last frame built, by flattened index.
    visible_range: Range<usize>,
    /// The top-most visible selectable row of the last frame: its identity, its rows from the
    /// top visible row and the top row's hidden part. A new version keeps it at the same y.
    anchor: Option<(D::RowId, usize, Pixels)>,
    /// Executor time of the last wheel event over the body.
    last_wheel: Option<Instant>,
    /// True while the scrollbar thumb is dragged. Shared with the scrollbar's handle.
    thumb_dragging: Rc<Cell<bool>>,
    /// Order-free hash of the last id set sent to `on_visible_range`.
    demand_hash: u64,
    /// True while a demand check waits for the next frame.
    demand_scheduled: bool,
    /// Reused buffer for the demand ids.
    demand_ids: Vec<D::RowId>,
    last_version: u64,
    last_style_version: u64,
    /// Calls of the body's item builder in the current frame. The `uniform_list` calls it twice
    /// to measure one row before it asks for the visible range.
    list_calls: u8,
    /// Truncation flags of the hovered row's cells, by visible column position. Kept across
    /// frames so a cut cell gets its tooltip on the next render.
    hover_clips: Vec<Rc<Cell<bool>>>,
    /// Shared truncation flag of every cell outside the hovered row. Never read.
    clip_sink: Rc<Cell<bool>>,
    /// False until the hovered row has rendered once and set its truncation flags.
    hover_tips_ready: bool,
    /// Scratch for `delegate.cell`, reused for every cell.
    out: CellOut,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub rows_built_last_frame: Cell<u32>,
    /// The column separator quads painted since the last render, in window coordinates.
    #[cfg(any(test, feature = "test-hooks"))]
    pub separator_quads: Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    /// The tree guide line quads painted since the last render, in window coordinates.
    #[cfg(any(test, feature = "test-hooks"))]
    pub guide_quads: Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    /// Row line canvases (guides and separators) built since the last render.
    #[cfg(any(test, feature = "test-hooks"))]
    pub line_canvases: Cell<u32>,
}

/// What every row of one frame shares.
struct FrameCtx<C> {
    m: UiMetrics,
    theme: Arc<TlTheme>,
    hairline: Pixels,
    total_width: Pixels,
    guide_lines: bool,
    separators: bool,
    visible: Arc<[C]>,
    offsets: Arc<[Pixels]>,
    name_col: Option<C>,
}

impl<D: TreeTableDelegate> TlTreeTable<D> {
    pub fn new(delegate: D, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            delegate,
            selection: Selection::new(),
            scroll: UniformListScrollHandle::new(),
            x_offset: Pixels::ZERO,
            focus: cx.focus_handle(),
            text_cache: CellTextCache::new(),
            icons: IconSourceCache::new(),
            header_state: HeaderState::default(),
            header_listener: {
                let this = cx.weak_entity();
                Rc::new(move |ev, window, cx| {
                    let _ = this.update(cx, |t, cx| t.header_event(ev, window, cx));
                })
            },
            resize_mode: ColumnResizeMode::default(),
            column_separators: false,
            default_layout: None,
            reorder_esc: None,
            hover_row: None,
            paused: false,
            visible_range: 0..0,
            anchor: None,
            last_wheel: None,
            thumb_dragging: Rc::new(Cell::new(false)),
            demand_hash: 0,
            demand_scheduled: false,
            demand_ids: Vec::new(),
            last_version: 0,
            last_style_version: 0,
            list_calls: 0,
            hover_clips: Vec::new(),
            clip_sink: Rc::new(Cell::new(false)),
            hover_tips_ready: false,
            out: CellOut::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            rows_built_last_frame: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            separator_quads: Rc::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            guide_quads: Rc::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            line_canvases: Cell::new(0),
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn delegate_mut(&mut self) -> &mut D {
        &mut self.delegate
    }

    /// The layout the table draws: the live copy during a boundary drag, else the delegate's.
    pub fn columns(&self) -> &ColumnLayout<D::Column> {
        match &self.header_state.resize {
            Some(drag) => &drag.layout,
            None => self.delegate.columns(),
        }
    }

    /// The owner passes the current `tables.column_resize` value; the table reads no settings.
    pub fn set_resize_mode(&mut self, mode: ColumnResizeMode) {
        self.resize_mode = mode;
    }

    /// The owner passes the current `tables.column_separators` value: 1 px divider lines
    /// between the body's columns (UI-PRC-002, R-075).
    pub fn set_column_separators(&mut self, on: bool) {
        self.column_separators = on;
    }

    /// The owner's default order, visible set and widths. With them the table handles
    /// `ResetColumns` itself; without them the action goes on to the owner.
    pub fn set_default_layout(&mut self, layout: ColumnLayout<D::Column>) {
        self.default_layout = Some(layout);
    }

    /// The owner calls this after installing new rows: starts a new `CellTextCache` version,
    /// keeps the anchor row at its y and re-renders.
    pub fn on_new_version(&mut self, cx: &mut Context<Self>) {
        let m = *metrics(cx);
        self.sync_versions(m.style_version);
        self.keep_anchor(m.row_height, cx);
        cx.notify();
    }

    /// Scroll anchoring (UI-PRC-010): the top-most visible selectable row of the last frame
    /// keeps its y, sub-row offset included. When it is gone the scroll position stays. Skipped
    /// during Ctrl-pause, a thumb drag, and for [`WHEEL_QUIET`] after a wheel event.
    fn keep_anchor(&mut self, row_h: Pixels, cx: &mut Context<Self>) {
        if self.paused || self.thumb_dragging.get() {
            return;
        }
        if let Some(t) = self.last_wheel
            && cx.background_executor().now().saturating_duration_since(t) < WHEEL_QUIET
        {
            return;
        }
        let Some((id, rows_from_top, sub)) = self.anchor else {
            return;
        };
        let Some(ix) = self.delegate.row_index(id) else {
            return;
        };
        let top = (row_h * ix.saturating_sub(rows_from_top) + sub).max(Pixels::ZERO);
        let x = ScrollbarHandle::offset(&self.scroll).x;
        ScrollbarHandle::set_offset(&self.scroll, point(x, -top));
    }

    /// Scrolls to the first row and drops the scroll anchor, so the next view stays at the top.
    /// The owner calls this when a filter change replaces the rows.
    pub fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.anchor = None;
        let x = ScrollbarHandle::offset(&self.scroll).x;
        ScrollbarHandle::set_offset(&self.scroll, point(x, Pixels::ZERO));
        cx.notify();
    }

    /// Scrolls row `id` to the viewport centre and selects it (PROC-071). False when `id` is not
    /// in the view; the delegate expands its path before calling.
    pub fn jump_to(&mut self, id: D::RowId, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.delegate.row_index(id) else {
            return false;
        };
        self.scroll
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
        let ix = self.visible_range.start.min(last);
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
        self.scroll
            .scroll_to_item_strict_with_offset(ix, ScrollStrategy::Top, 0);
        cx.notify();
    }

    /// The body rows the last frame built.
    pub fn visible_range(&self) -> Range<usize> {
        self.visible_range.clone()
    }

    /// The file icon image sources built so far, and how many the cache holds now.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn icon_sources(&self) -> (u32, usize) {
        (self.icons.built, self.icons.len())
    }

    /// After a keyboard focus move: scrolls the least amount that keeps one row of margin
    /// between the focus row and the header or the bottom edge. gpui's `Nearest` with an offset
    /// keeps the margin at the top only, so the bottom case targets the row below.
    fn keep_focus_visible(&self, cx: &mut Context<Self>) {
        let Some(ix) = self.focus_index() else {
            return;
        };
        let row_h = metrics(cx).row_height;
        let scroll_top = -ScrollbarHandle::offset(&self.scroll).y;
        if row_h * ix < scroll_top + row_h {
            self.scroll
                .scroll_to_item_with_offset(ix, ScrollStrategy::Nearest, 1);
        } else {
            let below = (ix + 1).min(self.delegate.row_count().saturating_sub(1));
            self.scroll.scroll_to_item(below, ScrollStrategy::Nearest);
        }
    }

    /// Reports the visible rows plus one viewport above and below, at most
    /// [`DEMAND_MAX_IDS`], to the delegate when the id set changed (ADR-009 "Visible demand").
    fn report_demand(&mut self, cx: &mut Context<Self>) {
        self.demand_scheduled = false;
        let count = self.delegate.row_count();
        let range = self.visible_range.start.min(count)..self.visible_range.end.min(count);
        let ids = demand_window(range, count);
        self.demand_ids.clear();
        let mut hash = ids.len() as u64;
        for ix in ids {
            let id = self.delegate.row_id(ix);
            hash = hash.wrapping_add(FxBuildHasher.hash_one(id));
            self.demand_ids.push(id);
        }
        if hash == self.demand_hash {
            return;
        }
        self.demand_hash = hash;
        let ids = std::mem::take(&mut self.demand_ids);
        self.delegate.on_visible_range(&ids, cx);
        self.demand_ids = ids;
    }

    /// A clone of the body's scroll handle. The handle is `Rc`-backed, so scrolling through the
    /// clone moves this table (R-186).
    pub fn scroll_handle(&self) -> UniformListScrollHandle {
        self.scroll.clone()
    }

    /// Hits and misses of the cell text cache since the table was created.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn text_cache_stats(&self) -> (u64, u64) {
        self.text_cache.stats()
    }

    /// How far header and body are scrolled left.
    pub fn x_offset(&self) -> Pixels {
        self.x_offset
    }

    /// The row under the pointer, by flattened index.
    pub fn hover_row(&self) -> Option<usize> {
        self.hover_row
    }

    /// True when the columns are wider than the body, so the horizontal scrollbar shows. Uses
    /// the body bounds of the last frame.
    pub fn horizontal_overflow(&self) -> bool {
        let viewport = ScrollbarHandle::viewport_bounds(&self.scroll).size.width;
        viewport > Pixels::ZERO && self.columns().total_width() > viewport
    }

    fn sync_versions(&mut self, style_version: u64) {
        let version = self.delegate.table_version();
        if version != self.last_version || style_version != self.last_style_version {
            self.text_cache.begin_version(version, style_version);
            self.header_state
                .totals
                .begin_version(version, style_version);
            self.icons.begin_version(version);
            self.last_version = version;
            self.last_style_version = style_version;
        }
    }

    fn set_hover(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        if row != self.hover_row {
            self.hover_row = row;
            self.hover_tips_ready = false;
            for flag in &self.hover_clips {
                flag.set(false);
            }
            cx.notify();
        } else if row.is_some() && !self.hover_tips_ready {
            // The hovered row has now set its truncation flags; render once more so cut cells
            // get their tooltips.
            self.hover_tips_ready = true;
            cx.notify();
        }
    }

    /// The fill of row `ix` (UI-PRC-002): the group band, else selected, else hover, else the
    /// zebra on the flattened index with the NEW or EXITED tint over it.
    pub fn row_background(&self, ix: usize, theme: &TlTheme) -> Hsla {
        if self.delegate.row_kind(ix) == RowKind::GroupHeader {
            return theme.header;
        }
        if self.selection.contains(&self.delegate.row_id(ix)) {
            return theme.row_selected;
        }
        if self.hover_row == Some(ix) {
            return theme.row_hover;
        }
        let zebra = if ix.is_multiple_of(2) {
            theme.list_bg
        } else {
            theme.row_alt
        };
        match self.delegate.row_tint(ix, theme) {
            Some(tint) => zebra.blend(tint),
            None => zebra,
        }
    }

    /// Builds the rows of `range` for the body. The two measuring calls get an empty row of the
    /// same size.
    fn build_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let call = self.list_calls;
        self.list_calls = self.list_calls.saturating_add(1);
        let m = *metrics(cx);
        let total_width = self.columns().total_width();
        if call < 2 && range.len() == 1 {
            return vec![div().w(total_width).h(m.row_height).into_any_element()];
        }
        #[cfg(any(test, feature = "test-hooks"))]
        self.rows_built_last_frame.set(range.len() as u32);
        self.record_frame(range.clone(), m.row_height);
        self.delegate.before_rows(range.clone(), window, cx);

        let layout = self.columns();
        let frame = FrameCtx {
            m,
            theme: current(cx).clone(),
            hairline: UiMetrics::hairline(window),
            total_width,
            guide_lines: self.delegate.guide_lines(),
            separators: self.column_separators,
            visible: layout.visible_columns().clone(),
            offsets: layout.offsets().clone(),
            name_col: name_column(&self.delegate),
        };
        let columns = frame.visible.len();
        if self.hover_clips.len() < columns {
            self.hover_clips
                .resize_with(columns, || Rc::new(Cell::new(false)));
        }
        let mut rows = Vec::with_capacity(range.len());
        for ix in range {
            rows.push(self.build_row(ix, &frame, window, cx));
        }
        rows
    }

    /// Keeps the frame's row range and anchor row. Runs inside the list's prepaint, after any
    /// pending scroll was applied.
    fn record_frame(&mut self, range: Range<usize>, row_h: Pixels) {
        let scroll_top = -ScrollbarHandle::offset(&self.scroll).y;
        let sub = (scroll_top - row_h * range.start).max(Pixels::ZERO);
        // At the very top there is nothing to hold: a new view must show its own first rows,
        // not follow the old top row to wherever the sort moved it.
        self.anchor = if scroll_top <= Pixels::ZERO {
            None
        } else {
            range
                .clone()
                .find(|&ix| self.delegate.selectable(ix))
                .map(|ix| (self.delegate.row_id(ix), ix - range.start, sub))
        };
        self.visible_range = range;
    }

    fn build_row(
        &mut self,
        ix: usize,
        f: &FrameCtx<D::Column>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = &f.m;
        let theme = &*f.theme;
        let kind = self.delegate.row_kind(ix);
        let mut row = div()
            .relative()
            .w(f.total_width)
            .h(m.row_height)
            .bg(self.row_background(ix, theme));

        let visible = &f.visible;
        let offsets = &f.offsets;
        let name_col = f.name_col;

        let id = self.delegate.row_id(ix);
        row = row.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                this.row_mouse_down(id, ev, window, cx);
            }),
        );

        if kind == RowKind::GroupHeader {
            row = row.debug_selector(|| "TlTreeTable-group-row".into());
            if ix > 0 {
                row = row.child(
                    div()
                        .debug_selector(|| "TlTreeTable-group-line".into())
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(f.total_width)
                        .h(f.hairline)
                        .bg(theme.divider),
                );
            }
            if let Some(pos) = name_col.and_then(|c| visible.iter().position(|v| *v == c)) {
                let x = offsets[pos];
                let ctx = CellCtx {
                    m,
                    theme,
                    selected: false,
                    width: f.total_width - x,
                };
                let name = self.delegate.name_cell(ix);
                let cell = paint_group_header_cell(name, self.delegate.row_expandable(ix), &ctx);
                row = row.child(cell.absolute().top_0().left(x));
            }
            return row.into_any_element();
        }

        let selected = self.selection.contains(&id);
        let hovered = self.hover_row == Some(ix);
        // Cell backgrounds show only under the zebra; hover and selection cover them.
        let plain = !selected && !hovered;
        let flags = self.delegate.row_flags(ix);
        let stale = flags.has(RowFlags::STALE);
        let tips = hovered && self.hover_tips_ready;
        let row_key = FxBuildHasher.hash_one(id);
        // The Name cell's tree guides, when this row draws any: (depth, cell left, cell width).
        let mut guides = None;

        row = row.on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
            this.set_hover(Some(ix), cx);
        }));

        for (pos, &col) in visible.iter().enumerate() {
            let x = offsets[pos];
            let width = offsets.get(pos + 1).copied().unwrap_or(f.total_width) - x;
            let ctx = CellCtx {
                m,
                theme,
                selected,
                width,
            };
            if Some(col) == name_col {
                let mut name = self.delegate.name_cell(ix);
                if (hovered || selected)
                    && let Some(trailing) = self.delegate.name_trailing(
                        ix,
                        hovered,
                        selected,
                        self.selection.focus() == Some(id),
                        window,
                        cx,
                    )
                {
                    name.trailing = Some(trailing);
                }
                let depth = self.delegate.row_depth(ix);
                let expandable = self.delegate.row_expandable(ix);
                if f.guide_lines && (depth > 0 || flags.ancestor_continues != 0) {
                    guides = Some((depth, x, width));
                }
                let mut cell = paint_name_cell(
                    name,
                    self.delegate.name_weight(ix),
                    depth,
                    expandable,
                    flags,
                    &ctx,
                    tips.then_some(row_key),
                    &mut self.icons,
                    window,
                    cx,
                );
                if tips && let Some(view) = self.delegate.row_tooltip(ix) {
                    let key =
                        ElementId::NamedInteger(SharedString::new_static("tl-row-tip"), row_key);
                    let state = HoverTooltip::state(key, window, cx);
                    cell = HoverTooltip::attach(
                        &state,
                        cell,
                        DELAY_ROW_NAME,
                        move |_, _| view.clone(),
                        window,
                        cx,
                    );
                }
                row = row.child(cell.absolute().top_0().left(x));
                if let Some(expanded) = expandable {
                    let left = x + name_indent(depth, m.ui_scale);
                    row = row.child(
                        self.chevron_hit(id, expanded, left, hovered, row_key, m, window, cx),
                    );
                }
                continue;
            }

            let (mut cell, text) = self.build_cell(ix, id, col, pos, hovered, &ctx, window);
            if let Some(color) = cell_text_color(&self.out, &ctx) {
                cell = cell.text_color(color);
            }
            // The row fill under the cell already holds the tint, so heat lies over it.
            if plain && let Some(bg) = cell_background(&self.out, &ctx) {
                cell = cell.bg(bg);
            }
            if stale {
                cell = cell.opacity(metric_cell_opacity(flags));
            }
            if tips {
                cell = self.attach_cell_tip(cell, col, pos, text, row_key, window, cx);
            }
            row = row.child(cell.absolute().top_0().left(x));
        }
        if f.separators || guides.is_some() {
            // Painted after the cells so the cell fills do not cover the lines.
            row = row.child(self.line_canvas(f, flags, guides));
        }
        if !selected && self.selection.focus() == Some(id) {
            // The keyboard caret on a row that is not selected (UI-PRC-008).
            row = row.child(
                div()
                    .debug_selector(|| "TlTreeTable-focus-row".into())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .border(px(m.ui_scale))
                    .border_color(theme.accent)
                    .rounded(m.radius_small),
            );
        }
        row.into_any_element()
    }

    /// The row's lines: one canvas whose paint draws the Name cell's tree guides (`guides` is
    /// its depth, left edge and width, [`guide_quads`]), clipped to that cell, then one column
    /// separator per boundary when they are on.
    fn line_canvas(
        &self,
        f: &FrameCtx<D::Column>,
        flags: RowFlags,
        guides: Option<(u8, Pixels, Pixels)>,
    ) -> impl IntoElement {
        let offsets = f.separators.then(|| f.offsets.clone());
        let total_width = f.total_width;
        let line = f.hairline;
        let row_h = f.m.row_height;
        let ui_scale = f.m.ui_scale;
        let divider = f.theme.divider;
        #[cfg(any(test, feature = "test-hooks"))]
        let (guide_log, separator_log) = {
            self.line_canvases.set(self.line_canvases.get() + 1);
            (self.guide_quads.clone(), self.separator_quads.clone())
        };
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                if let Some((depth, left, width)) = guides {
                    let origin = bounds.origin + point(left, Pixels::ZERO);
                    let cell = Bounds::new(origin, size(width, row_h));
                    let quads =
                        guide_quads(depth, flags, origin, row_h, ui_scale, window.scale_factor());
                    window.with_content_mask(Some(ContentMask { bounds: cell }), |window| {
                        for q in quads {
                            #[cfg(any(test, feature = "test-hooks"))]
                            guide_log.borrow_mut().push(q);
                            window.paint_quad(fill(q, divider));
                        }
                    });
                }
                if let Some(offsets) = &offsets {
                    for q in separator_quads(
                        offsets,
                        total_width,
                        bounds.origin,
                        bounds.size.height,
                        line,
                    ) {
                        #[cfg(any(test, feature = "test-hooks"))]
                        separator_log.borrow_mut().push(q);
                        window.paint_quad(fill(q, divider));
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// The chevron's 20 px hit area over the Name cell. `left` is the chevron box's left edge in
    /// the row. A press toggles the row without changing the selection (UI-PRC-002). The
    /// Expand or Collapse tooltip is attached on the hovered row only.
    #[allow(clippy::too_many_arguments)]
    fn chevron_hit(
        &self,
        id: D::RowId,
        expanded: bool,
        left: Pixels,
        hovered: bool,
        row_key: u64,
        m: &UiMetrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = m.ui_scale;
        let hit = div()
            .debug_selector(|| "TlTreeTable-chevron".into())
            .absolute()
            .top_0()
            .left(left - px((CHEVRON_HIT_W - CHEVRON) / 2.0 * s))
            .w(px(CHEVRON_HIT_W * s))
            .h(m.row_height)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    window.focus(&this.focus, cx);
                    this.delegate.on_toggle_expand(id, cx);
                }),
            );
        if !hovered {
            return hit;
        }
        let key = ElementId::NamedInteger(SharedString::new_static("tl-chevron-tip"), row_key);
        let label = if expanded {
            "fnd-table-collapse"
        } else {
            "fnd-table-expand"
        };
        let state = HoverTooltip::state(key, window, cx);
        HoverTooltip::attach(
            &state,
            hit,
            DELAY_CONTROL,
            move |_, cx| TlTooltip::text(tr(cx, label)).view(cx),
            window,
            cx,
        )
    }

    /// One metric cell and the text it shows. Leaves the delegate's output in `self.out` for the
    /// caller's overrides.
    #[allow(clippy::too_many_arguments)]
    fn build_cell(
        &mut self,
        ix: usize,
        id: D::RowId,
        col: D::Column,
        pos: usize,
        hovered: bool,
        ctx: &CellCtx,
        window: &mut Window,
    ) -> (Div, SharedString) {
        self.out.reset();
        // Only the hovered row attaches tooltips, so only it asks for the ones that allocate.
        self.out.want_tooltip = hovered && self.hover_tips_ready;
        self.delegate.cell(ix, col, &mut self.out);
        let kind = self.delegate.column_spec(col).kind;
        let flag = if hovered {
            self.hover_clips[pos].clone()
        } else {
            self.clip_sink.clone()
        };
        let text = match self.out.arc.clone() {
            Some(arc) if kind == ColumnKind::Path => {
                let room = (ctx.width - ctx.m.sp[4] * 2.0).max(Pixels::ZERO);
                let font_size = ctx.m.text.base;
                self.text_cache.path(id, col, room, &arc, |source, max| {
                    let mut measure = |s: &str| -> Pixels {
                        let run = TextRun {
                            len: s.len(),
                            font: font(UI_FAMILY),
                            color: ctx.theme.text_primary,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        window
                            .text_system()
                            .shape_line(SharedString::new(s), font_size, &[run], None)
                            .width
                    };
                    middle_ellipsis(source, max, &mut measure)
                })
            }
            Some(arc) => self.text_cache.get_or_insert_arc(id, col, &arc),
            None => self.formatted_text(ix, id, col),
        };
        let cell = value_cell(kind, text.clone(), &self.out, flag, ctx);
        (cell, text)
    }

    /// The cached display text of a cell whose delegate output in `self.out` has no `arc`.
    /// Formats it on a miss.
    fn formatted_text(&mut self, ix: usize, id: D::RowId, col: D::Column) -> SharedString {
        let delegate = &self.delegate;
        self.text_cache
            .get_or_format(id, col, self.out.bits, self.out.validity, |s| {
                let mut full = CellOut {
                    text: std::mem::take(s),
                    want_text: true,
                    ..CellOut::default()
                };
                delegate.cell(ix, col, &mut full);
                *s = full.text;
            })
    }

    /// The hover tooltip of a metric cell on the hovered row: the delegate's explicit tooltip,
    /// else the full text when the cell was cut. `text` is the text the cell shows.
    #[allow(clippy::too_many_arguments)]
    fn attach_cell_tip(
        &self,
        cell: Div,
        col: D::Column,
        pos: usize,
        text: SharedString,
        row_key: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> Div {
        let key = ElementId::NamedInteger(SharedString::new_static(col.key()), row_key);
        let path_cut = match &self.out.arc {
            Some(arc) if self.delegate.column_spec(col).kind == ColumnKind::Path => {
                (text.as_str() != &**arc).then(|| SharedString::from(arc.clone()))
            }
            _ => None,
        };
        if let Some(tip) = self.out.tooltip.clone() {
            let state = HoverTooltip::state(key, window, cx);
            return HoverTooltip::attach(
                &state,
                cell,
                self.delegate.cell_tip_delay(),
                self.delegate.cell_tip_content(col, tip).build(),
                window,
                cx,
            );
        }
        if let Some(tip) = path_cut {
            let state = HoverTooltip::state(key, window, cx);
            return HoverTooltip::attach(
                &state,
                cell,
                DELAY_CONTROL,
                TlTooltip::text(tip).build(),
                window,
                cx,
            );
        }
        attach_cell_tooltip(cell, key, text, &self.hover_clips[pos], window, cx)
    }

    /// Left press on the row `id` (UI-PRC-008, UI-PRC-002). A press on a group header toggles
    /// it and never touches the selection: the plain `Selection` gestures cannot tell a header
    /// from a row.
    fn row_mouse_down(
        &mut self,
        id: D::RowId,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        let Some(ix) = self.delegate.row_index(id) else {
            return;
        };
        let double = ev.click_count == 2;
        if self.delegate.row_kind(ix) == RowKind::GroupHeader {
            // The first press of a double-click has toggled already; the second does nothing, so
            // a double-click toggles once.
            if !double {
                self.delegate.on_toggle_expand(id, cx);
            }
            return;
        }
        if double {
            if !self.delegate.double_click_expands() {
                cx.emit(TreeTableEvent::Activate { row_index: ix });
                self.delegate.on_activate(id, cx);
                // A delegate that propagates for the keyboard path must not let the press reach
                // the body, which would clear the selection Properties opens on.
                cx.stop_propagation();
            } else if self.delegate.row_expandable(ix).is_some() {
                self.delegate.on_toggle_expand(id, cx);
            }
            return;
        }
        if !self.delegate.selectable(ix) {
            return;
        }
        let v = DelegateView(&self.delegate);
        match (ev.modifiers.control, ev.modifiers.shift) {
            (true, true) => self.selection.ctrl_shift_click(id, &v),
            (true, false) => self.selection.ctrl_click(id),
            (false, true) => self.selection.shift_click(id, &v),
            (false, false) => self.selection.click(id),
        }
        cx.notify();
    }

    /// Left press on the body below the last row: nothing selected.
    fn empty_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if !self.selection.is_empty() || self.selection.focus().is_some() {
            self.selection.clear();
            cx.notify();
        }
    }

    /// Display index of the focus row, when it is in the view.
    fn focus_index(&self) -> Option<usize> {
        self.selection
            .focus()
            .and_then(|id| self.delegate.row_index(id))
    }

    fn on_move_focus(&mut self, a: &MoveFocus, _: &mut Window, cx: &mut Context<Self>) {
        let v = DelegateView(&self.delegate);
        self.selection.move_focus(a.delta, a.extend, &v);
        self.keep_focus_visible(cx);
        cx.notify();
    }

    /// PgUp or PgDn: by the whole rows that fit the body, minus one, at least one.
    fn on_move_focus_page(&mut self, a: &MoveFocusPage, _: &mut Window, cx: &mut Context<Self>) {
        let viewport = ScrollbarHandle::viewport_bounds(&self.scroll).size.height;
        let row_h = metrics(cx).row_height;
        let fit = if row_h > Pixels::ZERO {
            (viewport / row_h).floor().max(0.0) as usize
        } else {
            0
        };
        let v = DelegateView(&self.delegate);
        self.selection
            .move_page(fit.saturating_sub(1).max(1), a.down, a.extend, &v);
        self.keep_focus_visible(cx);
        cx.notify();
    }

    fn on_move_focus_edge(&mut self, a: &MoveFocusEdge, _: &mut Window, cx: &mut Context<Self>) {
        let v = DelegateView(&self.delegate);
        self.selection.move_edge(a.end, a.extend, &v);
        self.keep_focus_visible(cx);
        cx.notify();
    }

    fn on_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let v = DelegateView(&self.delegate);
        self.selection.select_all(&v);
        cx.notify();
    }

    fn on_toggle_select_focus(
        &mut self,
        _: &ToggleSelectFocus,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection.toggle_focus();
        cx.notify();
    }

    /// Right: a collapsed row with children expands; an expanded row hands focus and selection
    /// to its first child; a leaf does nothing.
    fn on_expand_or_child(&mut self, _: &ExpandOrChild, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.focus_index() else {
            return;
        };
        match self.delegate.row_expandable(ix) {
            Some(false) => self.delegate.on_toggle_expand(self.delegate.row_id(ix), cx),
            Some(true) => {
                let child = ix + 1;
                if child < self.delegate.row_count()
                    && self.delegate.row_depth(child)
                        == self.delegate.row_depth(ix).saturating_add(1)
                    && self.delegate.selectable(child)
                {
                    self.selection.click(self.delegate.row_id(child));
                    self.keep_focus_visible(cx);
                    cx.notify();
                }
            }
            None => {}
        }
    }

    /// Left: an expanded row collapses; else focus and selection go to the parent. When the
    /// parent is a group header (a depth-1 row in Grouped mode), the group collapses and the
    /// selection stays (UI-PRC-009).
    fn on_collapse_or_parent(
        &mut self,
        _: &CollapseOrParent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.focus_index() else {
            return;
        };
        if self.delegate.row_expandable(ix) == Some(true) {
            self.delegate.on_toggle_expand(self.delegate.row_id(ix), cx);
            return;
        }
        let depth = self.delegate.row_depth(ix);
        if depth == 0 {
            return;
        }
        let Some(parent) = (0..ix).rev().find(|&p| self.delegate.row_depth(p) < depth) else {
            return;
        };
        let parent_id = self.delegate.row_id(parent);
        if self.delegate.row_kind(parent) == RowKind::GroupHeader {
            self.delegate.on_toggle_expand(parent_id, cx);
        } else if self.delegate.selectable(parent) {
            self.selection.click(parent_id);
            self.keep_focus_visible(cx);
            cx.notify();
        }
    }

    /// Space: toggles the focus row; nothing on a leaf.
    fn on_toggle_expand(&mut self, _: &ToggleExpand, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.focus_index() else {
            return;
        };
        if self.delegate.row_expandable(ix).is_some() {
            self.delegate.on_toggle_expand(self.delegate.row_id(ix), cx);
        }
    }

    /// Enter, Alt+Enter, Ctrl+I: a group header toggles, any other row is activated (D-008).
    fn on_open_properties(&mut self, _: &OpenProperties, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.focus_index() else {
            return;
        };
        let id = self.delegate.row_id(ix);
        if self.delegate.row_kind(ix) == RowKind::GroupHeader {
            self.delegate.on_toggle_expand(id, cx);
        } else {
            self.delegate.on_activate(id, cx);
        }
    }

    /// The flattened index of the row under window y `y`, from the body bounds of the last frame.
    fn row_at_y(&self, y: Pixels, row_h: Pixels) -> Option<usize> {
        let body = ScrollbarHandle::viewport_bounds(&self.scroll);
        if row_h <= Pixels::ZERO || y < body.origin.y || y >= body.bottom() {
            return None;
        }
        let top = y - body.origin.y - ScrollbarHandle::offset(&self.scroll).y;
        let ix = (top / row_h).floor() as usize;
        (ix < self.delegate.row_count()).then_some(ix)
    }

    /// The visible column under window x `x`. Right of the last column: the last column.
    fn column_at_x(&self, x: Pixels) -> Option<D::Column> {
        let body = ScrollbarHandle::viewport_bounds(&self.scroll);
        let content_x = x - body.origin.x - ScrollbarHandle::offset(&self.scroll).x;
        let layout = self.columns();
        let pos = layout
            .offsets()
            .partition_point(|&o| o <= content_x)
            .saturating_sub(1);
        layout.visible_columns().get(pos).copied()
    }

    /// Right press on the body (UI-PRC-008). One handler for every row: the row and the column
    /// come from the pointer position. An unselected row becomes the only selection, a selected
    /// row keeps the set, a group header changes nothing and is the only target.
    fn body_right_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let row_h = metrics(cx).row_height;
        let Some(ix) = self.row_at_y(ev.position.y, row_h) else {
            return;
        };
        let Some(col) = self.column_at_x(ev.position.x) else {
            return;
        };
        let id = self.delegate.row_id(ix);
        let targets = if self.delegate.row_kind(ix) == RowKind::GroupHeader {
            smallvec![id]
        } else {
            if !self.delegate.selectable(ix) {
                return;
            }
            if self.selection.right_click(id) {
                cx.notify();
            }
            self.selection.in_view_order(&DelegateView(&self.delegate))
        };
        cx.stop_propagation();
        self.open_menu(targets, Some(col), ev.position, window, cx);
    }

    /// Shift+F10 and the Menu key: the menu for the current selection, its top left corner at
    /// the focus row's Name text start and the row's bottom edge (UI-PRC-009). Nothing without
    /// a focus row or a selection.
    fn on_open_context_menu(
        &mut self,
        _: &OpenContextMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.focus_index() else {
            return;
        };
        let targets = self.selection.in_view_order(&DelegateView(&self.delegate));
        let Some(col) = name_column(&self.delegate) else {
            return;
        };
        if targets.is_empty() {
            return;
        }
        let at = self.focus_menu_anchor(ix, col, cx);
        self.open_menu(targets, None, at, window, cx);
    }

    /// Window position of row `ix`'s Name text start at the row's bottom edge. When the Name
    /// column is hidden: the row's left edge plus `sp4`.
    fn focus_menu_anchor(&self, ix: usize, name_col: D::Column, cx: &App) -> Point<Pixels> {
        let m = metrics(cx);
        let s = m.ui_scale;
        let body = ScrollbarHandle::viewport_bounds(&self.scroll);
        let scroll = ScrollbarHandle::offset(&self.scroll);
        let layout = self.columns();
        let x = match layout.visible_columns().iter().position(|c| *c == name_col) {
            Some(pos) => {
                let cell = self.delegate.name_cell(ix);
                let icon = match cell.icon {
                    NameIcon::None => 0.0,
                    _ => ICON_GAP + icon_px(cell.icon_size),
                };
                layout.offsets()[pos]
                    + name_indent(self.delegate.row_depth(ix), s)
                    + px((CHEVRON + icon + ICON_GAP) * s)
                    + scroll.x
            }
            None => m.sp[4],
        };
        let y = m.row_height * (ix + 1) + scroll.y;
        point(body.origin.x + x, body.origin.y + y)
    }

    /// Opens the delegate's menu for `targets` at `at`. The targets are captured now, so a
    /// re-sort or a new version before an item runs does not change them.
    fn open_menu(
        &mut self,
        targets: SmallVec<[D::RowId; 8]>,
        col: Option<D::Column>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.focus.clone();
        let table = cx.entity();
        let table_closed = table.clone();
        // Built once this update ends: the delegate needs the table's own context, and
        // `TlMenu::build` hands its closure a plain `App`.
        window.defer(cx, move |window, cx| {
            let menu = TlMenu::build(window, cx, focus, |menu, window, cx| {
                table.update(cx, |this, cx| {
                    this.delegate.context_menu(&targets, col, menu, window, cx)
                })
            });
            let sub = window.subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                // The item's action is queued before the dismiss, so queue behind it.
                let table = table_closed.clone();
                window.defer(cx, move |_, cx| {
                    table.update(cx, |this, cx| this.delegate.on_menu_closed(cx));
                });
            });
            sub.detach();
            open_context_menu_at(at, window, cx, menu);
        });
    }

    /// Ctrl+Shift+C (ADR-015): the selected rows in view order as TSV under a line of column
    /// labels. Nothing without a selection.
    fn on_copy_rows_as_table(
        &mut self,
        _: &CopyRowsAsTable,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.selection.in_view_order(&DelegateView(&self.delegate));
        if ids.is_empty() {
            return;
        }
        let text = self.rows_as_tsv(&ids);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.emit(TreeTableEvent::CopiedRows { rows: ids.len() });
    }

    /// The labels of the visible columns joined by tabs, then one line per row in `ids` with
    /// each visible column's text. Cells copy the cached display text, the Name column its
    /// primary text and glyph cells their copy text (UI-FND-026). Group headers are left out.
    pub fn rows_as_tsv(&mut self, ids: &[D::RowId]) -> String {
        let visible = self.columns().visible_columns().clone();
        let name_col = name_column(&self.delegate);
        let mut text = String::new();
        for (pos, &col) in visible.iter().enumerate() {
            if pos > 0 {
                text.push('\t');
            }
            text.push_str(&self.delegate.column_spec(col).label);
        }
        for &id in ids {
            let Some(ix) = self.delegate.row_index(id) else {
                continue;
            };
            if self.delegate.row_kind(ix) == RowKind::GroupHeader {
                continue;
            }
            text.push('\n');
            for (pos, &col) in visible.iter().enumerate() {
                if pos > 0 {
                    text.push('\t');
                }
                if Some(col) == name_col {
                    text.push_str(&self.delegate.name_cell(ix).primary);
                } else {
                    text.push_str(&self.copy_cell_text(ix, id, col));
                }
            }
        }
        text
    }

    /// The text one metric cell copies: the full source of an `arc` cell, else the cached
    /// display text, with the glyph copy text for a non-valid cell.
    fn copy_cell_text(&mut self, ix: usize, id: D::RowId, col: D::Column) -> SharedString {
        self.out.reset();
        self.delegate.cell(ix, col, &mut self.out);
        let text = match self.out.arc.clone() {
            Some(arc) => SharedString::from(arc),
            None => self.formatted_text(ix, id, col),
        };
        copy_text(self.out.validity, &text)
    }

    /// Header hover, press, totals and the cells of the last frame.
    pub fn header_state(&self) -> &HeaderState<D::Column> {
        &self.header_state
    }

    /// The header total of `col` as line 2 shows it: the cached text and its validity. `None`
    /// when the delegate has no total for `col`. Formats only when the value or validity changed.
    pub fn header_total_text(&mut self, col: D::Column) -> Option<(SharedString, Validity)> {
        self.out.reset();
        if !self.delegate.header_total(col, &mut self.out) {
            return None;
        }
        let validity = self.out.validity;
        let delegate = &self.delegate;
        let totals = &mut self.header_state.totals;
        let text = match self.out.arc.clone() {
            Some(arc) => totals.get_or_insert_arc((), col, &arc),
            None => totals.get_or_format((), col, self.out.bits, validity, |s| {
                let mut full = CellOut {
                    text: std::mem::take(s),
                    want_text: true,
                    ..CellOut::default()
                };
                delegate.header_total(col, &mut full);
                *s = full.text;
            }),
        };
        Some((text, validity))
    }

    /// Fills `header_state.cells` and the sort arrow for this frame. The `Vec` keeps its
    /// capacity; texts are `SharedString` clones and cached totals.
    fn prepare_header(
        &mut self,
        lines: HeaderLines,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &Window,
    ) {
        let layout = self.columns();
        let visible = layout.visible_columns().clone();
        let offsets = layout.offsets().clone();
        let total_width = layout.total_width();
        let sort = self.delegate.sort();
        let (sort_col, ascending) = (sort.column(), sort.ascending());
        let mut cells = std::mem::take(&mut self.header_state.cells);
        cells.clear();
        self.header_state.arrow = None;
        for (pos, &id) in visible.iter().enumerate() {
            let x = offsets[pos];
            let w = offsets.get(pos + 1).copied().unwrap_or(total_width) - x;
            let spec = self.delegate.column_spec(id);
            let (label, align, sortable) = (spec.label.clone(), spec.align, spec.sortable);
            let sorted = sortable && id == sort_col;
            if sorted {
                let label_w = if align == ColumnAlign::Left {
                    self.header_state
                        .label_width(id, &label, m.style_version, m, window)
                } else {
                    Pixels::ZERO
                };
                let (top, h) = header::line1(lines, m);
                let a = px(SORT_ARROW * m.ui_scale);
                let origin = point(x + sort_arrow_x(align, w, label_w, m), top + (h - a) / 2.0);
                self.header_state.arrow = Some((id, ascending, Bounds::new(origin, size(a, a))));
            }
            let total = match lines {
                HeaderLines::Two => self.header_total_text(id),
                HeaderLines::One => None,
            };
            let fill = self.header_state.fill(id, sortable, theme);
            cells.push(HeaderCell {
                id,
                label,
                align,
                x,
                w,
                sort: sorted.then_some(ascending),
                total,
                fill,
            });
        }
        self.header_state.cells = cells;
        self.header_state.insert_x = self
            .header_state
            .reorder
            .map(|d| offsets.get(d.slot).copied().unwrap_or(total_width));
    }

    /// Pointer events from the header's hitboxes (UI-PRC-005). A left press and release on the
    /// same sortable cell, without moving more than 4 px, sorts by that column. Moving more than
    /// 4 px drags the column to a new place, except Name. A press on a resize band starts a
    /// boundary drag; a double-click on one fits that column.
    fn header_event(&mut self, ev: HeaderEvent, window: &mut Window, cx: &mut Context<Self>) {
        let scale = metrics(cx).ui_scale;
        let col_at = |this: &Self, pos: Option<usize>| {
            pos.and_then(|p| this.columns().visible_columns().get(p).copied())
        };
        match ev {
            HeaderEvent::Move {
                hovered,
                boundary,
                at,
            } => {
                if let Some(drag) = &mut self.header_state.resize {
                    drag.drag_to(at.x);
                    cx.notify();
                    return;
                }
                if self.header_state.reorder.is_some() {
                    self.reorder_to(at.x);
                    cx.notify();
                    return;
                }
                let col = col_at(self, hovered);
                let band = col_at(self, boundary);
                let mut changed = self.header_state.set_hover(col);
                if self.header_state.boundary_hover != band {
                    self.header_state.boundary_hover = band;
                    changed = true;
                }
                if let Some(p) = &mut self.header_state.press
                    && !p.dragged
                    && beyond_slop(p.at, at, scale)
                {
                    p.dragged = true;
                    changed = true;
                    let p = *p;
                    self.start_reorder(p, at, window, cx);
                }
                if changed {
                    cx.notify();
                }
            }
            HeaderEvent::Down { pos, at } => {
                if let Some(col) = col_at(self, Some(pos)) {
                    self.header_state.press = Some(HeaderPress {
                        col,
                        at,
                        dragged: false,
                    });
                    cx.notify();
                }
            }
            HeaderEvent::BoundaryDown {
                pos,
                at,
                click_count,
            } => {
                let Some(col) = col_at(self, Some(pos)) else {
                    return;
                };
                self.header_state.press = None;
                if click_count >= 2 {
                    self.fit_columns(Some(col), window, cx);
                } else {
                    self.start_resize(col, at, window, cx);
                }
            }
            HeaderEvent::Up { hovered, at } => {
                if let Some(drag) = self.header_state.resize.take() {
                    if drag.changed() {
                        self.delegate.on_columns_changed(&drag.layout, cx);
                    }
                    cx.notify();
                    return;
                }
                if let Some(drag) = self.header_state.reorder.take() {
                    self.header_state.press = None;
                    self.reorder_esc = None;
                    self.drop_column(drag, cx);
                    cx.notify();
                    return;
                }
                let Some(p) = self.header_state.press.take() else {
                    return;
                };
                cx.notify();
                if p.dragged || beyond_slop(p.at, at, scale) || col_at(self, hovered) != Some(p.col)
                {
                    return;
                }
                self.sort_by_column(p.col, cx);
            }
            HeaderEvent::ContextMenu { at } => {
                if self.header_state.resize.is_some() || self.header_state.reorder.is_some() {
                    return;
                }
                let x = self.header_state.content_x(at.x, self.x_offset);
                if let Some(col) = self.columns().column_at_x(x) {
                    cx.stop_propagation();
                    self.open_header_menu(col, at, window, cx);
                }
            }
        }
    }

    /// Opens the header menu (UI-PRC-006) for `col` with its top left corner at `at`. Its
    /// actions run in the table's key context.
    fn open_header_menu(
        &mut self,
        col: D::Column,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = self.delegate.column_spec(col).clone();
        let layout = self.delegate.columns().clone();
        let sort = self.delegate.sort();
        let fixed = self.delegate.columns_fixed();
        let mode = self.resize_mode;
        let menu = TlMenu::build(window, cx, self.focus.clone(), move |menu, window, cx| {
            header_menu(&spec, &layout, &sort, fixed, mode, menu, window, cx)
        });
        open_context_menu_at(at, window, cx, menu);
    }

    /// `HideColumn` from the header menu: hides the column and calls `on_columns_changed` once.
    /// Hiding the last visible column shows the Warning toast instead. A key outside this
    /// table's column set goes on to the next handler.
    fn on_hide_column(&mut self, a: &HideColumn, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.delegate.columns();
        let Some(col) = D::Column::parse(layout.table, &a.id) else {
            cx.propagate();
            return;
        };
        let mut layout = layout.clone();
        match layout.set_visible(col, false) {
            Ok(()) => {
                layout.rebuild(layout.ui_scale());
                self.delegate.on_columns_changed(&layout, cx);
                cx.notify();
            }
            Err(ColumnError::LastVisible) => {
                let text = tr(cx, "table-toast-last-visible");
                push_toast(ToastKind::Warning, text, window, cx);
            }
            Err(err) => {
                tracing::debug!(target: "tasklens::ui", ?err, ?col, "hide column refused");
            }
        }
    }

    /// `ResetColumns` (UI-PRC-006): with the owner's defaults, replaces the layout with them,
    /// calls `on_columns_changed` once and shows "Columns reset to default". Without defaults,
    /// or for another table, the action goes on to the owner.
    fn on_reset_columns(&mut self, a: &ResetColumns, window: &mut Window, cx: &mut Context<Self>) {
        let has_defaults = self
            .default_layout
            .as_ref()
            .is_some_and(|d| d.table == a.table);
        if !has_defaults {
            cx.propagate();
            return;
        }
        self.reset_columns(window, cx);
    }

    /// Replaces the layout with the owner's defaults, calls `on_columns_changed` once and shows
    /// "Columns reset to default" in `window`. False and no change without defaults.
    fn reset_columns(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let scale = self.delegate.columns().ui_scale();
        let Some(defaults) = &self.default_layout else {
            return false;
        };
        let mut layout = defaults.clone();
        layout.rebuild(scale);
        self.delegate.on_columns_changed(&layout, cx);
        cx.notify();
        let text = tr(cx, "table-toast-columns-reset");
        push_toast(ToastKind::Info, text, window, cx);
        true
    }

    /// `EnableAllColumns` (R-180): shows every column that exists on this system, each in its
    /// remembered slot and width, and calls `on_columns_changed` once. Another table's id goes
    /// on to the next handler.
    fn on_enable_all_columns(
        &mut self,
        a: &EnableAllColumns,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let layout = self.delegate.columns();
        if a.table != layout.table {
            cx.propagate();
            return;
        }
        let mut layout = layout.clone();
        let ids = layout.order.clone();
        for id in ids {
            if self.delegate.column_spec(id).availability != Availability::NotOnThisSystem {
                let _ = layout.set_visible(id, true);
            }
        }
        layout.rebuild(layout.ui_scale());
        self.delegate.on_columns_changed(&layout, cx);
        cx.notify();
    }

    /// `OpenColumnPicker` for this table: opens the picker bound to it, or focuses the open one
    /// and binds it here. A table with fixed columns or another id passes the action on.
    fn on_open_column_picker(
        &mut self,
        a: &OpenColumnPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if a.table != self.delegate.columns().table || self.delegate.columns_fixed() {
            cx.propagate();
            return;
        }
        let host: Rc<dyn ColumnHost> = Rc::new(cx.weak_entity());
        let parent = Some(window.window_handle());
        // The picker reads this table while it opens, so it opens after this update.
        cx.defer(move |cx| columns_window::open_column_picker(host, parent, cx));
    }

    /// Shows or hides the column `key` for the picker and calls `on_columns_changed` once.
    fn set_column_visible(
        &mut self,
        key: &str,
        on: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), ColumnError> {
        let layout = self.delegate.columns();
        let col = D::Column::parse(layout.table, key).ok_or(ColumnError::Unknown)?;
        let mut layout = layout.clone();
        if layout.visible.contains(&col) == on {
            return Ok(());
        }
        layout.set_visible(col, on)?;
        layout.rebuild(layout.ui_scale());
        self.delegate.on_columns_changed(&layout, cx);
        cx.notify();
        Ok(())
    }

    /// Turns `press`, which just moved past the slop to window point `at`, into a reorder drag.
    /// Name (`hideable == false`) never drags. Esc cancels the drag through a keystroke
    /// interceptor, which runs before every key binding, so the global Esc chain never sees it.
    fn start_reorder(
        &mut self,
        press: HeaderPress<D::Column>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.delegate.column_spec(press.col).hideable {
            return;
        }
        let layout = self.columns();
        let Some(pos) = layout
            .visible_columns()
            .iter()
            .position(|c| *c == press.col)
        else {
            return;
        };
        let x_offset = self.x_offset;
        let grab = self.header_state.content_x(press.at.x, x_offset) - layout.offsets()[pos];
        self.header_state.reorder = Some(ReorderDrag {
            col: press.col,
            grab,
            x: Pixels::ZERO,
            slot: pos,
        });
        self.reorder_to(at.x);
        self.header_state.set_hover(None);
        let this = cx.weak_entity();
        let handle = window.window_handle();
        self.reorder_esc = Some(cx.intercept_keystrokes(move |ev, window, cx| {
            if ev.keystroke.key == "escape" && window.window_handle() == handle {
                let _ = this.update(cx, |t, cx| t.cancel_reorder(cx));
                cx.stop_propagation();
            }
        }));
    }

    /// Moves the reorder ghost to window x `x` and picks the nearest allowed boundary.
    fn reorder_to(&mut self, x: Pixels) {
        let x = self.header_state.content_x(x, self.x_offset);
        let layout = self.columns();
        let slot = insertion_slot(
            layout.visible_columns(),
            layout.offsets(),
            layout.total_width(),
            x,
        );
        if let Some(drag) = &mut self.header_state.reorder {
            drag.x = x;
            drag.slot = slot;
        }
    }

    /// Esc during a reorder drag: ends it with no change. The release that follows does nothing.
    fn cancel_reorder(&mut self, cx: &mut Context<Self>) {
        self.header_state.reorder = None;
        self.header_state.press = None;
        self.reorder_esc = None;
        cx.notify();
    }

    /// Applies a finished reorder drag: moves the column before the column at the drop slot, or
    /// to the end, and calls `on_columns_changed` once. A drop on either edge of the column
    /// itself changes nothing.
    fn drop_column(&mut self, drag: ReorderDrag<D::Column>, cx: &mut Context<Self>) {
        let layout = self.delegate.columns();
        let visible = layout.visible_columns();
        let Some(pos) = visible.iter().position(|c| *c == drag.col) else {
            return;
        };
        if drag.slot == pos || drag.slot == pos + 1 {
            return;
        }
        let before = visible.get(drag.slot).copied();
        let mut layout = layout.clone();
        if layout.move_column(drag.col, before).is_err() {
            return;
        }
        layout.rebuild(layout.ui_scale());
        self.delegate.on_columns_changed(&layout, cx);
    }

    /// Starts a boundary drag of `col` at window point `at`, on a copy of the delegate's layout.
    fn start_resize(
        &mut self,
        col: D::Column,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mins = self.column_mins(window, cx);
        let layout = self.delegate.columns().clone();
        let scale = layout.ui_scale();
        let body = ScrollbarHandle::viewport_bounds(&self.scroll).size.width;
        // Before the first layout of the body the table's own width stands in.
        let viewport = if body > Pixels::ZERO {
            body
        } else {
            layout.total_width()
        } / scale;
        let start_widths = layout.widths.clone();
        self.header_state.resize = Some(ResizeDrag {
            col,
            start_x: at.x,
            start_width: start_widths.get(&col).copied().unwrap_or(Pixels::ZERO),
            start_widths,
            mins,
            viewport,
            mode: self.resize_mode,
            layout,
        });
        self.header_state.set_hover(None);
        cx.notify();
    }

    /// The minimum width of every visible column at 100% (PROC-023), with the sort arrow's room
    /// on the sort column. Measured through the global `TextMetricsCache`.
    fn column_mins(&self, window: &Window, cx: &mut App) -> FxHashMap<D::Column, Pixels> {
        let m = *metrics(cx);
        let sort_col = self.delegate.sort().column();
        let layout = self.delegate.columns();
        let scale = layout.ui_scale();
        if !cx.has_global::<TextMetricsCache>() {
            cx.set_global(TextMetricsCache::default());
        }
        cx.update_global::<TextMetricsCache, _>(|cache, cx| {
            cache.clear_if_stale(m.style_version);
            layout
                .visible_columns()
                .iter()
                .map(|&col| {
                    let spec = self.delegate.column_spec(col);
                    let sorted = spec.sortable && col == sort_col;
                    let min = min_width(spec, sorted, cache, &m, window, cx);
                    (col, min / scale)
                })
                .collect()
        })
    }

    /// Fits `only`, or every visible column on `None`, to its content (UI-PRC-006): the largest
    /// of the minimum, the widest text + 16 and the header label + 16 + 12. Texts come from the
    /// rows the last frame built and the cell texts still cached from the last 3 table versions,
    /// never from a pass over all rows. Then calls `on_columns_changed` once.
    fn fit_columns(
        &mut self,
        only: Option<D::Column>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = *metrics(cx);
        let mins = self.column_mins(window, cx);
        let mut layout = self.delegate.columns().clone();
        let scale = layout.ui_scale();
        let targets: SmallVec<[D::Column; 24]> = match only {
            Some(col) => smallvec![col],
            None => layout.visible_columns().iter().copied().collect(),
        };
        let widest = self.widest_texts(&targets, &m, window, cx);
        for (i, &col) in targets.iter().enumerate() {
            let label = self.delegate.column_spec(col).label.clone();
            let label_w = self
                .header_state
                .label_width(col, &label, m.style_version, &m, window);
            let min = mins.get(&col).copied().unwrap_or(Pixels::ZERO) * scale;
            let w = fit_width(min, widest[i], label_w, &m);
            layout.widths.insert(col, w / scale);
        }
        layout.rebuild(scale);
        self.delegate.on_columns_changed(&layout, cx);
        cx.notify();
    }

    /// The widest content of each column in `targets`, scaled, in `targets` order. The Name
    /// column counts its indent, chevron and icon before the text. Numeric columns measure
    /// digits at their `tnum` width.
    fn widest_texts(
        &mut self,
        targets: &[D::Column],
        m: &UiMetrics,
        window: &Window,
        cx: &mut App,
    ) -> SmallVec<[Pixels; 24]> {
        let s = m.ui_scale;
        let name_col = name_column(&self.delegate);
        let count = self.delegate.row_count();
        let rows = self.visible_range.start.min(count)..self.visible_range.end.min(count);
        // Texts of the visible rows: (target position, text, width before the text).
        let mut texts: Vec<(usize, SharedString, Pixels)> =
            Vec::with_capacity(rows.len() * targets.len());
        for ix in rows {
            if self.delegate.row_kind(ix) == RowKind::GroupHeader {
                continue;
            }
            let id = self.delegate.row_id(ix);
            for (i, &col) in targets.iter().enumerate() {
                if Some(col) == name_col {
                    let cell = self.delegate.name_cell(ix);
                    let icon = match cell.icon {
                        NameIcon::None => 0.0,
                        _ => ICON_GAP + icon_px(cell.icon_size),
                    };
                    let prefix = name_indent(self.delegate.row_depth(ix), s)
                        + px((CHEVRON + icon + ICON_GAP) * s);
                    texts.push((i, cell.primary, prefix));
                    continue;
                }
                self.out.reset();
                self.delegate.cell(ix, col, &mut self.out);
                let text = match self.out.arc.clone() {
                    Some(arc) => SharedString::from(arc),
                    None => self.formatted_text(ix, id, col),
                };
                texts.push((i, text, Pixels::ZERO));
            }
        }
        let numeric: SmallVec<[bool; 24]> = targets
            .iter()
            .map(|&c| self.delegate.column_spec(c).kind == ColumnKind::Numeric)
            .collect();
        let size = m.text.base;
        let text_cache = &self.text_cache;
        if !cx.has_global::<TextMetricsCache>() {
            cx.set_global(TextMetricsCache::default());
        }
        cx.update_global::<TextMetricsCache, _>(|cache, cx| {
            cache.clear_if_stale(m.style_version);
            let digit = cache.template_width("0", size, window, cx);
            let mut widest: SmallVec<[Pixels; 24]> = smallvec![Pixels::ZERO; targets.len()];
            let mut measure = |i: usize, text: &str, before: Pixels| {
                let w = text.chars().fold(before, |sum, ch| {
                    sum + if numeric[i] && ch.is_ascii_digit() {
                        digit
                    } else {
                        cache.char_advance(ch, size, window, cx)
                    }
                });
                widest[i] = widest[i].max(w);
            };
            for (i, text, before) in &texts {
                measure(*i, text, *before);
            }
            for (col, text) in text_cache.cell_texts() {
                if Some(col) != name_col
                    && let Some(i) = targets.iter().position(|c| *c == col)
                {
                    measure(i, text, Pixels::ZERO);
                }
            }
            widest
        })
    }

    /// `ResizeColumnsToFit` from the header menu: fits every visible column. An action for
    /// another table goes on to the next handler.
    fn on_resize_columns_to_fit(
        &mut self,
        a: &ResizeColumnsToFit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if a.table != self.delegate.columns().table {
            cx.propagate();
            return;
        }
        self.fit_columns(None, window, cx);
    }

    /// Asks the delegate to sort by `col` (header click and `SortByColumn`). The delegate picks
    /// the direction with `next_sort`. Nothing on an unsortable column.
    pub fn sort_by_column(&mut self, col: D::Column, cx: &mut Context<Self>) {
        if !self.delegate.column_spec(col).sortable {
            return;
        }
        self.delegate.on_sort(col, cx);
        cx.notify();
    }

    /// `SortByColumn` from the header menu. A key outside this table's column set is ignored.
    fn on_sort_by_column(&mut self, a: &SortByColumn, _: &mut Window, cx: &mut Context<Self>) {
        let table = self.columns().table;
        if let Some(col) = D::Column::parse(table, &a.id) {
            self.sort_by_column(col, cx);
        }
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
        let Some(table) = self.upgrade() else {
            return Vec::new();
        };
        let delegate = &table.read(cx).delegate;
        let layout = delegate.columns();
        layout
            .order
            .iter()
            .map(|&id| {
                let spec = delegate.column_spec(id);
                PickerColumn {
                    key: id.key(),
                    label: spec.label.clone(),
                    full_name: spec.full_name.clone(),
                    group: spec.group.clone(),
                    availability: spec.availability,
                    hideable: spec.hideable,
                    visible: layout.visible.contains(&id),
                }
            })
            .collect()
    }

    fn set_visible(
        &self,
        key: &str,
        on: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Result<(), ColumnError> {
        self.update(cx, |t, cx| t.set_column_visible(key, on, cx))
            .unwrap_or(Err(ColumnError::Unknown))
    }

    fn reset(&self, window: &mut Window, cx: &mut App) {
        let reset = self.update(cx, |t, cx| t.reset_columns(window, cx));
        if !matches!(reset, Ok(true)) {
            tracing::debug!(target: "tasklens::ui", "picker reset: the table has no defaults");
        }
    }

    fn observe(&self, picker: &mut Context<ColumnPicker>) -> Option<Subscription> {
        let table = self.upgrade()?;
        Some(picker.observe(&table, |_, _, cx| cx.notify()))
    }
}

/// Pushes a toast to the window's `ToastQueue`. A window without a queue shows nothing.
fn push_toast(kind: ToastKind, text: SharedString, window: &mut Window, cx: &mut App) {
    let Some(queue) = ToastQueue::of_window(window, cx) else {
        tracing::debug!(target: "tasklens::ui", %text, "no toast queue in this window");
        return;
    };
    queue.update(cx, |q, cx| q.push(kind, text, None, None, cx));
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

/// The body's scroll handle as the scrollbar sees it. It tells the table when a thumb drag
/// starts and ends, so anchoring pauses during the drag.
#[derive(Clone)]
struct BodyScroll {
    list: UniformListScrollHandle,
    dragging: Rc<Cell<bool>>,
}

impl ScrollbarHandle for BodyScroll {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        ScrollbarHandle::viewport_bounds(&self.list)
    }
    fn offset(&self) -> Point<Pixels> {
        ScrollbarHandle::offset(&self.list)
    }
    fn set_offset(&self, offset: Point<Pixels>) {
        ScrollbarHandle::set_offset(&self.list, offset);
    }
    fn content_size(&self) -> Size<Pixels> {
        ScrollbarHandle::content_size(&self.list)
    }
    fn start_drag(&self) {
        self.dragging.set(true);
    }
    fn end_drag(&self) {
        self.dragging.set(false);
    }
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
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        self.sync_versions(m.style_version);
        self.list_calls = 0;
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.separator_quads.borrow_mut().clear();
            self.guide_quads.borrow_mut().clear();
            self.line_canvases.set(0);
        }
        self.x_offset = -ScrollbarHandle::offset(&self.scroll).x;
        let hairline = UiMetrics::hairline(window);

        let lines = self.delegate.header_lines();
        self.prepare_header(lines, &m, &theme, window);
        let tooltip = self
            .header_state
            .hover
            .and_then(|c| header_tooltip(self.delegate.column_spec(c)));
        let header = TlTreeTableHeader {
            layout: self.columns(),
            lines,
            x_offset: self.x_offset,
            m: &m,
            theme: &theme,
            hairline,
            state: &self.header_state,
            listener: self.header_listener.clone(),
        }
        .render(tooltip, window, cx);

        let count = self.delegate.row_count();
        if count == 0 {
            // The list builds no rows, so nothing records the frame.
            self.visible_range = 0..0;
            self.anchor = None;
        }
        // Demand runs after every render, so a new version with the same range still reports
        // a changed id set (ADR-009).
        if !self.demand_scheduled {
            self.demand_scheduled = true;
            cx.on_next_frame(window, |this, _, cx| this.report_demand(cx));
        }
        let empty = if count == 0 {
            self.delegate.empty_text()
        } else {
            None
        };
        let ring = self.focus.is_focused(window)
            && window.last_input_was_keyboard()
            && self.selection.is_empty()
            && self.selection.focus().is_none();

        let list = uniform_list("tl-tree-table-body", count, cx.processor(Self::build_rows))
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .track_scroll(&self.scroll)
            .size_full();

        let bar = |st: ScrollbarThumbStyle, w: f32| {
            st.bg(theme.scroll_thumb)
                .width(px(w * m.ui_scale))
                .inset(px(4.0 * m.ui_scale))
                .radius(Pixels::ZERO)
                .min_length(px(48.0 * m.ui_scale))
        };
        let handle = BodyScroll {
            list: self.scroll.clone(),
            dragging: self.thumb_dragging.clone(),
        };
        let scrollbar = Scrollbar::new(&handle)
            .id("tl-tree-table-scrollbar")
            .axis(ScrollbarAxis::Both)
            .mode(ScrollbarMode::Always)
            .styles(|s| {
                s.track(|t| t.bg(theme.scroll_track).width(px(SCROLLBAR_W * m.ui_scale)))
                    .track_hover(|t| t.bg(theme.scroll_track))
                    .track_active(|t| t.bg(theme.scroll_track))
                    .thumb(|t| bar(t, 6.0))
                    .thumb_hover(|t| bar(t, 8.0))
                    .thumb_active(|t| bar(t, 8.0))
            });

        let mut body = div()
            .id("tl-tree-table-body-box")
            .debug_selector(|| "TlTreeTable-body".into())
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                this.last_wheel = Some(cx.background_executor().now());
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.set_hover(None, cx);
                }
            }))
            // Rows stop the press, so only a press below the last row gets here.
            .on_mouse_down(MouseButton::Left, cx.listener(Self::empty_mouse_down))
            // The one context-menu handler; rows do not stop the right press.
            .on_mouse_down(MouseButton::Right, cx.listener(Self::body_right_down))
            .child(list)
            .child(scrollbar);
        if let Some(text) = empty.clone()
            && self.delegate.empty_clear_filter()
        {
            body = body.child(
                div()
                    .debug_selector(|| "TlTreeTable-empty-state".into())
                    .absolute()
                    .top(px(EMPTY_STATE_TOP * m.ui_scale))
                    .left_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(TlEmptyState::new(text).clear_filter(Box::new(ClearFilter))),
            );
        } else if let Some(text) = empty {
            let mut label = div();
            *label.text_style() = t_base(&m);
            body = body.child(
                label
                    .debug_selector(|| "TlTreeTable-empty".into())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_secondary)
                    .child(text),
            );
        }
        if let Some(x) = self.header_state.insert_x {
            body = body.child(
                insert_line(x - self.x_offset, &theme, &m)
                    .debug_selector(|| "TlTreeTable-insert".into()),
            );
        }
        if ring {
            body = body.child(
                div()
                    .debug_selector(|| "TlTreeTable-focus-ring".into())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .border(px(m.ui_scale))
                    .border_color(theme.accent),
            );
        }

        v_flex()
            .debug_selector(|| "TlTreeTable".into())
            .size_full()
            .bg(theme.list_bg)
            .key_context(TABLE_CONTEXT)
            .track_focus(&self.focus)
            .tab_index(0)
            .tab_stop(self.delegate.tab_stop())
            // CopyRow, CopyCell, CollapseAll and the header Actions have no handler here, so they
            // bubble to the owner page (open item 16).
            .on_action(cx.listener(Self::on_move_focus))
            .on_action(cx.listener(Self::on_move_focus_page))
            .on_action(cx.listener(Self::on_move_focus_edge))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_toggle_select_focus))
            .on_action(cx.listener(Self::on_expand_or_child))
            .on_action(cx.listener(Self::on_collapse_or_parent))
            .on_action(cx.listener(Self::on_toggle_expand))
            .on_action(cx.listener(Self::on_open_properties))
            .on_action(cx.listener(Self::on_open_context_menu))
            .on_action(cx.listener(Self::on_copy_rows_as_table))
            .on_action(cx.listener(Self::on_sort_by_column))
            .on_action(cx.listener(Self::on_resize_columns_to_fit))
            .on_action(cx.listener(Self::on_hide_column))
            .on_action(cx.listener(Self::on_reset_columns))
            .on_action(cx.listener(Self::on_enable_all_columns))
            .on_action(cx.listener(Self::on_open_column_picker))
            .child(header)
            .child(body)
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
