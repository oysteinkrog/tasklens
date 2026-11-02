//! The header and column logic of `TlTreeTable` (UI-PRC-005, UI-PRC-006, R-180, R-181): header
//! pointer events, the header menu, the column actions, reorder and boundary drags, fitting and
//! the column picker's view. It reaches the delegate through [`DynColumns`], which the shell's
//! `Host` implements, so it is compiled once per column type instead of once per delegate type.
//! The workspace has 5 column types and 23 delegate types.
//!
//! The column ids stay typed here instead of becoming `ColKey`s like the body's:
//! `TlTreeTable::columns` hands owners the live drag layout as a `ColumnLayout<C>`, and
//! `TlTreeTable::header_state` the `HeaderState<C>`, so both must hold the column type itself.

use std::rc::Rc;

use gpui_kit::base::{ScrollbarHandle, v_flex};
use gpui_kit::{
    App, BorrowAppContext as _, Bounds, Div, InteractiveElement, ParentElement, Pixels, Point,
    SharedString, Styled, Subscription, Window, point, px, size,
};
use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use tl_model::validity::Validity;

use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::tr;
use crate::selector::TlSelector;
use crate::table::actions::{
    EnableAllColumns, HideColumn, OpenColumnPicker, ResetColumns, ResizeColumnsToFit, SortByColumn,
    TABLE_CONTEXT,
};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnError, ColumnLayout, ColumnSpec, TableColumn, fit_width,
    min_width,
};
use crate::table::columns_window::{self, ColumnHost, PickerColumn};
use crate::table::header::{
    self, HeaderCell, HeaderEvent, HeaderListener, HeaderPress, HeaderState, ReorderDrag,
    ResizeDrag, SORT_ARROW, TlTreeTableHeader, beyond_slop, header_menu, header_tooltip,
    insertion_slot, sort_arrow_x,
};
use crate::table::tree_core::{ColKey, DynTreeDelegate, TreeTableCore, open_boxed_menu};
use crate::table::tree_table::{ColumnResizeMode, HeaderLines, TableSort};
use crate::text::metrics::TextMetricsCache;
use crate::text_cache::CellTextCache;
use crate::theme::{TlTheme, UiMetrics, metrics};

/// What the header and column logic reads from the delegate. Every `TreeTableDelegate`
/// implements it for its column type.
pub(crate) trait ColumnSource<C: TableColumn> {
    /// `TreeTableDelegate::columns`.
    fn col_layout(&self) -> &ColumnLayout<C>;
    /// `TreeTableDelegate::column_spec`.
    fn col_spec(&self, col: C) -> &ColumnSpec<C>;
    /// The column and direction of `TreeTableDelegate::sort`.
    fn col_sort(&self) -> (C, bool);
    /// `TreeTableDelegate::columns_fixed`.
    fn cols_fixed(&self) -> bool;
    /// `TreeTableDelegate::header_lines`.
    fn col_lines(&self) -> HeaderLines;
    /// `TreeTableDelegate::header_total`.
    fn col_total(&self, col: C, out: &mut CellOut) -> bool;
}

/// The header state with the delegate and the app, borrowed at once.
pub(crate) struct Parts<'a, C: TableColumn> {
    pub(crate) head: &'a mut HeaderCore<C>,
    pub(crate) src: &'a dyn ColumnSource<C>,
    pub(crate) app: &'a mut App,
}

/// [`DynTreeDelegate`] plus the typed column calls. Implemented once per delegate type by the
/// shell's `Host`.
pub(crate) trait DynColumns<C: TableColumn>: DynTreeDelegate {
    fn parts(&mut self) -> Parts<'_, C>;
    fn head_ref(&self) -> &HeaderCore<C>;
    fn source(&self) -> &dyn ColumnSource<C>;
    /// `TreeTableDelegate::on_columns_changed`.
    fn set_columns(&mut self, layout: &ColumnLayout<C>);
    /// `TreeTableDelegate::on_sort`.
    fn sort_by(&mut self, col: C);
    /// The body's key of `col`.
    fn col_key(&mut self, col: C) -> ColKey;
    /// The column picker's view of this table.
    fn picker_host(&self) -> Rc<dyn ColumnHost>;

