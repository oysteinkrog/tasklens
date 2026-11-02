//! The non-generic body of `TlTreeTable` (ADR-009). `TlTreeTable<D>` keeps the typed state its
//! owners read (selection, header state, column layouts) and hands everything per row and per
//! frame to [`TreeTableCore`], which talks to the delegate through [`DynTreeDelegate`]. The body
//! is then compiled once in tl-ui-table instead of once per delegate type.
//!
//! Ids are erased like this:
//! - Rows: a [`RowKey`] is a number the typed shell hands out per row id (`RowKeys`). The core
//!   keeps keys across frames only in the cell text cache and in the row listeners of the last
//!   frame. Everything else asks by row index in the current table version.
//! - Columns: a [`ColKey`] is a column's slot in the shell's append-only column registry, with
//!   its `state.toml` key for element ids.

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::base::{
    Scrollbar, ScrollbarAxis, ScrollbarHandle, ScrollbarMode, ScrollbarThumbStyle,
};
use gpui_kit::{
    Action, AnyElement, AnyEntity, AnyView, App, AsyncApp, Bounds, ClipboardItem, ContentMask, Div,
    ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement,
    ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, ScrollStrategy, ScrollWheelEvent, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled, TextRun, UniformListScrollHandle, Window, canvas, div,
    fill, font, point, px, size, uniform_list,
};
use gpui_kit::{BorrowAppContext as _, DismissEvent};
use smallvec::{SmallVec, smallvec};

use crate::components::empty::TlEmptyState;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::tooltip::{DELAY_CONTROL, DELAY_ROW_NAME, HoverTooltip, TlTooltip};
use crate::components::validity::copy_text;
use crate::i18n::tr;
use crate::selector::TlSelector;
use crate::table::actions::{
    CollapseOrParent, CopyRowsAsTable, ExpandOrChild, MoveFocus, MoveFocusEdge, MoveFocusPage,
    OpenContextMenu, OpenProperties, SelectAll, ToggleExpand, ToggleSelectFocus,
};
use crate::table::cells::{
    CellAt, CellCtx, CellOut, IconSourceCache, attach_cell_tooltip, cell_background,
    cell_text_color, guide_quads, icon_px, name_indent, paint_group_header_cell, paint_name_cell,
    value_cell,
};
use crate::table::columns::ColumnKind;
use crate::table::tree_table::{
    CHEVRON, CHEVRON_HIT_W, EMPTY_STATE_TOP, ICON_GAP, NameCell, NameIcon, PRESS_HOLD_AFTER_UP,
    PRESS_HOLD_MAX, RowFlags, RowKind, SCROLLBAR_W, TreeTableEvent, WHEEL_QUIET,
    metric_cell_opacity, separator_quads,
};
use crate::text::ellipsis::middle_ellipsis;
use crate::text::fonts::UI_FAMILY;
use crate::text::metrics::TextMetricsCache;
use crate::text::style::t_base;
use crate::text_cache::CellTextCache;
use crate::theme::{TlTheme, UiMetrics, metrics};

/// A row id as the core sees it. The typed shell maps each row id it is asked about to one key
/// and back. Keys are never reused within one table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct RowKey(pub(crate) u64);

/// A column id as the core sees it: the column's slot in the table's column registry, and its
/// `state.toml` key. Equal when the slots are equal; keys of two tables are never compared.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ColKey {
    pub(crate) slot: u32,
    pub(crate) name: &'static str,
}

impl PartialEq for ColKey {
    fn eq(&self, other: &Self) -> bool {
        self.slot == other.slot
    }
}

impl Eq for ColKey {}

impl Hash for ColKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.slot.hash(state);
    }
}

/// The columns one frame draws: the live drag copy during a boundary drag, else the delegate's.
pub(crate) struct FrameLayout {
    pub(crate) visible: Arc<[ColKey]>,
    pub(crate) offsets: Arc<[Pixels]>,
}

/// The row menu the delegate builds once the press has been handled, and what runs when it
/// closes. Both hold the typed targets the shell captured at the press.
pub(crate) struct RowMenu {
    pub(crate) build: MenuBuild,
    pub(crate) closed: Rc<dyn Fn(&mut App)>,
}

/// Builds a menu's items, once, when it opens.
pub(crate) type MenuBuild = Box<dyn FnOnce(TlMenu, &mut Window, &mut App) -> TlMenu>;

/// A left click on a row, by modifier.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Click {
    Plain,
    Ctrl,
    Shift,
    CtrlShift,
}

