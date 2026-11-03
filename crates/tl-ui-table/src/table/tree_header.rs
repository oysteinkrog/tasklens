//! The header and column logic of `TlTreeTable` (UI-PRC-005, UI-PRC-006, R-180, R-181): header
//! pointer events, the header menu, the column actions, reorder and boundary drags, fitting and
//! the frame's header and root element. Like the body in `tree_core`, it is not generic: column
//! ids are [`ColKey`]s and the delegate is reached through [`DynTreeDelegate`]. The generic
//! header and layout code (`HeaderState`, `TlTreeTableHeader`, `ResizeDrag`, `header_menu`)
//! therefore runs only as `ColKey` code, compiled once in tl-ui-table.

use std::rc::Rc;

use gpui_kit::base::{ScrollbarHandle, v_flex};
use gpui_kit::{
    App, BorrowAppContext as _, Bounds, CursorStyle, Div, InteractiveElement, ParentElement,
    Pixels, Point, SharedString, Styled, Subscription, Window, point, px, size,
};
use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use tl_model::validity::Validity;

use crate::table::actions::{
    EnableAllColumns, HideColumn, OpenColumnPicker, ResetColumns, ResizeColumnsToFit, SortByColumn,
    TABLE_CONTEXT,
};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnError, ColumnLayout, fit_width, min_width,
};
use crate::table::columns_window;
use crate::table::header::{
    self, HeaderCell, HeaderEvent, HeaderListener, HeaderPress, HeaderState, ReorderDrag,
    ResizeDrag, SORT_ARROW, TlTreeTableHeader, beyond_slop, header_menu, header_tooltip,
    insertion_slot, sort_arrow_x,
};
use crate::table::tree_core::{
    ColKey, DynRows, DynTreeDelegate, Link, TreeTableCore, open_boxed_menu,
};
use crate::table::tree_table::{ColumnResizeMode, HeaderLines, TableSort};
use crate::text_cache::CellTextCache;
use tl_ui_base::i18n::tr;
use tl_ui_base::selector::TlSelector;
use tl_ui_base::text::metrics::TextMetricsCache;
use tl_ui_base::theme::{TlTheme, UiMetrics, metrics};
use tl_ui_kit::components::toast::{ToastKind, ToastQueue};

/// The header and column state of one table.
pub(crate) struct HeaderCore {
    /// Header hover, press, drags, totals and this frame's header cells.
    pub(crate) state: HeaderState<ColKey>,
    /// Routes the header's pointer events to [`header_event`]. Created once with the table.
    listener: HeaderListener,
    /// `tables.column_resize`, passed in by the owner.
    pub(crate) resize_mode: ColumnResizeMode,
    /// The owner's default layout for "Reset to default". `None`: `ResetColumns` goes on to the
    /// owner.
    pub(crate) default_layout: Option<ColumnLayout<ColKey>>,
    /// Catches Esc before any key binding while a column reorder drag is in progress.
    reorder_esc: Option<Subscription>,
}

impl HeaderCore {
    pub(crate) fn new(link: &Link) -> Self {
        let listener: HeaderListener = {
            let link = link.clone();
            Rc::new(move |ev, window, cx| {
                link(cx, &mut |core, h| header_event(core, h, ev, window));
            })
        };
        Self {
            state: HeaderState::default(),
            listener,
            resize_mode: ColumnResizeMode::default(),
            default_layout: None,
            reorder_esc: None,
        }
    }
}

/// The header state with typed column ids, as `TlTreeTable::header_state` returns it. A copy
/// made per call; the table keeps the state with [`ColKey`]s.
pub struct HeaderView<C> {
    /// The header cell under the pointer.
    pub hover: Option<C>,
    pub press: Option<HeaderPress<C>>,
    /// The column whose resize band is under the pointer.
    pub boundary_hover: Option<C>,
    /// The cell drag in progress.
    pub reorder: Option<ReorderDrag<C>>,
    /// Content x of the reorder insertion line this frame.
    pub insert_x: Option<Pixels>,
    /// The last frame's visible cells.
    pub cells: Vec<HeaderCell<C>>,
    /// The last frame's sort arrow: column, ascending, bounds in header content coordinates.
    pub arrow: Option<(C, bool, Bounds<Pixels>)>,
    /// The totals cache's counters.
    pub totals: TotalsStats,
    cursor: CursorStyle,
}

impl<C> HeaderView<C> {
    /// The pointer cursor over the header: column-resize over a resize band and during a
    /// boundary drag, else the arrow (UI-FND-003).
    pub fn cursor(&self) -> CursorStyle {
        self.cursor
    }
}