    fn head(&mut self) -> &mut HeaderCore<C> {
        self.parts().head
    }
}

/// The way back into the table for the header listener, the column actions and the reorder
/// Esc interceptor: runs `f` inside an update of the table entity, which it holds weakly.
pub(crate) type ColLink<C> = Rc<dyn Fn(&mut App, ColCall<'_, C>)>;

/// What a [`ColLink`] runs inside the table's update.
pub(crate) type ColCall<'a, C> = &'a mut dyn FnMut(&mut TreeTableCore, &mut dyn DynColumns<C>);

/// The header and column state of one table.
pub(crate) struct HeaderCore<C: TableColumn> {
    /// Header hover, press, totals and this frame's header cells.
    pub(crate) state: HeaderState<C>,
    /// Routes the header's pointer events to [`header_event`]. Created once in `new`.
    listener: HeaderListener,
    /// `tables.column_resize`, passed in by the owner.
    pub(crate) resize_mode: ColumnResizeMode,
    /// The owner's default layout for "Reset to default". `None`: `ResetColumns` goes on to the
    /// owner.
    pub(crate) default_layout: Option<ColumnLayout<C>>,
    /// Catches Esc before any key binding while a column reorder drag is in progress.
    reorder_esc: Option<Subscription>,
    link: ColLink<C>,
}

impl<C: TableColumn> HeaderCore<C> {
    pub(crate) fn new(link: ColLink<C>) -> Self {
        let listener: HeaderListener = {
            let link = link.clone();
            Rc::new(move |ev, window, cx| {
                link(cx, &mut |core, h| header_event(h, core, ev, window));
            })
        };
        Self {
            state: HeaderState::default(),
            listener,
            resize_mode: ColumnResizeMode::default(),
            default_layout: None,
            reorder_esc: None,
            link,
        }
    }
}

/// The layout the table draws: the live copy during a boundary drag, else the delegate's.
pub(crate) fn live_layout<'a, C: TableColumn>(
    head: &'a HeaderCore<C>,
    src: &'a dyn ColumnSource<C>,
) -> &'a ColumnLayout<C> {
    match &head.state.resize {
        Some(drag) => &drag.layout,
        None => src.col_layout(),
    }
}

/// [`live_layout`] of `h`.
fn live<C: TableColumn>(h: &dyn DynColumns<C>) -> &ColumnLayout<C> {
    live_layout(h.head_ref(), h.source())
}

/// A sort state as the header menu reads it.
struct SortOf<C>(C, bool);

impl<C: Copy> TableSort<C> for SortOf<C> {
    fn column(&self) -> C {
        self.0
    }
    fn ascending(&self) -> bool {
        self.1
    }
}

/// The header, the body and the table's action handlers for one frame.
pub(crate) fn render_table<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    core: &mut TreeTableCore,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
) -> Div {
    let hairline = UiMetrics::hairline(window);
    let lines = h.source().col_lines();
    prepare_header(h, core, lines, m, theme, window);
    let header = {
        let p = h.parts();
        let tooltip = p
            .head
            .state
            .hover
            .and_then(|c| header_tooltip(p.src.col_spec(c)));
        TlTreeTableHeader {
            layout: live_layout(p.head, p.src),
            lines,
            x_offset: core.x_offset,
            m,
            theme,
            hairline,
            state: &p.head.state,
            listener: p.head.listener.clone(),
        }
        .render(tooltip, window, p.app)
    };

    let insert_x = h.head_ref().state.insert_x;
    let body = core.render_body(insert_x, m, theme, window, &mut *h);

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
    let link = h.head_ref().link.clone();
    column_actions(&link, core.key_actions(root))
        .child(header)
        .child(body)
}

/// Wraps a column handler as an action listener that enters the table through `link`.
fn listen<C: TableColumn, E: 'static>(
    link: &ColLink<C>,
    f: fn(&mut dyn DynColumns<C>, &mut TreeTableCore, &E, &mut Window),
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let link = link.clone();
    move |a: &E, window: &mut Window, cx: &mut App| {
        link(cx, &mut |core, h| f(h, core, a, window));
    }
}