/// The one way back into the table from a listener, a timer or a frame callback: runs `f` on the
/// core and a fresh delegate host inside an update of the table entity. Holds the entity weakly,
/// as `Context::listener` does; a dropped table runs nothing.
pub(crate) type Link = Rc<dyn Fn(&mut App, CoreCall<'_>)>;

/// What a [`Link`] runs inside the table's update.
pub(crate) type CoreCall<'a> = &'a mut dyn FnMut(&mut TreeTableCore, &mut dyn DynTreeDelegate);

/// What the core asks of the delegate, the typed selection and the typed shell state, short of
/// the table's `Context`. Rows are named by index in the current table version, or by
/// [`RowKey`] where a listener keeps one. Implemented once per delegate type by the shell's
/// `Host`.
pub(crate) trait DynRows {
    fn row_count(&self) -> usize;
    /// The key of row `ix`'s id, handed out on first use.
    fn row_key(&mut self, ix: usize) -> RowKey;
    /// The current index of the row `key` names. `None` when the row left the view.
    fn key_index(&self, key: RowKey) -> Option<usize>;
    /// `FxBuildHasher.hash_one` of row `ix`'s id, for element ids.
    fn row_hash(&self, ix: usize) -> u64;
    fn row_kind(&self, ix: usize) -> RowKind;
    fn row_depth(&self, ix: usize) -> u8;
    fn row_expandable(&self, ix: usize) -> Option<bool>;
    fn row_flags(&self, ix: usize) -> RowFlags;
    fn selectable(&self, ix: usize) -> bool;
    fn row_tint(&self, ix: usize, theme: &TlTheme) -> Option<Hsla>;
    fn row_tooltip(&self, ix: usize) -> Option<AnyView>;
    fn name_cell(&self, ix: usize) -> NameCell;
    fn name_weight(&self, ix: usize) -> FontWeight;
    fn cell(&self, ix: usize, col: ColKey, out: &mut CellOut);

    fn layout(&mut self) -> FrameLayout;
    /// `layout().total_width`, without building the visible list.
    fn total_width(&self) -> Pixels;
    /// `name_column(delegate)` as a key.
    fn name_column(&mut self) -> Option<ColKey>;
    fn col_kind(&self, col: ColKey) -> ColumnKind;
    fn col_label(&self, col: ColKey) -> SharedString;
    fn fit_samples(&self, col: ColKey) -> Vec<Arc<str>>;
    fn cell_tip_delay(&self) -> Duration;
    fn cell_tip_content(&self, col: ColKey, tip: SharedString) -> TlTooltip;
    fn flat_names(&self) -> bool;
    fn guide_lines(&self) -> bool;
    fn double_click_expands(&self) -> bool;
    fn empty_text(&self) -> Option<SharedString>;
    fn empty_clear_filter(&self) -> bool;
    fn empty_clear_action(&self) -> Box<dyn Action>;
    /// `TreeTableDelegate::tab_stop`.
    fn tab_stop(&self) -> bool;

    /// Row `ix`'s id is in the selection.
    fn is_selected(&self, ix: usize) -> bool;
    /// Row `ix`'s id is the selection focus.
    fn is_focus(&self, ix: usize) -> bool;
    /// Display index of the focus row, when it is in the view.
    fn focus_index(&self) -> Option<usize>;
    fn selection_empty(&self) -> bool;
    fn has_focus_row(&self) -> bool;
    fn click(&mut self, ix: usize, click: Click);
    /// `Selection::right_click`. True when the set changed.
    fn right_click(&mut self, ix: usize) -> bool;
    fn clear_selection(&mut self);
    fn move_focus(&mut self, delta: i32, extend: bool);
    fn move_page(&mut self, rows: usize, down: bool, extend: bool);
    fn move_edge(&mut self, end: bool, extend: bool);
    fn select_all(&mut self);
    fn toggle_focus(&mut self);
    /// The selected rows in the view, by index, in display order.
    fn selected_in_view(&self) -> SmallVec<[usize; 8]>;

    /// `TlTreeTable::paused`.
    fn paused(&self) -> bool;
    /// `TlTreeTable::pressed`.
    fn pressed(&self) -> bool;
    fn set_pressed(&mut self);
    /// Clears `pressed`; true when it was set.
    fn take_pressed(&mut self) -> bool;
    /// Keeps the scroll anchor: the row at index `.0`, its rows from the top visible row and
    /// the top row's hidden part. `None` drops it.
    fn set_anchor(&mut self, anchor: Option<(usize, usize, Pixels)>);
    /// The scroll anchor with the row's index in the current version. `None` when there is no
    /// anchor or its row left the view.
    fn anchor(&self) -> Option<(usize, usize, Pixels)>;

    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    fn set_rows_built(&self, rows: u32);
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    fn count_line_canvas(&self);
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    fn quad_logs(&self) -> QuadLogs;
}

/// [`DynRows`] plus what needs the table's `Context`: the app, notify and emit, and the
/// delegate callbacks.
pub(crate) trait DynTreeDelegate: DynRows {
    /// The app behind the table's `Context`.
    fn app(&mut self) -> &mut App;
    /// `Context::notify` on the table.
    fn notify(&mut self);
    /// `Context::emit` on the table.
    fn emit(&mut self, event: TreeTableEvent);
    /// A strong handle to the table, for callbacks that `Context` gives a strong entity.
    fn keep_alive(&self) -> AnyEntity;
    fn name_trailing(
        &mut self,
        ix: usize,
        hovered: bool,
        selected: bool,
        focused: bool,
        window: &mut Window,
    ) -> Option<AnyElement>;
    fn before_rows(&mut self, range: Range<usize>, window: &mut Window);
    /// `on_toggle_expand` for row `ix`.
    fn toggle_expand(&mut self, ix: usize);
    /// `on_toggle_expand` for the row `key` names, in the view or not.
    fn toggle_expand_key(&mut self, key: RowKey);
    /// `on_activate` for row `ix`.
    fn activate(&mut self, ix: usize);
    /// The row menu for the rows `targets`, captured now.
    fn row_menu(&mut self, targets: &[usize], col: Option<ColKey>) -> RowMenu;
    /// Reports the demand for `visible` to the delegate when its id set changed.
    fn report_demand(&mut self, visible: Range<usize>);
}

/// The test-hook logs of painted guide and separator quads.
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
pub(crate) type QuadLogs = (
    Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
);

/// What every row of one frame shares.
struct FrameCtx {
    m: UiMetrics,
    theme: Arc<TlTheme>,
    hairline: Pixels,
    total_width: Pixels,
    guide_lines: bool,
    separators: bool,
    visible: Arc<[ColKey]>,
    offsets: Arc<[Pixels]>,
    name_col: Option<ColKey>,
}

/// The table state that does not depend on the delegate type.
pub(crate) struct TreeTableCore {
    pub(crate) link: Link,
    pub(crate) scroll: UniformListScrollHandle,
    /// Shared by header and body: how far the content is scrolled left. Read from `scroll` on
    /// every render.
    pub(crate) x_offset: Pixels,
    /// Created in `new`, never in render.
    pub(crate) focus: FocusHandle,
    pub(crate) text_cache: CellTextCache<RowKey, ColKey>,
    /// Image sources of the Name cells' file icons, reused across frames.
    pub(crate) icons: IconSourceCache,
    /// `tables.column_separators`, passed in by the owner.
    pub(crate) column_separators: bool,
    pub(crate) hover_row: Option<usize>,
    /// Bumped by every press, so a release timer of an earlier press does nothing.
    press_gen: u64,
    /// The body rows the last frame built, by flattened index.
    pub(crate) visible_range: Range<usize>,
    /// Executor time of the last wheel event over the body.
    last_wheel: Option<Instant>,
    /// True while the scrollbar thumb is dragged. Shared with the scrollbar's handle.
    thumb_dragging: Rc<Cell<bool>>,
    /// True while a demand check waits for the next frame.
    demand_scheduled: bool,
    pub(crate) last_version: u64,
    pub(crate) last_style_version: u64,
    /// Calls of the body's item builder in the current frame. The `uniform_list` calls it twice
    /// to measure one row before it asks for the visible range.
    pub(crate) list_calls: u8,
    /// Whether this frame's probe row carried the scrollbar gutter, or `None` when no probe ran.
    /// The probe reads the body width of the last frame, so after the frame
    /// [`Self::check_gutter`] compares it with the new width.
    pub(crate) probe_gutter: Option<bool>,
    /// Truncation flags of the hovered row's cells, by visible column position. Kept across
    /// frames so a cut cell gets its tooltip on the next render.
    hover_clips: Vec<Rc<Cell<bool>>>,
    /// Shared truncation flag of every cell outside the hovered row. Never read.
    clip_sink: Rc<Cell<bool>>,
    /// False until the hovered row has rendered once and set its truncation flags.
    hover_tips_ready: bool,
    /// Scratch for `delegate.cell`, reused for every cell.
    pub(crate) out: CellOut,
}

/// Wraps a core handler as an element listener that enters the table through `link`.
fn listen<E: ?Sized + 'static>(
    link: &Link,
    f: impl Fn(&mut TreeTableCore, &E, &mut Window, &mut dyn DynTreeDelegate) + 'static,
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let link = link.clone();
    move |ev: &E, window: &mut Window, cx: &mut App| {
        link(cx, &mut |core, host| f(core, ev, window, host));
    }
}