/// Hits and misses of the header totals cache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TotalsStats(u64, u64);

impl TotalsStats {
    /// Hits and misses since the table was created.
    pub fn stats(&self) -> (u64, u64) {
        (self.0, self.1)
    }
}

/// `state` with every key passed through `col`.
pub(crate) fn header_view<C>(
    state: &HeaderState<ColKey>,
    col: impl Fn(ColKey) -> C,
) -> HeaderView<C> {
    let (hits, misses) = state.totals.stats();
    HeaderView {
        hover: state.hover.map(&col),
        press: state.press.map(|p| HeaderPress {
            col: col(p.col),
            at: p.at,
            dragged: p.dragged,
        }),
        boundary_hover: state.boundary_hover.map(&col),
        reorder: state.reorder.map(|d| ReorderDrag {
            col: col(d.col),
            grab: d.grab,
            x: d.x,
            slot: d.slot,
        }),
        insert_x: state.insert_x,
        cells: state
            .cells
            .iter()
            .map(|c| HeaderCell {
                id: col(c.id),
                label: c.label.clone(),
                align: c.align,
                x: c.x,
                w: c.w,
                sort: c.sort,
                total: c.total.clone(),
                fill: c.fill,
            })
            .collect(),
        arrow: state.arrow.map(|(c, asc, b)| (col(c), asc, b)),
        totals: TotalsStats(hits, misses),
        cursor: state.cursor(),
    }
}

/// A sort state as the header menu reads it.
struct SortOf(ColKey, bool);

impl TableSort<ColKey> for SortOf {
    fn column(&self) -> ColKey {
        self.0
    }
    fn ascending(&self) -> bool {
        self.1
    }
}

/// The header, the body and the table's action handlers for one frame.
pub(crate) fn render_table(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
) -> Div {
    let hairline = UiMetrics::hairline(window);
    let lines = h.header_lines();
    prepare_header(core, h, lines, m, theme, window);
    let tooltip = core
        .head
        .state
        .hover
        .and_then(|c| header_tooltip(&h.spec(c)));
    let header = TlTreeTableHeader {
        layout: core.live(),
        lines,
        x_offset: core.x_offset,
        m,
        theme,
        hairline,
        state: &core.head.state,
        listener: core.head.listener.clone(),
    }
    .render(tooltip, window, h.app());

    let insert_x = core.head.state.insert_x;
    let body = core.render_body(insert_x, m, theme, window, h);

    let root = v_flex()
        .tl_selector(|| "TlTreeTable".into())
        .size_full()
        .bg(theme.list_bg)
        .key_context(TABLE_CONTEXT)
        .track_focus(&core.focus)
        .tab_index(0)
        .tab_stop(h.tab_stop());
    // CopyRow, CopyCell, CollapseAll and the header Actions have no handler here, so they
    // bubble to the owner page (open item 16).
    let link = core.link.clone();
    column_actions(&link, core.key_actions(root))
        .child(header)
        .child(body)
}

/// Wraps a column handler as an action listener that enters the table through `link`.
fn listen<E: 'static>(
    link: &Link,
    f: fn(&mut TreeTableCore, &mut dyn DynTreeDelegate, &E, &mut Window),
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let link = link.clone();
    move |a: &E, window: &mut Window, cx: &mut App| {
        link(cx, &mut |core, h| f(core, h, a, window));
    }
}

/// Adds the handlers of the header menu's column actions.
fn column_actions(link: &Link, el: Div) -> Div {
    el.on_action(listen(link, on_sort_by_column))
        .on_action(listen(link, on_resize_columns_to_fit))
        .on_action(listen(link, on_hide_column))
        .on_action(listen(link, on_reset_columns))
        .on_action(listen(link, on_enable_all_columns))
        .on_action(listen(link, on_open_column_picker))
}

/// Column widths are stored at 100%. After a UI scale change the delegate's layout still holds
/// the old scale, so rebuild it at `ui_scale` and hand it back. Skipped during a resize drag,
/// which works on its own copy.
pub(crate) fn follow_ui_scale(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    ui_scale: f32,
) {
    if core.head.state.resize.is_some() || core.layout().ui_scale() == ui_scale {
        return;
    }
    let mut layout = h.fresh_layout();
    layout.rebuild(ui_scale);
    core.apply_columns(&layout, h);
}