/// Adds the handlers of the header menu's column actions.
fn column_actions<C: TableColumn>(link: &ColLink<C>, el: Div) -> Div {
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
pub(crate) fn follow_ui_scale<C: TableColumn>(h: &mut dyn DynColumns<C>, ui_scale: f32) {
    if h.head_ref().state.resize.is_some() || h.source().col_layout().ui_scale() == ui_scale {
        return;
    }
    let mut layout = h.source().col_layout().clone();
    layout.rebuild(ui_scale);
    h.set_columns(&layout);
}

/// The header total of `col` as line 2 shows it: the cached text and its validity. `None` when
/// the delegate has no total for `col`. Formats only when the value or validity changed.
pub(crate) fn header_total_text<C: TableColumn>(
    totals: &mut CellTextCache<(), C>,
    out: &mut CellOut,
    src: &dyn ColumnSource<C>,
    col: C,
) -> Option<(SharedString, Validity)> {
    out.reset();
    if !src.col_total(col, out) {
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
            src.col_total(col, &mut full);
            *s = full.text;
        }),
    };
    Some((text, validity))
}

/// Fills `header_state.cells` and the sort arrow for this frame. The `Vec` keeps its capacity;
/// texts are `SharedString` clones and cached totals.
fn prepare_header<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    core: &mut TreeTableCore,
    lines: HeaderLines,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &Window,
) {
    let Parts { head, src, .. } = h.parts();
    let layout = live_layout(head, src);
    let visible = layout.visible_columns().clone();
    let offsets = layout.offsets().clone();
    let total_width = layout.total_width();
    let (sort_col, ascending) = src.col_sort();
    let state = &mut head.state;
    let mut cells = std::mem::take(&mut state.cells);
    cells.clear();
    state.arrow = None;
    for (pos, &id) in visible.iter().enumerate() {
        let x = offsets[pos];
        let w = offsets.get(pos + 1).copied().unwrap_or(total_width) - x;
        let spec = src.col_spec(id);
        let (label, align, sortable) = (spec.label.clone(), spec.align, spec.sortable);
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
            HeaderLines::Two => header_total_text(&mut state.totals, &mut core.out, src, id),
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
fn col_at<C: TableColumn>(h: &dyn DynColumns<C>, pos: Option<usize>) -> Option<C> {
    pos.and_then(|p| live(h).visible_columns().get(p).copied())
}

/// Pointer events from the header's hitboxes (UI-PRC-005). A left press and release on the
/// same sortable cell, without moving more than 4 px, sorts by that column. Moving more than
/// 4 px drags the column to a new place, except Name. A press on a resize band starts a
/// boundary drag; a double-click on one fits that column.
fn header_event<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    core: &mut TreeTableCore,
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
            if let Some(drag) = &mut h.head().state.resize {
                drag.drag_to(at.x);
                h.notify();
                return;
            }
            if h.head_ref().state.reorder.is_some() {
                reorder_to(h, core, at.x);
                h.notify();
                return;
            }
            let col = col_at(h, hovered);
            let band = col_at(h, boundary);
            let state = &mut h.head().state;
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
                start_reorder(h, core, p, at, window);
            }
            if changed {
                h.notify();
            }
        }
        HeaderEvent::Down { pos, at } => {
            if let Some(col) = col_at(h, Some(pos)) {
                h.head().state.press = Some(HeaderPress {
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
            let Some(col) = col_at(h, Some(pos)) else {
                return;
            };
            h.head().state.press = None;
            if click_count >= 2 {
                fit_columns(h, core, Some(col), window);
            } else {
                start_resize(h, core, col, at, window);
            }
        }
        HeaderEvent::Up { hovered, at } => {
            if let Some(drag) = h.head().state.resize.take() {
                if drag.changed() {
                    h.set_columns(&drag.layout);
                }
                h.notify();
                return;
            }
            if let Some(drag) = h.head().state.reorder.take() {
                let head = h.head();
                head.state.press = None;
                head.reorder_esc = None;
                drop_column(h, drag);
                h.notify();
                return;
            }
            let Some(p) = h.head().state.press.take() else {
                return;
            };
            h.notify();
            if p.dragged || beyond_slop(p.at, at, scale) || col_at(h, hovered) != Some(p.col) {
                return;
            }
            sort_by_column(h, p.col);
        }
        HeaderEvent::ContextMenu { at } => {
            let state = &h.head_ref().state;
            if state.resize.is_some() || state.reorder.is_some() {
                return;
            }
            let x = state.content_x(at.x, core.x_offset);
            if let Some(col) = live(h).column_at_x(x) {
                h.app().stop_propagation();
                open_header_menu(h, core, col, at, window);
            }
        }
    }
}

/// Opens the header menu (UI-PRC-006) for `col` with its top left corner at `at`. Its actions
/// run in the table's key context.
fn open_header_menu<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    core: &TreeTableCore,
    col: C,
    at: Point<Pixels>,
    window: &mut Window,
) {
    let src = h.source();
    let spec = src.col_spec(col).clone();
    let layout = src.col_layout().clone();
    let (sort_col, ascending) = src.col_sort();
    let sort = SortOf(sort_col, ascending);
    let fixed = src.cols_fixed();
    let mode = h.head_ref().resize_mode;
    let build = Box::new(move |menu, window: &mut Window, cx: &mut App| {
        header_menu(&spec, &layout, &sort, fixed, mode, menu, window, cx)
    });
    open_boxed_menu(at, core.focus.clone(), build, window, h.app());
}

