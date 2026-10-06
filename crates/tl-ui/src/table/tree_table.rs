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
    AnyElement, AnyView, App, Bounds, Context, Div, ElementId, FocusHandle, Hsla,
    InteractiveElement, IntoElement, ListHorizontalSizingBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollStrategy, ScrollWheelEvent,
    SharedString, Size, StatefulInteractiveElement as _, Styled, TextRun, UniformListScrollHandle,
    Window, div, font, point, px, uniform_list,
};
use rustc_hash::FxBuildHasher;
use smallvec::SmallVec;
use tl_model::validity::Validity;

use crate::components::menu::TlMenu;
use crate::components::tooltip::{DELAY_CONTROL, DELAY_ROW_NAME, HoverTooltip, TlTooltip};
use crate::i18n::tr;
use crate::table::actions::{
    CollapseOrParent, ExpandOrChild, MoveFocus, MoveFocusEdge, MoveFocusPage, OpenProperties,
    SelectAll, TABLE_CONTEXT, ToggleExpand, ToggleSelectFocus,
};
use crate::table::cells::{
    CellCtx, CellOut, attach_cell_tooltip, cell_background, cell_text_color, glyph_cell,
    name_indent, numeric_cell, paint_group_header_cell, paint_name_cell, path_cell, text_cell,
};
use crate::table::columns::{ColumnKind, ColumnLayout, ColumnSpec, TableColumn};
use crate::table::header::{HeaderState, TlTreeTableHeader};
use crate::table::selection::{Selection, SelectionView};
use crate::text::ellipsis::middle_ellipsis;
use crate::text::fonts::UI_FAMILY;
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
    type Sort: Clone + PartialEq + 'static;

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
    /// Fills the header totals line for `col`. False: no line 2 for this column.
    fn header_total(&self, col: Self::Column, out: &mut CellOut) -> bool;
    fn sort(&self) -> Self::Sort;
    /// False for group headers.
    fn selectable(&self, ix: usize) -> bool;
    fn context_menu(
        &mut self,
        targets: &[Self::RowId],
        clicked_col: Self::Column,
        menu: TlMenu,
        window: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu;
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
    /// Text for an empty table (UI-PRC-024).
    fn empty_text(&self) -> Option<SharedString>;

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::Two
    }

    /// True: no column picker and no header-menu items 2, 3 and 6. Resize and sort still work.
    fn columns_fixed(&self) -> bool {
        false
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

/// Opacity of the metric cells of a `STALE` row (UI-PRC-002).
pub const STALE_OPACITY: f32 = 0.6;

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
    /// Drag and resize, filled by E18-009 to E18-011.
    #[allow(dead_code)]
    header_state: HeaderState,
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
}