/// The header total of `col` as line 2 shows it: the cached text and its validity. `None` when
/// the delegate has no total for `col`. Formats only when the value or validity changed.
pub(crate) fn header_total_text(
    totals: &mut CellTextCache<(), ColKey>,
    out: &mut CellOut,
    h: &dyn DynRows,
    col: ColKey,
) -> Option<(SharedString, Validity)> {
    out.reset();
    if !h.header_total(col, out) {
        return None;
    }
    let validity = out.validity;
    let text = match out.arc.clone() {
        Some(arc) => totals.get_or_insert_arc((), col, &arc),
        None => totals.get_or_format((), col, out.bits, validity, |s| {
            let mut full = CellOut {
                text: std::mem::take(s),
                want_text: true,
                ..CellOut::default()
            };
            h.header_total(col, &mut full);
            *s = full.text;
        }),
    };
    Some((text, validity))
}

/// Fills `head.state.cells` and the sort arrow for this frame. The `Vec` keeps its capacity;
/// texts are `SharedString` clones and cached totals.
fn prepare_header(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    lines: HeaderLines,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &Window,
) {
    let layout = core.live();
    let visible = layout.visible_columns().clone();
    let offsets = layout.offsets().clone();
    let total_width = layout.total_width();
    let (sort_col, ascending) = h.sort_key();
    let state = &mut core.head.state;
    let mut cells = std::mem::take(&mut state.cells);
    cells.clear();
    state.arrow = None;
    for (pos, &id) in visible.iter().enumerate() {
        let x = offsets[pos];
        let w = offsets.get(pos + 1).copied().unwrap_or(total_width) - x;
        let spec = h.spec(id);
        let (label, align, sortable) = (spec.label, spec.align, spec.sortable);
        let sorted = sortable && id == sort_col;
        if sorted {
            let label_w = if align == ColumnAlign::Left {
                state.label_width(id, &label, m.style_version, m, window)
            } else {
                Pixels::ZERO
            };
            let (top, line_h) = header::line1(lines, m);
            let a = px(SORT_ARROW * m.ui_scale);
            let origin = point(
                x + sort_arrow_x(align, w, label_w, m),
                top + (line_h - a) / 2.0,
            );
            state.arrow = Some((id, ascending, Bounds::new(origin, size(a, a))));
        }
        let total = match lines {
            HeaderLines::Two => header_total_text(&mut state.totals, &mut core.out, h, id),
            HeaderLines::One => None,
        };
        let fill = state.fill(id, sortable, theme);
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
    state.cells = cells;
    state.insert_x = state
        .reorder
        .map(|d| offsets.get(d.slot).copied().unwrap_or(total_width));
}

/// The visible column at position `pos` of the live layout.
fn col_at(core: &TreeTableCore, pos: Option<usize>) -> Option<ColKey> {
    pos.and_then(|p| core.live().visible_columns().get(p).copied())
}

/// Pointer events from the header's hitboxes (UI-PRC-005). A left press and release on the
/// same sortable cell, without moving more than 4 px, sorts by that column. Moving more than
/// 4 px drags the column to a new place, except Name. A press on a resize band starts a
/// boundary drag; a double-click on one fits that column.
fn header_event(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    ev: HeaderEvent,
    window: &mut Window,
) {
    let scale = metrics(h.app()).ui_scale;
    match ev {
        HeaderEvent::Move {
            hovered,
            boundary,
            at,
        } => {
            if let Some(drag) = &mut core.head.state.resize {
                drag.drag_to(at.x);
                h.set_drag(Some(&drag.layout));
                h.notify();
                return;
            }
            if core.head.state.reorder.is_some() {
                reorder_to(core, at.x);
                h.notify();
                return;
            }
            let col = col_at(core, hovered);
            let band = col_at(core, boundary);
            let state = &mut core.head.state;
            let mut changed = state.set_hover(col);
            if state.boundary_hover != band {
                state.boundary_hover = band;
                changed = true;
            }
            if let Some(p) = &mut state.press
                && !p.dragged
                && beyond_slop(p.at, at, scale)
            {
                p.dragged = true;
                changed = true;
                let p = *p;
                start_reorder(core, h, p, at, window);
            }
            if changed {
                h.notify();
            }
        }
        HeaderEvent::Down { pos, at } => {
            if let Some(col) = col_at(core, Some(pos)) {
                core.head.state.press = Some(HeaderPress {
                    col,
                    at,
                    dragged: false,
                });
                h.notify();
            }
        }
        HeaderEvent::BoundaryDown {
            pos,
            at,
            click_count,
        } => {
            let Some(col) = col_at(core, Some(pos)) else {
                return;
            };
            core.head.state.press = None;
            if click_count >= 2 {
                fit_columns(core, h, Some(col), window);
            } else {
                start_resize(core, h, col, at, window);
            }
        }
        HeaderEvent::Up { hovered, at } => {
            if let Some(drag) = core.head.state.resize.take() {
                h.set_drag(None);
                if drag.changed() {
                    core.apply_columns(&drag.layout, h);
                }
                h.notify();
                return;
            }
            if let Some(drag) = core.head.state.reorder.take() {
                core.head.state.press = None;
                core.head.reorder_esc = None;
                drop_column(core, h, drag);
                h.notify();
                return;
            }
            let Some(p) = core.head.state.press.take() else {
                return;
            };
            h.notify();
            if p.dragged || beyond_slop(p.at, at, scale) || col_at(core, hovered) != Some(p.col) {
                return;
            }
            sort_by_column(h, p.col);
        }
        HeaderEvent::ContextMenu { at } => {
            let state = &core.head.state;
            if state.resize.is_some() || state.reorder.is_some() {
                return;
            }
            let x = state.content_x(at.x, core.x_offset);
            if let Some(col) = core.live().column_at_x(x) {
                h.app().stop_propagation();
                open_header_menu(core, h, col, at, window);
            }
        }
    }
}

/// Opens the header menu (UI-PRC-006) for `col` with its top left corner at `at`. Its actions
/// run in the table's key context.
fn open_header_menu(
    core: &TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    col: ColKey,
    at: Point<Pixels>,
    window: &mut Window,
) {
    let spec = h.spec(col);
    let layout = h.fresh_layout();
    let (sort_col, ascending) = h.sort_key();
    let sort = SortOf(sort_col, ascending);
    let fixed = h.columns_fixed();
    let mode = core.head.resize_mode;
    let build = Box::new(move |menu, window: &mut Window, cx: &mut App| {
        header_menu(&spec, &layout, &sort, fixed, mode, menu, window, cx)
    });
    open_boxed_menu(at, core.focus.clone(), build, window, h.app());
}

/// `HideColumn` from the header menu: hides the column and calls `on_columns_changed` once.
/// Hiding the last visible column shows the Warning toast instead. A key outside this table's
/// column set goes on to the next handler.
fn on_hide_column(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    a: &HideColumn,
    window: &mut Window,
) {
    let Some(col) = h.parse_col(&a.id) else {
        h.app().propagate();
        return;
    };
    let mut layout = h.fresh_layout();
    match layout.set_visible(col, false) {
        Ok(()) => {
            layout.rebuild(layout.ui_scale());
            core.apply_columns(&layout, h);
            h.notify();
        }
        Err(ColumnError::LastVisible) => {
            let cx = h.app();
            let text = tr(cx, "table-toast-last-visible");
            push_toast(ToastKind::Warning, text, window, cx);
        }
        Err(err) => {
            let col = col.name;
            tracing::debug!(target: "tasklens::ui", ?err, ?col, "hide column refused");
        }
    }
}

/// `ResetColumns` (UI-PRC-006): with the owner's defaults, replaces the layout with them, calls
/// `on_columns_changed` once and shows "Columns reset to default". Without defaults, or for
/// another table, the action goes on to the owner.
fn on_reset_columns(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    a: &ResetColumns,
    window: &mut Window,
) {
    let has_defaults = core
        .head
        .default_layout
        .as_ref()
        .is_some_and(|d| d.table == a.table);
    if !has_defaults {
        h.app().propagate();
        return;
    }
    reset_columns(core, h, window);
}

/// Replaces the layout with the owner's defaults, calls `on_columns_changed` once and shows
/// "Columns reset to default" in `window`. False and no change without defaults.
pub(crate) fn reset_columns(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    window: &mut Window,
) -> bool {
    let scale = core.layout().ui_scale();
    let Some(defaults) = &core.head.default_layout else {
        return false;
    };
    let mut layout = defaults.clone();
    layout.rebuild(scale);
    core.apply_columns(&layout, h);
    h.notify();
    let cx = h.app();
    let text = tr(cx, "table-toast-columns-reset");
    push_toast(ToastKind::Info, text, window, cx);
    true
}

/// `EnableAllColumns` (R-180): shows every column that exists on this system, each in its
/// remembered slot and width, and calls `on_columns_changed` once. Another table's id goes on to
/// the next handler.
fn on_enable_all_columns(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    a: &EnableAllColumns,
    _: &mut Window,
) {
    if a.table != core.layout().table {
        h.app().propagate();
        return;
    }
    let mut layout = h.fresh_layout();
    let ids = layout.order.clone();
    for id in ids {
        if h.spec(id).availability != Availability::NotOnThisSystem {
            let _ = layout.set_visible(id, true);
        }
    }
    layout.rebuild(layout.ui_scale());
    core.apply_columns(&layout, h);
    h.notify();
}

/// `OpenColumnPicker` for this table: opens the picker bound to it, or focuses the open one and
/// binds it here. A table with fixed columns or another id passes the action on.
fn on_open_column_picker(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    a: &OpenColumnPicker,
    window: &mut Window,
) {
    if a.table != core.layout().table || h.columns_fixed() {
        h.app().propagate();
        return;
    }
    let host = h.picker_host();
    let parent = Some(window.window_handle());
    // The picker reads this table while it opens, so it opens after this update.
    h.app()
        .defer(move |cx| columns_window::open_column_picker(host, parent, cx));
}

/// Shows or hides the column `key` for the picker and calls `on_columns_changed` once.
pub(crate) fn set_column_visible(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    key: &str,
    on: bool,
) -> Result<(), ColumnError> {
    let col = h.parse_col(key).ok_or(ColumnError::Unknown)?;
    let mut layout = h.fresh_layout();
    if layout.visible.contains(&col) == on {
        return Ok(());
    }
    layout.set_visible(col, on)?;
    layout.rebuild(layout.ui_scale());
    core.apply_columns(&layout, h);
    h.notify();
    Ok(())
}

/// Turns `press`, which just moved past the slop to window point `at`, into a reorder drag.
/// Name (`hideable == false`) never drags. Esc cancels the drag through a keystroke
/// interceptor, which runs before every key binding, so the global Esc chain never sees it.
fn start_reorder(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    press: HeaderPress<ColKey>,
    at: Point<Pixels>,
    window: &mut Window,
) {
    if !h.spec(press.col).hideable {
        return;
    }
    let layout = core.live();
    let Some(pos) = layout
        .visible_columns()
        .iter()
        .position(|c| *c == press.col)
    else {
        return;
    };
    let grab = core.head.state.content_x(press.at.x, core.x_offset) - layout.offsets()[pos];
    core.head.state.reorder = Some(ReorderDrag {
        col: press.col,
        grab,
        x: Pixels::ZERO,
        slot: pos,
    });
    reorder_to(core, at.x);
    core.head.state.set_hover(None);
    let link = core.link.clone();
    let handle = window.window_handle();
    let esc = h.app().intercept_keystrokes(move |ev, window, cx| {
        if ev.keystroke.key == "escape" && window.window_handle() == handle {
            link(cx, &mut |core, h| cancel_reorder(core, h));
            cx.stop_propagation();
        }
    });
    core.head.reorder_esc = Some(esc);
}

/// Moves the reorder ghost to window x `x` and picks the nearest allowed boundary.
fn reorder_to(core: &mut TreeTableCore, x: Pixels) {
    let x = core.head.state.content_x(x, core.x_offset);
    let layout = core.live();
    let slot = insertion_slot(
        layout.visible_columns(),
        layout.offsets(),
        layout.total_width(),
        x,
    );
    if let Some(drag) = &mut core.head.state.reorder {
        drag.x = x;
        drag.slot = slot;
    }
}

/// Esc during a reorder drag: ends it with no change. The release that follows does nothing.
fn cancel_reorder(core: &mut TreeTableCore, h: &mut dyn DynTreeDelegate) {
    let head = &mut core.head;
    head.state.reorder = None;
    head.state.press = None;
    head.reorder_esc = None;
    h.notify();
}

/// Applies a finished reorder drag: moves the column before the column at the drop slot, or to
/// the end, and calls `on_columns_changed` once. A drop on either edge of the column itself
/// changes nothing.
fn drop_column(core: &mut TreeTableCore, h: &mut dyn DynTreeDelegate, drag: ReorderDrag<ColKey>) {
    let mut layout = h.fresh_layout();
    let visible = layout.visible_columns();
    let Some(pos) = visible.iter().position(|c| *c == drag.col) else {
        return;
    };
    if drag.slot == pos || drag.slot == pos + 1 {
        return;
    }
    let before = visible.get(drag.slot).copied();
    if layout.move_column(drag.col, before).is_err() {
        return;
    }
    layout.rebuild(layout.ui_scale());
    core.apply_columns(&layout, h);
}

/// Starts a boundary drag of `col` at window point `at`, on a copy of the delegate's layout.
fn start_resize(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    col: ColKey,
    at: Point<Pixels>,
    window: &Window,
) {
    let mins = column_mins(core, h, window);
    let layout = h.fresh_layout();
    let scale = layout.ui_scale();
    let body = ScrollbarHandle::viewport_bounds(&core.scroll).size.width;
    // Before the first layout of the body the table's own width stands in.
    let viewport = if body > Pixels::ZERO {
        body
    } else {
        layout.total_width()
    } / scale;
    let start_widths = layout.widths.clone();
    h.set_drag(Some(&layout));
    let head = &mut core.head;
    head.state.resize = Some(ResizeDrag {
        col,
        start_x: at.x,
        start_width: start_widths.get(&col).copied().unwrap_or(Pixels::ZERO),
        start_widths,
        mins,
        viewport,
        mode: head.resize_mode,
        layout,
    });
    head.state.set_hover(None);
    h.notify();
}

/// The minimum width of every visible column at 100% (PROC-023), with the sort arrow's room on
/// the sort column. Measured through the global `TextMetricsCache`.
fn column_mins(
    core: &TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    window: &Window,
) -> FxHashMap<ColKey, Pixels> {
    let (sort_col, _) = h.sort_key();
    let layout = core.layout();
    let scale = layout.ui_scale();
    let specs: SmallVec<[_; 24]> = layout
        .visible_columns()
        .iter()
        .map(|&col| h.spec(col))
        .collect();
    let cx = h.app();
    let m = *metrics(cx);
    if !cx.has_global::<TextMetricsCache>() {
        cx.set_global(TextMetricsCache::default());
    }
    cx.update_global::<TextMetricsCache, _>(|cache, cx| {
        cache.clear_if_stale(m.style_version);
        specs
            .iter()
            .map(|spec| {
                let sorted = spec.sortable && spec.id == sort_col;
                let min = min_width(spec, sorted, cache, &m, window, cx);
                (spec.id, min / scale)
            })
            .collect()
    })
}

/// Fits `only`, or every visible column on `None`, to its content (UI-PRC-006): the largest of
/// the minimum, the widest text + 16 and the header label + 16 + 12. Texts come from the rows
/// the last frame built, the cell texts still cached from the last 3 table versions and the
/// delegate's `fit_samples`, never from a pass over all rows. Then calls `on_columns_changed`
/// once.
fn fit_columns(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    only: Option<ColKey>,
    window: &Window,
) {
    let m = *metrics(h.app());
    let mins = column_mins(core, h, window);
    let mut layout = h.fresh_layout();
    let scale = layout.ui_scale();
    let targets: SmallVec<[ColKey; 24]> = match only {
        Some(col) => smallvec![col],
        None => layout.visible_columns().iter().copied().collect(),
    };
    let widest = core.widest_texts(&targets, &m, window, h);
    for (i, &col) in targets.iter().enumerate() {
        let label = h.spec(col).label;
        let label_w = core
            .head
            .state
            .label_width(col, &label, m.style_version, &m, window);
        let min = mins.get(&col).copied().unwrap_or(Pixels::ZERO) * scale;
        let w = fit_width(min, widest[i], label_w, &m);
        layout.widths.insert(col, w / scale);
    }
    layout.rebuild(scale);
    core.apply_columns(&layout, h);
    h.notify();
}

/// `ResizeColumnsToFit` from the header menu: fits every visible column. An action for another
/// table goes on to the next handler.
fn on_resize_columns_to_fit(
    core: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    a: &ResizeColumnsToFit,
    window: &mut Window,
) {
    if a.table != core.layout().table {
        h.app().propagate();
        return;
    }
    fit_columns(core, h, None, window);
}

/// Asks the delegate to sort by `col` (header click and `SortByColumn`). The delegate picks the
/// direction with `next_sort`. Nothing on an unsortable column.
pub(crate) fn sort_by_column(h: &mut dyn DynTreeDelegate, col: ColKey) {
    if !h.spec(col).sortable {
        return;
    }
    h.sort_by(col);
    h.notify();
}

/// `SortByColumn` from the header menu. A key outside this table's column set is ignored.
fn on_sort_by_column(
    _: &mut TreeTableCore,
    h: &mut dyn DynTreeDelegate,
    a: &SortByColumn,
    _: &mut Window,
) {
    if let Some(col) = h.parse_col(&a.id) {
        sort_by_column(h, col);
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