/// `HideColumn` from the header menu: hides the column and calls `on_columns_changed` once.
/// Hiding the last visible column shows the Warning toast instead. A key outside this table's
/// column set goes on to the next handler.
fn on_hide_column<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    _: &mut TreeTableCore,
    a: &HideColumn,
    window: &mut Window,
) {
    let layout = h.source().col_layout();
    let Some(col) = C::parse(layout.table, &a.id) else {
        h.app().propagate();
        return;
    };
    let mut layout = layout.clone();
    match layout.set_visible(col, false) {
        Ok(()) => {
            layout.rebuild(layout.ui_scale());
            h.set_columns(&layout);
            h.notify();
        }
        Err(ColumnError::LastVisible) => {
            let cx = h.app();
            let text = tr(cx, "table-toast-last-visible");
            push_toast(ToastKind::Warning, text, window, cx);
        }
        Err(err) => {
            tracing::debug!(target: "tasklens::ui", ?err, ?col, "hide column refused");
        }
    }
}

/// `ResetColumns` (UI-PRC-006): with the owner's defaults, replaces the layout with them, calls
/// `on_columns_changed` once and shows "Columns reset to default". Without defaults, or for
/// another table, the action goes on to the owner.
fn on_reset_columns<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    _: &mut TreeTableCore,
    a: &ResetColumns,
    window: &mut Window,
) {
    let has_defaults = h
        .head_ref()
        .default_layout
        .as_ref()
        .is_some_and(|d| d.table == a.table);
    if !has_defaults {
        h.app().propagate();
        return;
    }
    reset_columns(h, window);
}

/// Replaces the layout with the owner's defaults, calls `on_columns_changed` once and shows
/// "Columns reset to default" in `window`. False and no change without defaults.
pub(crate) fn reset_columns<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    window: &mut Window,
) -> bool {
    let scale = h.source().col_layout().ui_scale();
    let Some(defaults) = &h.head_ref().default_layout else {
        return false;
    };
    let mut layout = defaults.clone();
    layout.rebuild(scale);
    h.set_columns(&layout);
    h.notify();
    let cx = h.app();
    let text = tr(cx, "table-toast-columns-reset");
    push_toast(ToastKind::Info, text, window, cx);
    true
}