impl TreeTableCore {
    pub(crate) fn new(link: Link, focus: FocusHandle) -> Self {
        Self {
            link,
            scroll: UniformListScrollHandle::new(),
            x_offset: Pixels::ZERO,
            focus,
            text_cache: CellTextCache::new(),
            icons: IconSourceCache::new(),
            column_separators: false,
            hover_row: None,
            press_gen: 0,
            visible_range: 0..0,
            last_wheel: None,
            thumb_dragging: Rc::new(Cell::new(false)),
            demand_scheduled: false,
            last_version: 0,
            last_style_version: 0,
            list_calls: 0,
            probe_gutter: None,
            hover_clips: Vec::new(),
            clip_sink: Rc::new(Cell::new(false)),
            hover_tips_ready: false,
            out: CellOut::default(),
        }
    }

    /// Scroll anchoring (UI-PRC-010): the top-most visible selectable row of the last frame
    /// keeps its y, sub-row offset included. When it is gone the scroll position stays. Skipped
    /// during Ctrl-pause, a thumb drag, and for [`WHEEL_QUIET`] after a wheel event.
    pub(crate) fn keep_anchor(&mut self, row_h: Pixels, host: &mut dyn DynTreeDelegate) {
        if host.paused() || self.thumb_dragging.get() {
            return;
        }
        if let Some(t) = self.last_wheel
            && host
                .app()
                .background_executor()
                .now()
                .saturating_duration_since(t)
                < WHEEL_QUIET
        {
            return;
        }
        let Some((ix, rows_from_top, sub)) = host.anchor() else {
            return;
        };
        let top = (row_h * ix.saturating_sub(rows_from_top) + sub).max(Pixels::ZERO);
        let x = ScrollbarHandle::offset(&self.scroll).x;
        ScrollbarHandle::set_offset(&self.scroll, point(x, -top));
    }

    /// After a keyboard focus move: scrolls the least amount that keeps one row of margin
    /// between the focus row and the header or the bottom edge. gpui's `Nearest` with an offset
    /// keeps the margin at the top only, so the bottom case targets the row below.
    fn keep_focus_visible(&self, host: &mut dyn DynTreeDelegate) {
        let Some(ix) = host.focus_index() else {
            return;
        };
        let row_h = metrics(host.app()).row_height;
        let scroll_top = -ScrollbarHandle::offset(&self.scroll).y;
        if row_h * ix < scroll_top + row_h {
            self.scroll
                .scroll_to_item_with_offset(ix, ScrollStrategy::Nearest, 1);
        } else {
            let below = (ix + 1).min(host.row_count().saturating_sub(1));
            self.scroll.scroll_to_item(below, ScrollStrategy::Nearest);
        }
    }

    /// Renders once more when the last frame's rows were measured with the wrong gutter: on the
    /// first frame, or after the body width changed so the columns started or stopped
    /// overflowing. Without it the last column could stay under the vertical scrollbar until
    /// the next update.
    fn check_gutter(&mut self, host: &mut dyn DynTreeDelegate) {
        if let Some(drawn) = self.probe_gutter.take()
            && drawn != self.horizontal_overflow(host.total_width())
        {
            host.notify();
        }
    }

    /// True when columns `total_width` wide are wider than the body, so the horizontal
    /// scrollbar shows. Uses the body bounds of the last frame.
    pub(crate) fn horizontal_overflow(&self, total_width: Pixels) -> bool {
        let viewport = ScrollbarHandle::viewport_bounds(&self.scroll).size.width;
        viewport > Pixels::ZERO && total_width > viewport
    }

    fn set_hover(&mut self, row: Option<usize>, host: &mut dyn DynTreeDelegate) {
        if row != self.hover_row {
            self.hover_row = row;
            self.hover_tips_ready = false;
            for flag in &self.hover_clips {
                flag.set(false);
            }
            host.notify();
        } else if row.is_some() && !self.hover_tips_ready {
            // The hovered row has now set its truncation flags; render once more so cut cells
            // get their tooltips.
            self.hover_tips_ready = true;
            host.notify();
        }
    }

    /// The fill of row `ix` (UI-PRC-002), see [`row_fill`].
    fn row_background(&self, ix: usize, theme: &TlTheme, host: &dyn DynRows) -> Hsla {
        row_fill(
            ix,
            host.row_kind(ix) == RowKind::GroupHeader,
            || host.is_selected(ix),
            self.hover_row == Some(ix),
            || host.row_tint(ix, theme),
            theme,
        )
    }