/// What every row of one frame shares.
struct FrameCtx<C> {
    m: UiMetrics,
    theme: Arc<TlTheme>,
    hairline: Pixels,
    total_width: Pixels,
    guide_lines: bool,
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
            header_state: HeaderState::default(),
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
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn delegate_mut(&mut self) -> &mut D {
        &mut self.delegate
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

    /// The body rows the last frame built.
    pub fn visible_range(&self) -> Range<usize> {
        self.visible_range.clone()
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

    /// The body's scroll handle, shared by both scrollbars.
    pub fn scroll_handle(&self) -> &UniformListScrollHandle {
        &self.scroll
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
        viewport > Pixels::ZERO && self.delegate.columns().total_width() > viewport
    }

    fn sync_versions(&mut self, style_version: u64) {
        let version = self.delegate.table_version();
        if version != self.last_version || style_version != self.last_style_version {
            self.text_cache.begin_version(version, style_version);
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
        match row_tint(self.delegate.row_flags(ix), theme) {
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
        let total_width = self.delegate.columns().total_width();
        if call < 2 && range.len() == 1 {
            return vec![div().w(total_width).h(m.row_height).into_any_element()];
        }
        #[cfg(any(test, feature = "test-hooks"))]
        self.rows_built_last_frame.set(range.len() as u32);
        self.record_frame(range.clone(), m.row_height);

        let layout = self.delegate.columns();
        let frame = FrameCtx {
            m,
            theme: current(cx).clone(),
            hairline: UiMetrics::hairline(window),
            total_width,
            guide_lines: self.delegate.guide_lines(),
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
        self.anchor = range
            .clone()
            .find(|&ix| self.delegate.selectable(ix))
            .map(|ix| (self.delegate.row_id(ix), ix - range.start, sub));
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
                let label = self.delegate.name_cell(ix).primary;
                let cell = paint_group_header_cell(label, self.delegate.row_expandable(ix), &ctx);
                row = row.child(cell.absolute().top_0().left(x));
            }
            return row.into_any_element();
        }

        let selected = self.selection.contains(&id);
        let hovered = self.hover_row == Some(ix);
        // Cell backgrounds show only under the zebra; hover and selection cover them.
        let plain = !selected && !hovered;
        let flags = self.delegate.row_flags(ix);
        let tint = row_tint(flags, theme);
        let stale = flags.has(RowFlags::STALE);
        let tips = hovered && self.hover_tips_ready;
        let row_key = FxBuildHasher.hash_one(id);

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
                let name = self.delegate.name_cell(ix);
                let depth = self.delegate.row_depth(ix);
                let expandable = self.delegate.row_expandable(ix);
                let mut cell = paint_name_cell(
                    name,
                    depth,
                    expandable,
                    flags,
                    f.guide_lines,
                    &ctx,
                    tips.then_some(row_key),
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
            if plain && let Some(bg) = cell_background(&self.out, &ctx) {
                cell = cell.bg(tint.map_or(bg, |t| bg.blend(t)));
            }
            if stale {
                cell = cell.opacity(metric_cell_opacity(flags));
            }
            if tips {
                cell = self.attach_cell_tip(cell, col, pos, text, row_key, window, cx);
            }
            row = row.child(cell.absolute().top_0().left(x));
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
        let out = &mut self.out;
        out.reset();
        self.delegate.cell(ix, col, out);
        let spec = self.delegate.column_spec(col);
        let kind = spec.kind;
        let flag = if hovered {
            self.hover_clips[pos].clone()
        } else {
            self.clip_sink.clone()
        };
        let text = match out.arc.clone() {
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
            None => {
                let delegate = &self.delegate;
                self.text_cache
                    .get_or_format(id, col, out.bits, out.validity, |s| {
                        let mut full = CellOut {
                            text: std::mem::take(s),
                            want_text: true,
                            ..CellOut::default()
                        };
                        delegate.cell(ix, col, &mut full);
                        *s = full.text;
                    })
            }
        };
        let validity = out.validity;
        let cell = if !matches!(validity, Validity::Valid | Validity::Stale) {
            glyph_cell(validity, &text, ctx)
        } else {
            match kind {
                ColumnKind::Text => text_cell(text.clone(), ctx, flag),
                ColumnKind::Path => path_cell(text.clone(), ctx),
                ColumnKind::Numeric => numeric_cell(text.clone(), validity, flag, ctx),
            }
        };
        (cell, text)
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
        if let Some(tip) = self.out.tooltip.clone().or(path_cut) {
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
                self.delegate.on_activate(id, cx);
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

/// The NEW or EXITED tint of a row, if any (UI-PRC-002).
fn row_tint(flags: RowFlags, theme: &TlTheme) -> Option<Hsla> {
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
        self.x_offset = -ScrollbarHandle::offset(&self.scroll).x;
        let hairline = UiMetrics::hairline(window);

        let header = TlTreeTableHeader {
            layout: self.delegate.columns(),
            lines: self.delegate.header_lines(),
            x_offset: self.x_offset,
            m: &m,
            theme: &theme,
            hairline,
        }
        .render(|id| self.delegate.column_spec(id));

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
            .child(list)
            .child(scrollbar);
        if let Some(text) = empty {
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
            .tab_stop(true)
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