/// `EnableAllColumns` (R-180): shows every column that exists on this system, each in its
/// remembered slot and width, and calls `on_columns_changed` once. Another table's id goes on to
/// the next handler.
fn on_enable_all_columns<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    _: &mut TreeTableCore,
    a: &EnableAllColumns,
    _: &mut Window,
) {
    let src = h.source();
    let layout = src.col_layout();
    if a.table != layout.table {
        h.app().propagate();
        return;
    }
    let mut layout = layout.clone();
    let ids = layout.order.clone();
    for id in ids {
        if src.col_spec(id).availability != Availability::NotOnThisSystem {
            let _ = layout.set_visible(id, true);
        }
    }
    layout.rebuild(layout.ui_scale());
    h.set_columns(&layout);
    h.notify();
}

/// `OpenColumnPicker` for this table: opens the picker bound to it, or focuses the open one and
/// binds it here. A table with fixed columns or another id passes the action on.
fn on_open_column_picker<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    _: &mut TreeTableCore,
    a: &OpenColumnPicker,
    window: &mut Window,
) {
    let src = h.source();
    if a.table != src.col_layout().table || src.cols_fixed() {
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
pub(crate) fn set_column_visible<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    key: &str,
    on: bool,
) -> Result<(), ColumnError> {
    let layout = h.source().col_layout();
    let col = C::parse(layout.table, key).ok_or(ColumnError::Unknown)?;
    let mut layout = layout.clone();
    if layout.visible.contains(&col) == on {
        return Ok(());
    }
    layout.set_visible(col, on)?;
    layout.rebuild(layout.ui_scale());
    h.set_columns(&layout);
    h.notify();
    Ok(())
}

/// Every column of the delegate's layout in its order, as the column picker lists it (R-181).
pub(crate) fn picker_columns<C: TableColumn>(src: &dyn ColumnSource<C>) -> Vec<PickerColumn> {
    let layout = src.col_layout();
    layout
        .order
        .iter()
        .map(|&id| {
            let spec = src.col_spec(id);
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

/// Turns `press`, which just moved past the slop to window point `at`, into a reorder drag.
/// Name (`hideable == false`) never drags. Esc cancels the drag through a keystroke
/// interceptor, which runs before every key binding, so the global Esc chain never sees it.
fn start_reorder<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    core: &TreeTableCore,
    press: HeaderPress<C>,
    at: Point<Pixels>,
    window: &mut Window,
) {
    if !h.source().col_spec(press.col).hideable {
        return;
    }
    let layout = live(h);
    let Some(pos) = layout
        .visible_columns()
        .iter()
        .position(|c| *c == press.col)
    else {
        return;
    };
    let left = layout.offsets()[pos];
    let state = &mut h.head().state;
    let grab = state.content_x(press.at.x, core.x_offset) - left;
    state.reorder = Some(ReorderDrag {
        col: press.col,
        grab,
        x: Pixels::ZERO,
        slot: pos,
    });
    reorder_to(h, core, at.x);
    h.head().state.set_hover(None);
    let link = h.head_ref().link.clone();
    let handle = window.window_handle();
    let esc = h.app().intercept_keystrokes(move |ev, window, cx| {
        if ev.keystroke.key == "escape" && window.window_handle() == handle {
            link(cx, &mut |_, h| cancel_reorder(h));
            cx.stop_propagation();
        }
    });
    h.head().reorder_esc = Some(esc);
}

/// Moves the reorder ghost to window x `x` and picks the nearest allowed boundary.
fn reorder_to<C: TableColumn>(h: &mut dyn DynColumns<C>, core: &TreeTableCore, x: Pixels) {
    let x = h.head_ref().state.content_x(x, core.x_offset);
    let layout = live(h);
    let slot = insertion_slot(
        layout.visible_columns(),
        layout.offsets(),
        layout.total_width(),
        x,
    );
    if let Some(drag) = &mut h.head().state.reorder {
        drag.x = x;
        drag.slot = slot;
    }
}

/// Esc during a reorder drag: ends it with no change. The release that follows does nothing.
fn cancel_reorder<C: TableColumn>(h: &mut dyn DynColumns<C>) {
    let head = h.head();
    head.state.reorder = None;
    head.state.press = None;
    head.reorder_esc = None;
    h.notify();
}

/// Applies a finished reorder drag: moves the column before the column at the drop slot, or to
/// the end, and calls `on_columns_changed` once. A drop on either edge of the column itself
/// changes nothing.
fn drop_column<C: TableColumn>(h: &mut dyn DynColumns<C>, drag: ReorderDrag<C>) {
    let layout = h.source().col_layout();
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
    h.set_columns(&layout);
}

/// Starts a boundary drag of `col` at window point `at`, on a copy of the delegate's layout.
fn start_resize<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    core: &TreeTableCore,
    col: C,
    at: Point<Pixels>,
    window: &Window,
) {
    let mins = column_mins(h, window);
    let layout = h.source().col_layout().clone();
    let scale = layout.ui_scale();
    let body = ScrollbarHandle::viewport_bounds(&core.scroll).size.width;
    // Before the first layout of the body the table's own width stands in.
    let viewport = if body > Pixels::ZERO {
        body
    } else {
        layout.total_width()
    } / scale;
    let start_widths = layout.widths.clone();
    let head = h.head();
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
fn column_mins<C: TableColumn>(h: &mut dyn DynColumns<C>, window: &Window) -> FxHashMap<C, Pixels> {
    let Parts { src, app: cx, .. } = h.parts();
    let m = *metrics(cx);
    let (sort_col, _) = src.col_sort();
    let layout = src.col_layout();
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
                let spec = src.col_spec(col);
                let sorted = spec.sortable && col == sort_col;
                let min = min_width(spec, sorted, cache, &m, window, cx);
                (col, min / scale)
            })
            .collect()
    })
}