    /// Builds the rows of `range` for the body. The two measuring calls get an empty row of the
    /// same size.
    fn build_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) -> Vec<AnyElement> {
        let call = self.list_calls;
        self.list_calls = self.list_calls.saturating_add(1);
        let m = *metrics(host.app());
        let total_width = host.total_width();
        // When the columns overflow, the rows get the scrollbar's width on the right, so the
        // last column can scroll out from under the vertical scrollbar.
        let tail = if self.horizontal_overflow(total_width) {
            px(SCROLLBAR_W * m.ui_scale)
        } else {
            Pixels::ZERO
        };
        // The list measures its content width from this first probe row, so it carries the
        // gutter too.
        if call < 2 && range.len() == 1 {
            self.probe_gutter = Some(tail > Pixels::ZERO);
            return vec![
                div()
                    .w(total_width + tail)
                    .h(m.row_height)
                    .into_any_element(),
            ];
        }
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        host.set_rows_built(range.len() as u32);
        self.record_frame(range.clone(), m.row_height, host);
        host.before_rows(range.clone(), window);

        let layout = host.layout();
        let frame = FrameCtx {
            m,
            theme: crate::theme::current(host.app()).clone(),
            hairline: UiMetrics::hairline(window),
            total_width,
            guide_lines: host.guide_lines(),
            separators: self.column_separators,
            visible: layout.visible,
            offsets: layout.offsets,
            name_col: host.name_column(),
        };
        let columns = frame.visible.len();
        if self.hover_clips.len() < columns {
            self.hover_clips
                .resize_with(columns, || Rc::new(Cell::new(false)));
        }
        let mut rows = Vec::with_capacity(range.len());
        for ix in range {
            let row = self.build_row(ix, &frame, window, host);
            rows.push(if tail > Pixels::ZERO {
                div()
                    .w(total_width + tail)
                    .h(m.row_height)
                    .child(row)
                    .into_any_element()
            } else {
                row
            });
        }
        rows
    }

    /// Keeps the frame's row range and anchor row. Runs inside the list's prepaint, after any
    /// pending scroll was applied.
    fn record_frame(&mut self, range: Range<usize>, row_h: Pixels, host: &mut dyn DynRows) {
        let scroll_top = -ScrollbarHandle::offset(&self.scroll).y;
        let sub = (scroll_top - row_h * range.start).max(Pixels::ZERO);
        // At the very top there is nothing to hold: a new view must show its own first rows,
        // not follow the old top row to wherever the sort moved it.
        let anchor = if scroll_top <= Pixels::ZERO {
            None
        } else {
            range
                .clone()
                .find(|&ix| host.selectable(ix))
                .map(|ix| (ix, ix - range.start, sub))
        };
        host.set_anchor(anchor);
        self.visible_range = range;
    }

    fn build_row(
        &mut self,
        ix: usize,
        f: &FrameCtx,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) -> AnyElement {
        let m = &f.m;
        let theme = &*f.theme;
        let kind = host.row_kind(ix);
        let mut row = div()
            .relative()
            .w(f.total_width)
            .h(m.row_height)
            .bg(self.row_background(ix, theme, host));

        let visible = &f.visible;
        let offsets = &f.offsets;
        let name_col = f.name_col;

        let key = host.row_key(ix);
        row = row.on_mouse_down(
            MouseButton::Left,
            listen(
                &self.link,
                move |this, ev: &MouseDownEvent, window, host| {
                    this.row_mouse_down(key, ev, window, host);
                },
            ),
        );

        if kind == RowKind::GroupHeader {
            row = row.tl_selector(|| format!("TlTreeTable-group-row-{ix}"));
            if ix > 0 {
                row = row.child(
                    div()
                        .tl_selector(|| format!("TlTreeTable-group-line-{ix}"))
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(f.total_width)
                        .h(f.hairline)
                        .bg(theme.divider),
                );
            }
            if let Some(pos) = name_col.and_then(|c| visible.iter().position(|v| *v == c)) {
                // The label stays at the left edge of the viewport while the table scrolls
                // sideways, as far as the row reaches.
                let x = offsets[pos];
                let shift =
                    (self.x_offset - x).clamp(Pixels::ZERO, (f.total_width - x).max(Pixels::ZERO));
                let x = x + shift;
                let ctx = CellCtx {
                    m,
                    theme,
                    selected: false,
                    width: f.total_width - x,
                    at: CellAt::Row(ix, visible[pos].name),
                };
                let name = host.name_cell(ix);
                let cell = paint_group_header_cell(name, host.row_expandable(ix), &ctx);
                row = row.child(cell.absolute().top_0().left(x));
            }
            return row.into_any_element();
        }

        let selected = host.is_selected(ix);
        let hovered = self.hover_row == Some(ix);
        // Cell backgrounds show only under the zebra; hover and selection cover them.
        let plain = !selected && !hovered;
        let flags = host.row_flags(ix);
        let stale = flags.has(RowFlags::STALE);
        let tips = hovered && self.hover_tips_ready;
        let row_key = host.row_hash(ix);
        let focused = host.is_focus(ix);
        // The Name cell's tree guides, when this row draws any: (depth, cell left, cell width).
        let mut guides = None;

        row = row.on_mouse_move(listen(
            &self.link,
            move |this, _: &MouseMoveEvent, _, host| {
                this.set_hover(Some(ix), host);
            },
        ));

        for (pos, &col) in visible.iter().enumerate() {
            let x = offsets[pos];
            let width = offsets.get(pos + 1).copied().unwrap_or(f.total_width) - x;
            let ctx = CellCtx {
                m,
                theme,
                selected,
                width,
                at: CellAt::Row(ix, col.name),
            };
            if Some(col) == name_col {
                let mut name = host.name_cell(ix);
                if (hovered || selected)
                    && let Some(trailing) =
                        host.name_trailing(ix, hovered, selected, focused, window)
                {
                    name.trailing = Some(trailing);
                }
                let depth = host.row_depth(ix);
                let expandable = host.row_expandable(ix);
                if f.guide_lines && (depth > 0 || flags.ancestor_continues != 0) {
                    guides = Some((depth, x, width));
                }
                let weight = host.name_weight(ix);
                let flat = host.flat_names();
                let mut cell = paint_name_cell(
                    name,
                    weight,
                    depth,
                    expandable,
                    flat,
                    flags,
                    &ctx,
                    tips.then_some(row_key),
                    &mut self.icons,
                    window,
                    host.app(),
                );
                if tips && let Some(view) = host.row_tooltip(ix) {
                    let key =
                        ElementId::NamedInteger(SharedString::new_static("tl-row-tip"), row_key);
                    let cx = host.app();
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
                    row =
                        row.child(self.chevron_hit(
                            ix, key, expanded, left, hovered, row_key, m, window, host,
                        ));
                }
                continue;
            }

            let (mut cell, text) = self.build_cell(ix, key, col, pos, hovered, &ctx, window, host);
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
                cell = self.attach_cell_tip(cell, col, pos, text, row_key, window, host);
            }
            row = row.child(cell.absolute().top_0().left(x));
        }
        if f.separators || guides.is_some() {
            // Painted after the cells so the cell fills do not cover the lines.
            row = row.child(self.line_canvas(f, flags, guides, host));
        }
        if !selected && focused {
            // The keyboard caret on a row that is not selected (UI-PRC-008).
            row = row.child(
                div()
                    .tl_selector(|| "TlTreeTable-focus-row".into())
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
        f: &FrameCtx,
        flags: RowFlags,
        guides: Option<(u8, Pixels, Pixels)>,
        host: &dyn DynRows,
    ) -> impl IntoElement {
        let offsets = f.separators.then(|| f.offsets.clone());
        let total_width = f.total_width;
        let line = f.hairline;
        let row_h = f.m.row_height;
        let ui_scale = f.m.ui_scale;
        let divider = f.theme.divider;
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        let (guide_log, separator_log) = {
            host.count_line_canvas();
            host.quad_logs()
        };
        #[cfg(not(any(test, debug_assertions, feature = "test-hooks")))]
        let _ = host;
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
                            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
                        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
        ix: usize,
        key: RowKey,
        expanded: bool,
        left: Pixels,
        hovered: bool,
        row_key: u64,
        m: &UiMetrics,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) -> Div {
        let s = m.ui_scale;
        let hit = div()
            .tl_selector(|| format!("TlTreeTable-chevron-{ix}"))
            .absolute()
            .top_0()
            .left(left - px((CHEVRON_HIT_W - CHEVRON) / 2.0 * s))
            .w(px(CHEVRON_HIT_W * s))
            .h(m.row_height)
            .on_mouse_down(
                MouseButton::Left,
                listen(&self.link, move |this, _: &MouseDownEvent, window, host| {
                    let cx = host.app();
                    cx.stop_propagation();
                    window.focus(&this.focus, cx);
                    host.toggle_expand_key(key);
                }),
            );
        if !hovered {
            return hit;
        }
        let tip_key = ElementId::NamedInteger(SharedString::new_static("tl-chevron-tip"), row_key);
        let label = if expanded {
            "fnd-table-collapse"
        } else {
            "fnd-table-expand"
        };
        let cx = host.app();
        let state = HoverTooltip::state(tip_key, window, cx);
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
        key: RowKey,
        col: ColKey,
        pos: usize,
        hovered: bool,
        ctx: &CellCtx,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) -> (Div, SharedString) {
        self.out.reset();
        // Only the hovered row attaches tooltips, so only it asks for the ones that allocate.
        self.out.want_tooltip = hovered && self.hover_tips_ready;
        host.cell(ix, col, &mut self.out);
        let kind = host.col_kind(col);
        let flag = if hovered {
            self.hover_clips[pos].clone()
        } else {
            self.clip_sink.clone()
        };
        let text = match self.out.arc.clone() {
            Some(arc) if kind == ColumnKind::Path => {
                let room = (ctx.width - ctx.m.sp[4] * 2.0).max(Pixels::ZERO);
                let font_size = ctx.m.text.base;
                self.text_cache.path(key, col, room, &arc, |source, max| {
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
            Some(arc) => self.text_cache.get_or_insert_arc(key, col, &arc),
            None => self.formatted_text(ix, key, col, host),
        };
        let cell = value_cell(kind, text.clone(), &self.out, flag, ctx);
        (cell, text)
    }

    /// The cached display text of a cell whose delegate output in `self.out` has no `arc`.
    /// Formats it on a miss.
    fn formatted_text(
        &mut self,
        ix: usize,
        key: RowKey,
        col: ColKey,
        host: &dyn DynRows,
    ) -> SharedString {
        self.text_cache
            .get_or_format(key, col, self.out.bits, self.out.validity, |s| {
                let mut full = CellOut {
                    text: std::mem::take(s),
                    want_text: true,
                    ..CellOut::default()
                };
                host.cell(ix, col, &mut full);
                *s = full.text;
            })
    }

    /// The hover tooltip of a metric cell on the hovered row: the delegate's explicit tooltip,
    /// else the full text when the cell was cut. `text` is the text the cell shows.
    #[allow(clippy::too_many_arguments)]
    fn attach_cell_tip(
        &self,
        cell: Div,
        col: ColKey,
        pos: usize,
        text: SharedString,
        row_key: u64,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) -> Div {
        let key = ElementId::NamedInteger(SharedString::new_static(col.name), row_key);
        let path_cut = match &self.out.arc {
            Some(arc) if host.col_kind(col) == ColumnKind::Path => {
                (text.as_str() != &**arc).then(|| SharedString::from(arc.clone()))
            }
            _ => None,
        };
        if let Some(tip) = self.out.tooltip.clone() {
            let delay = host.cell_tip_delay();
            let content = host.cell_tip_content(col, tip).build();
            let cx = host.app();
            let state = HoverTooltip::state(key, window, cx);
            return HoverTooltip::attach(&state, cell, delay, content, window, cx);
        }
        let cx = host.app();
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

    /// Left press on the row `key` (UI-PRC-008, UI-PRC-002). A press on a group header toggles
    /// it and never touches the selection: the plain `Selection` gestures cannot tell a header
    /// from a row.
    fn row_mouse_down(
        &mut self,
        key: RowKey,
        ev: &MouseDownEvent,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        let cx = host.app();
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        let Some(ix) = host.key_index(key) else {
            return;
        };
        host.set_pressed();
        self.press_gen += 1;
        self.release_press_after(PRESS_HOLD_MAX, host);
        let double = ev.click_count == 2;
        if host.row_kind(ix) == RowKind::GroupHeader {
            // The first press of a double-click has toggled already; the second does nothing, so
            // a double-click toggles once.
            if !double {
                host.toggle_expand(ix);
            }
            return;
        }
        if double {
            if !host.double_click_expands() {
                host.emit(TreeTableEvent::Activate { row_index: ix });
                host.activate(ix);
                // A delegate that propagates for the keyboard path must not let the press reach
                // the body, which would clear the selection Properties opens on.
                host.app().stop_propagation();
            } else if host.row_expandable(ix).is_some() {
                host.toggle_expand(ix);
            }
            return;
        }
        if !host.selectable(ix) {
            return;
        }
        let click = match (ev.modifiers.control, ev.modifiers.shift) {
            (true, true) => Click::CtrlShift,
            (true, false) => Click::Ctrl,
            (false, true) => Click::Shift,
            (false, false) => Click::Plain,
        };
        host.click(ix, click);
        host.notify();
    }

    /// The left button is up, over the table or anywhere else in the window. The hold ends after
    /// [`PRESS_HOLD_AFTER_UP`] unless another press comes first (a double-click).
    fn release_press(&mut self, host: &mut dyn DynTreeDelegate) {
        if host.pressed() {
            self.release_press_after(PRESS_HOLD_AFTER_UP, host);
        }
    }

    /// Ends the hold of the current press after `wait`; a later press cancels it.
    fn release_press_after(&mut self, wait: Duration, host: &mut dyn DynTreeDelegate) {
        let gen_at = self.press_gen;
        let link = self.link.clone();
        host.app()
            .spawn(async move |cx: &mut AsyncApp| {
                cx.background_executor().timer(wait).await;
                cx.update(|cx| {
                    link(cx, &mut |core, host| {
                        if core.press_gen == gen_at && host.take_pressed() {
                            host.emit(TreeTableEvent::PressReleased);
                        }
                    });
                });
            })
            .detach();
    }

    /// Left press on the body below the last row: nothing selected.
    fn empty_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        window.focus(&self.focus, host.app());
        if !host.selection_empty() || host.has_focus_row() {
            host.clear_selection();
            host.notify();
        }
    }

    fn on_move_focus(&mut self, a: &MoveFocus, _: &mut Window, host: &mut dyn DynTreeDelegate) {
        host.move_focus(a.delta, a.extend);
        self.keep_focus_visible(host);
        host.notify();
    }

    /// PgUp or PgDn: by the whole rows that fit the body, minus one, at least one.
    fn on_move_focus_page(
        &mut self,
        a: &MoveFocusPage,
        _: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        let viewport = ScrollbarHandle::viewport_bounds(&self.scroll).size.height;
        let row_h = metrics(host.app()).row_height;
        let fit = if row_h > Pixels::ZERO {
            (viewport / row_h).floor().max(0.0) as usize
        } else {
            0
        };
        host.move_page(fit.saturating_sub(1).max(1), a.down, a.extend);
        self.keep_focus_visible(host);
        host.notify();
    }

    fn on_move_focus_edge(
        &mut self,
        a: &MoveFocusEdge,
        _: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        host.move_edge(a.end, a.extend);
        self.keep_focus_visible(host);
        host.notify();
    }

    fn on_select_all(&mut self, _: &SelectAll, _: &mut Window, host: &mut dyn DynTreeDelegate) {
        host.select_all();
        host.notify();
    }

    fn on_toggle_select_focus(
        &mut self,
        _: &ToggleSelectFocus,
        _: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        host.toggle_focus();
        host.notify();
    }

    /// Right: a collapsed row with children expands; an expanded row hands focus and selection
    /// to its first child; a leaf does nothing.
    fn on_expand_or_child(
        &mut self,
        _: &ExpandOrChild,
        _: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        let Some(ix) = host.focus_index() else {
            return;
        };
        match host.row_expandable(ix) {
            Some(false) => host.toggle_expand(ix),
            Some(true) => {
                let child = ix + 1;
                if child < host.row_count()
                    && host.row_depth(child) == host.row_depth(ix).saturating_add(1)
                    && host.selectable(child)
                {
                    host.click(child, Click::Plain);
                    self.keep_focus_visible(host);
                    host.notify();
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
        host: &mut dyn DynTreeDelegate,
    ) {
        let Some(ix) = host.focus_index() else {
            return;
        };
        if host.row_expandable(ix) == Some(true) {
            host.toggle_expand(ix);
            return;
        }
        let depth = host.row_depth(ix);
        if depth == 0 {
            return;
        }
        let Some(parent) = (0..ix).rev().find(|&p| host.row_depth(p) < depth) else {
            return;
        };
        if host.row_kind(parent) == RowKind::GroupHeader {
            host.toggle_expand(parent);
        } else if host.selectable(parent) {
            host.click(parent, Click::Plain);
            self.keep_focus_visible(host);
            host.notify();
        }
    }

    /// Space: toggles the focus row; nothing on a leaf.
    fn on_toggle_expand(
        &mut self,
        _: &ToggleExpand,
        _: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        let Some(ix) = host.focus_index() else {
            return;
        };
        if host.row_expandable(ix).is_some() {
            host.toggle_expand(ix);
        }
    }

    /// Enter, Alt+Enter, Ctrl+I: a group header toggles, any other row is activated (D-008).
    fn on_open_properties(
        &mut self,
        _: &OpenProperties,
        _: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        let Some(ix) = host.focus_index() else {
            return;
        };
        if host.row_kind(ix) == RowKind::GroupHeader {
            host.toggle_expand(ix);
        } else {
            host.activate(ix);
        }
    }

    /// The flattened index of the row under window y `y`, from the body bounds of the last frame.
    fn row_at_y(&self, y: Pixels, row_h: Pixels, host: &dyn DynRows) -> Option<usize> {
        let body = ScrollbarHandle::viewport_bounds(&self.scroll);
        if row_h <= Pixels::ZERO || y < body.origin.y || y >= body.bottom() {
            return None;
        }
        let top = y - body.origin.y - ScrollbarHandle::offset(&self.scroll).y;
        let ix = (top / row_h).floor() as usize;
        (ix < host.row_count()).then_some(ix)
    }

    /// The visible column under window x `x`. Right of the last column: the last column.
    fn column_at_x(&self, x: Pixels, host: &mut dyn DynRows) -> Option<ColKey> {
        let body = ScrollbarHandle::viewport_bounds(&self.scroll);
        let content_x = x - body.origin.x - ScrollbarHandle::offset(&self.scroll).x;
        let layout = host.layout();
        let pos = layout
            .offsets
            .partition_point(|&o| o <= content_x)
            .saturating_sub(1);
        layout.visible.get(pos).copied()
    }

    /// Right press on the body (UI-PRC-008). One handler for every row: the row and the column
    /// come from the pointer position. An unselected row becomes the only selection, a selected
    /// row keeps the set, a group header changes nothing and is the only target.
    fn body_right_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        window.focus(&self.focus, host.app());
        let row_h = metrics(host.app()).row_height;
        let Some(ix) = self.row_at_y(ev.position.y, row_h, host) else {
            return;
        };
        let Some(col) = self.column_at_x(ev.position.x, host) else {
            return;
        };
        let targets = if host.row_kind(ix) == RowKind::GroupHeader {
            smallvec![ix]
        } else {
            if !host.selectable(ix) {
                return;
            }
            if host.right_click(ix) {
                host.notify();
            }
            host.selected_in_view()
        };
        host.app().stop_propagation();
        self.open_menu(&targets, Some(col), ev.position, window, host);
    }

    /// Shift+F10 and the Menu key: the menu for the current selection, its top left corner at
    /// the focus row's Name text start and the row's bottom edge (UI-PRC-009). Nothing without
    /// a focus row or a selection.
    fn on_open_context_menu(
        &mut self,
        _: &OpenContextMenu,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        let Some(ix) = host.focus_index() else {
            return;
        };
        let targets = host.selected_in_view();
        let Some(col) = host.name_column() else {
            return;
        };
        if targets.is_empty() {
            return;
        }
        let at = self.focus_menu_anchor(ix, col, host);
        self.open_menu(&targets, None, at, window, host);
    }

    /// Window position of row `ix`'s Name text start at the row's bottom edge. When the Name
    /// column is hidden: the row's left edge plus `sp4`.
    fn focus_menu_anchor(
        &self,
        ix: usize,
        name_col: ColKey,
        host: &mut dyn DynTreeDelegate,
    ) -> Point<Pixels> {
        let m = *metrics(host.app());
        let s = m.ui_scale;
        let body = ScrollbarHandle::viewport_bounds(&self.scroll);
        let scroll = ScrollbarHandle::offset(&self.scroll);
        let layout = host.layout();
        let x = match layout.visible.iter().position(|c| *c == name_col) {
            Some(pos) => {
                let cell = host.name_cell(ix);
                let icon = match cell.icon {
                    NameIcon::None => 0.0,
                    _ => ICON_GAP + icon_px(cell.icon_size),
                };
                let start = if host.flat_names() {
                    m.sp[4] + px(icon * s)
                } else {
                    name_indent(host.row_depth(ix), s) + px((CHEVRON + icon + ICON_GAP) * s)
                };
                layout.offsets[pos] + start + scroll.x
            }
            None => m.sp[4],
        };
        let y = m.row_height * (ix + 1) + scroll.y;
        point(body.origin.x + x, body.origin.y + y)
    }

    /// Opens the delegate's menu for the rows `targets` at `at`. The targets are captured now,
    /// so a re-sort or a new version before an item runs does not change them.
    fn open_menu(
        &mut self,
        targets: &[usize],
        col: Option<ColKey>,
        at: Point<Pixels>,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) {
        let RowMenu { build, closed } = host.row_menu(targets, col);
        let focus = self.focus.clone();
        // Built once this update ends: the delegate needs the table's own context, and
        // `TlMenu::build` hands its closure a plain `App`.
        window.defer(host.app(), move |window, cx| {
            let menu = TlMenu::build(window, cx, focus, build);
            let sub = window.subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                // The item's action is queued before the dismiss, so queue behind it.
                let closed = closed.clone();
                window.defer(cx, move |_, cx| closed(cx));
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
        host: &mut dyn DynTreeDelegate,
    ) {
        let rows = host.selected_in_view();
        if rows.is_empty() {
            return;
        }
        let text = self.rows_as_tsv(&rows, host);
        let cx = host.app();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        host.emit(TreeTableEvent::CopiedRows { rows: rows.len() });
    }

    /// The labels of the visible columns joined by tabs, then one line per row in `rows` with
    /// each visible column's text. Cells copy the cached display text, the Name column its
    /// primary text and glyph cells their copy text (UI-FND-026). Group headers are left out.
    pub(crate) fn rows_as_tsv(&mut self, rows: &[usize], host: &mut dyn DynRows) -> String {
        let visible = host.layout().visible;
        let name_col = host.name_column();
        let mut text = String::new();
        for (pos, &col) in visible.iter().enumerate() {
            if pos > 0 {
                text.push('\t');
            }
            text.push_str(&host.col_label(col));
        }
        for &ix in rows {
            if host.row_kind(ix) == RowKind::GroupHeader {
                continue;
            }
            text.push('\n');
            for (pos, &col) in visible.iter().enumerate() {
                if pos > 0 {
                    text.push('\t');
                }
                if Some(col) == name_col {
                    text.push_str(&host.name_cell(ix).primary);
                } else {
                    let key = host.row_key(ix);
                    text.push_str(&self.copy_cell_text(ix, key, col, host));
                }
            }
        }
        text
    }

    /// The text one metric cell copies: the full source of an `arc` cell, else the cached
    /// display text, with the glyph copy text for a non-valid cell.
    fn copy_cell_text(
        &mut self,
        ix: usize,
        key: RowKey,
        col: ColKey,
        host: &dyn DynRows,
    ) -> SharedString {
        self.out.reset();
        host.cell(ix, col, &mut self.out);
        let text = match self.out.arc.clone() {
            Some(arc) => SharedString::from(arc),
            None => self.formatted_text(ix, key, col, host),
        };
        copy_text(self.out.validity, &text)
    }

    /// The widest content of each column in `targets`, scaled, in `targets` order. The Name
    /// column counts its indent, chevron and icon before the text. Numeric columns measure
    /// digits at their `tnum` width. Texts come from the rows the last frame built, the cell
    /// texts still cached from the last 3 table versions and the delegate's `fit_samples`.
    pub(crate) fn widest_texts(
        &mut self,
        targets: &[ColKey],
        m: &UiMetrics,
        window: &Window,
        host: &mut dyn DynTreeDelegate,
    ) -> SmallVec<[Pixels; 24]> {
        let s = m.ui_scale;
        let name_col = host.name_column();
        let count = host.row_count();
        let rows = self.visible_range.start.min(count)..self.visible_range.end.min(count);
        // Texts of the visible rows: (target position, text, width before the text).
        let mut texts: Vec<(usize, SharedString, Pixels)> =
            Vec::with_capacity(rows.len() * targets.len());
        for ix in rows {
            if host.row_kind(ix) == RowKind::GroupHeader {
                continue;
            }
            let key = host.row_key(ix);
            for (i, &col) in targets.iter().enumerate() {
                if Some(col) == name_col {
                    let cell = host.name_cell(ix);
                    let icon = match cell.icon {
                        NameIcon::None => 0.0,
                        _ => ICON_GAP + icon_px(cell.icon_size),
                    };
                    let prefix =
                        name_indent(host.row_depth(ix), s) + px((CHEVRON + icon + ICON_GAP) * s);
                    texts.push((i, cell.primary, prefix));
                    continue;
                }
                self.out.reset();
                host.cell(ix, col, &mut self.out);
                let text = match self.out.arc.clone() {
                    Some(arc) => SharedString::from(arc),
                    None => self.formatted_text(ix, key, col, host),
                };
                texts.push((i, text, Pixels::ZERO));
            }
        }
        for (i, &col) in targets.iter().enumerate() {
            for text in host.fit_samples(col) {
                texts.push((i, SharedString::from(text), Pixels::ZERO));
            }
        }
        let numeric: SmallVec<[bool; 24]> = targets
            .iter()
            .map(|&c| host.col_kind(c) == ColumnKind::Numeric)
            .collect();
        let size = m.text.base;
        let text_cache = &self.text_cache;
        let cx = host.app();
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

    /// The body of one frame: the row list, its scrollbars, the empty state, the reorder
    /// insertion line at content x `insert_x` and the focus ring. Schedules the demand report
    /// and the gutter check for after the frame.
    pub(crate) fn render_body(
        &mut self,
        insert_x: Option<Pixels>,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        host: &mut dyn DynTreeDelegate,
    ) -> Stateful<Div> {
        let count = host.row_count();
        if count == 0 {
            // The list builds no rows, so nothing records the frame.
            self.visible_range = 0..0;
            host.set_anchor(None);
        }
        // Demand runs after every render, so a new version with the same range still reports
        // a changed id set (ADR-009).
        if !self.demand_scheduled {
            self.demand_scheduled = true;
            let link = self.link.clone();
            // `Context::on_next_frame` holds the table until the callback ran.
            let keep = host.keep_alive();
            window.on_next_frame(move |_, cx| {
                link(cx, &mut |core, host| {
                    core.demand_scheduled = false;
                    host.report_demand(core.visible_range.clone());
                    core.check_gutter(host);
                });
                drop(keep);
            });
        }
        let empty = if count == 0 { host.empty_text() } else { None };
        let ring = self.focus.is_focused(window)
            && window.last_input_was_keyboard()
            && host.selection_empty()
            && !host.has_focus_row();

        let link = self.link.clone();
        // `Context::processor` holds the table while the list lives.
        let keep = host.keep_alive();
        let list = uniform_list(
            "tl-tree-table-body",
            count,
            move |range: Range<usize>, window: &mut Window, cx: &mut App| {
                let _ = &keep;
                let mut rows = Vec::new();
                link(cx, &mut |core, host| {
                    rows = core.build_rows(range.clone(), window, host);
                });
                rows
            },
        )
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

        let link = &self.link;
        let mut body = div()
            .id("tl-tree-table-body-box")
            .tl_selector(|| "TlTreeTable-body".into())
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .on_scroll_wheel(listen(link, |this, _: &ScrollWheelEvent, _, host| {
                this.last_wheel = Some(host.app().background_executor().now());
            }))
            .on_hover(listen(link, |this, hovered: &bool, _, host| {
                if !*hovered {
                    this.set_hover(None, host);
                }
            }))
            // Rows stop the press, so only a press below the last row gets here.
            .on_mouse_down(MouseButton::Left, listen(link, Self::empty_mouse_down))
            .on_mouse_up(
                MouseButton::Left,
                listen(link, |this, _: &MouseUpEvent, _, host| {
                    this.release_press(host)
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                listen(link, |this, _: &MouseUpEvent, _, host| {
                    this.release_press(host)
                }),
            )
            // The one context-menu handler; rows do not stop the right press.
            .on_mouse_down(MouseButton::Right, listen(link, Self::body_right_down))
            .child(list)
            .child(scrollbar);
        if let Some(text) = empty.clone()
            && host.empty_clear_filter()
        {
            body = body.child(
                div()
                    .tl_selector(|| "TlTreeTable-empty-state".into())
                    .absolute()
                    .top(px(EMPTY_STATE_TOP * m.ui_scale))
                    .left_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(TlEmptyState::new(text).clear_filter(host.empty_clear_action())),
            );
        } else if let Some(text) = empty {
            let mut label = div();
            *label.text_style() = t_base(m);
            body = body.child(
                label
                    .tl_selector(|| "TlTreeTable-empty".into())
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
        if let Some(x) = insert_x {
            body = body.child(
                crate::table::header::insert_line(x - self.x_offset, theme, m)
                    .tl_selector(|| "TlTreeTable-insert".into()),
            );
        }
        if ring {
            body = body.child(
                div()
                    .tl_selector(|| "TlTreeTable-focus-ring".into())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .border(px(m.ui_scale))
                    .border_color(theme.accent),
            );
        }
        body
    }

    /// Adds the handlers of the keyboard and selection actions. CopyRow, CopyCell, CollapseAll
    /// and the header actions have no handler here, so they bubble to the owner page (open item
    /// 16).
    pub(crate) fn key_actions(&self, el: Div) -> Div {
        let link = &self.link;
        el.on_action(listen(link, Self::on_move_focus))
            .on_action(listen(link, Self::on_move_focus_page))
            .on_action(listen(link, Self::on_move_focus_edge))
            .on_action(listen(link, Self::on_select_all))
            .on_action(listen(link, Self::on_toggle_select_focus))
            .on_action(listen(link, Self::on_expand_or_child))
            .on_action(listen(link, Self::on_collapse_or_parent))
            .on_action(listen(link, Self::on_toggle_expand))
            .on_action(listen(link, Self::on_open_properties))
            .on_action(listen(link, Self::on_open_context_menu))
            .on_action(listen(link, Self::on_copy_rows_as_table))
    }
}

/// The fill of row `ix` (UI-PRC-002): the group band, else selected, else hover, else the
/// zebra on the flattened index with the NEW or EXITED tint over it.
pub(crate) fn row_fill(
    ix: usize,
    group: bool,
    selected: impl FnOnce() -> bool,
    hovered: bool,
    tint: impl FnOnce() -> Option<Hsla>,
    theme: &TlTheme,
) -> Hsla {
    if group {
        return theme.header;
    }
    if selected() {
        return theme.row_selected;
    }
    if hovered {
        return theme.row_hover;
    }
    let zebra = if ix.is_multiple_of(2) {
        theme.list_bg
    } else {
        theme.row_alt
    };
    match tint() {
        Some(tint) => zebra.blend(tint),
        None => zebra,
    }
}

/// Builds the menu `build` makes and opens it with its top left corner at `at`. One copy for
/// every table: the closure is boxed.
pub(crate) fn open_boxed_menu(
    at: Point<Pixels>,
    focus: FocusHandle,
    build: MenuBuild,
    window: &mut Window,
    cx: &mut App,
) {
    let menu = TlMenu::build(window, cx, focus, build);
    open_context_menu_at(at, window, cx, menu);
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