/// Fits `only`, or every visible column on `None`, to its content (UI-PRC-006): the largest of
/// the minimum, the widest text + 16 and the header label + 16 + 12. Texts come from the rows
/// the last frame built, the cell texts still cached from the last 3 table versions and the
/// delegate's `fit_samples`, never from a pass over all rows. Then calls `on_columns_changed`
/// once.
fn fit_columns<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    core: &mut TreeTableCore,
    only: Option<C>,
    window: &Window,
) {
    let m = *metrics(h.app());
    let mins = column_mins(h, window);
    let mut layout = h.source().col_layout().clone();
    let scale = layout.ui_scale();
    let targets: SmallVec<[C; 24]> = match only {
        Some(col) => smallvec![col],
        None => layout.visible_columns().iter().copied().collect(),
    };
    let keys: SmallVec<[ColKey; 24]> = targets.iter().map(|&c| h.col_key(c)).collect();
    let widest = core.widest_texts(&keys, &m, window, &mut *h);
    let Parts { head, src, .. } = h.parts();
    for (i, &col) in targets.iter().enumerate() {
        let label = src.col_spec(col).label.clone();
        let label_w = head
            .state
            .label_width(col, &label, m.style_version, &m, window);
        let min = mins.get(&col).copied().unwrap_or(Pixels::ZERO) * scale;
        let w = fit_width(min, widest[i], label_w, &m);
        layout.widths.insert(col, w / scale);
    }
    layout.rebuild(scale);
    h.set_columns(&layout);
    h.notify();
}

/// `ResizeColumnsToFit` from the header menu: fits every visible column. An action for another
/// table goes on to the next handler.
fn on_resize_columns_to_fit<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    core: &mut TreeTableCore,
    a: &ResizeColumnsToFit,
    window: &mut Window,
) {
    if a.table != h.source().col_layout().table {
        h.app().propagate();
        return;
    }
    fit_columns(h, core, None, window);
}

/// Asks the delegate to sort by `col` (header click and `SortByColumn`). The delegate picks the
/// direction with `next_sort`. Nothing on an unsortable column.
pub(crate) fn sort_by_column<C: TableColumn>(h: &mut dyn DynColumns<C>, col: C) {
    if !h.source().col_spec(col).sortable {
        return;
    }
    h.sort_by(col);
    h.notify();
}

/// `SortByColumn` from the header menu. A key outside this table's column set is ignored.
fn on_sort_by_column<C: TableColumn>(
    h: &mut dyn DynColumns<C>,
    _: &mut TreeTableCore,
    a: &SortByColumn,
    _: &mut Window,
) {
    let table = live(h).table;
    if let Some(col) = C::parse(table, &a.id) {
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
