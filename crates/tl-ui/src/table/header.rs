//! `TlTreeTableHeader`: the column header above a `TlTreeTable` body (UI-PRC-001, UI-PRC-005).
//!
//! One cell per visible column at the same x offsets as the body. Line 1 is the column label with
//! the sort arrow on the sort column. In `HeaderLines::Two` the header is 44 px tall and line 2
//! shows the delegate's system total, right-aligned in `text_secondary`. One `canvas` paints the
//! hover and pressed fills, the column boundaries, the bottom border and the sort arrow, and
//! holds one hitbox per cell; the texts are one `div` per line per visible column.
//!
//! The header owns no scroll state: the table passes its shared `x_offset` and everything is
//! translated by `-x_offset`, so header and body always move together. The table owns
//! [`HeaderState`] and fills its per-frame cells before each render, so the header itself formats
//! nothing: totals come from a `CellTextCache` and re-format only when their value changes.
//!
//! Resize (E18-010): an 8 px hit band centred on each column's right edge sets the column-resize
//! cursor. Dragging it changes the width live in a copy of the layout that [`HeaderState`] holds
//! until the release; double-clicking it fits the column.
//!
//! Reorder (E18-011): a press on a cell that moves more than 4 px drags the column. A ghost of the
//! cell follows the pointer and a 2 px `accent` line marks the nearest column boundary, never the
//! one before a pinned first column (Name). The table applies the move on release.
//!
//! Header menu (E18-012, UI-PRC-006): a right-click on a header cell opens [`header_menu`] at the
//! pointer for the column under it. The header is not focusable, so Shift+F10 never reaches it.

use std::cell::Cell;
use std::hash::BuildHasher as _;
use std::rc::Rc;

use gpui_kit::assets::IconNamed as _;
use gpui_kit::{
    Action, App, Bounds, CursorStyle, Div, ElementId, FontWeight, Hitbox, HitboxBehavior, Hsla,
    InteractiveElement, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, SharedString, Styled, TextRun, TransformationMatrix, Window,
    canvas, div, fill, font, point, prelude::FluentBuilder, px, size,
};
use rustc_hash::{FxBuildHasher, FxHashMap};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;
use tl_model::validity::Validity;

use crate::components::icons::TlIcon;
use crate::components::menu::TlMenu;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::{tr, tr_args};
use crate::table::actions::{
    HideColumn, OpenColumnPicker, ResetColumns, ResizeColumnsToFit, SetColumnResize, SortByColumn,
};
use crate::table::cells::{CellCtx, glyph_cell, numeric_cell};
use crate::table::columns::{ColumnAlign, ColumnLayout, ColumnSpec, TableColumn};
use crate::table::tree_table::{ColumnResizeMode, TableSort};
use crate::table::tree_table::{HEADER_H_ONE, HEADER_H_TWO, HeaderLines};
use crate::text::fonts::UI_FAMILY;
use crate::text::style::label_style;
use crate::text_cache::CellTextCache;
use crate::theme::{TlTheme, UiMetrics};

/// Top of line 1 in the two-line header.
pub const LINE1_TOP: f32 = 4.0;
/// Top of line 2 in the two-line header.
pub const LINE2_TOP: f32 = 22.0;
/// Height of each line in the two-line header.
pub const LINE_H: f32 = 18.0;
/// Size of the sort arrow (Lucide `arrow-up` or `arrow-down`).
pub const SORT_ARROW: f32 = 12.0;
/// Gap between the label and the sort arrow (`sp1`).
pub const SORT_ARROW_GAP: f32 = 2.0;
/// How far the label moves left on the sort column: the arrow and its gap.
pub const SORT_ARROW_SHIFT: f32 = SORT_ARROW + SORT_ARROW_GAP;
/// A press that moves more than this is a drag (reorder), not a click (sort).
pub const DRAG_SLOP: f32 = 4.0;
/// Half the width of the resize hit band on each column boundary (UI-PRC-005).
pub const RESIZE_HALF_BAND: f32 = 4.0;
/// Opacity of the reorder ghost's `header` fill (UI-PRC-005).
pub const GHOST_OPACITY: f32 = 0.85;
/// Width of the reorder insertion line.
pub const INSERT_LINE_W: f32 = 2.0;
/// Header tooltip of every unsortable column (UI-PRC-005).
pub const UNSORTABLE_TOOLTIP: &str = "Only measured for visible rows";
/// Minimum width of the header menu at 100% (R-112).
pub const HEADER_MENU_MIN_W: f32 = 240.0;

/// What the header's hitboxes report to the table. `pos` is a visible column position.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum HeaderEvent {
    /// The pointer moved; `hovered` is the header cell under it and `boundary` the resize band
    /// under it, if any. A band wins over the cell: `hovered` is `None` over a band.
    Move {
        hovered: Option<usize>,
        boundary: Option<usize>,
        at: Point<Pixels>,
    },
    /// Left press on a header cell, outside the resize bands.
    Down { pos: usize, at: Point<Pixels> },
    /// Left press on the resize band at the right edge of the cell at `pos`.
    BoundaryDown {
        pos: usize,
        at: Point<Pixels>,
        click_count: usize,
    },
    /// Left release anywhere in the window; `hovered` as for `Move`.
    Up {
        hovered: Option<usize>,
        at: Point<Pixels>,
    },
    /// Right press on a header cell or a resize band, at window point `at`.
    ContextMenu { at: Point<Pixels> },
}

/// The table's handler for [`HeaderEvent`]s. Created once with the table, cloned per frame.
pub type HeaderListener = Rc<dyn Fn(HeaderEvent, &mut Window, &mut App)>;

/// A left press on a header cell, kept until the release.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HeaderPress<C> {
    pub col: C,
    pub at: Point<Pixels>,
    /// True once the pointer moved more than [`DRAG_SLOP`] from `at`.
    pub dragged: bool,
}

/// A boundary drag in progress (UI-PRC-005). Widths and minimums are logical px at 100%.
#[derive(Clone, Debug)]
pub struct ResizeDrag<C: TableColumn> {
    pub col: C,
    /// Window x of the press.
    pub start_x: Pixels,
    /// Width of `col` at the press, at 100%.
    pub start_width: Pixels,
    /// Every width at the press. Each move starts again from these.
    pub start_widths: FxHashMap<C, Pixels>,
    pub mins: FxHashMap<C, Pixels>,
    /// Body width at the press, at 100%.
    pub viewport: Pixels,
    pub mode: ColumnResizeMode,
    /// The layout the table draws until the release.
    pub layout: ColumnLayout<C>,
}

impl<C: TableColumn> ResizeDrag<C> {
    /// Applies a pointer at window x `x`: restores the press widths, resizes once and rebuilds
    /// the offsets once.
    pub fn drag_to(&mut self, x: Pixels) {
        let scale = self.layout.ui_scale();
        let w = self.start_width + (x - self.start_x) / scale;
        self.layout.widths.clone_from(&self.start_widths);
        crate::table::columns::resize(
            &mut self.layout,
            self.col,
            w,
            self.mode,
            self.viewport,
            &self.mins,
        );
    }

    /// True when a width differs from the press.
    pub fn changed(&self) -> bool {
        self.layout.widths != self.start_widths
    }
}

/// A header cell drag in progress (UI-PRC-005). x values are header content coordinates (before
/// `x_offset`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ReorderDrag<C> {
    pub col: C,
    /// Pointer x minus the cell's left edge at the press. Keeps the ghost under the pointer
    /// where it was grabbed.
    pub grab: Pixels,
    /// Pointer x now.
    pub x: Pixels,
    /// The boundary the column lands on: slot k is the left edge of visible column k, slot
    /// `visible.len()` the right edge of the last column.
    pub slot: usize,
}

/// The insertion slot nearest to content x `x` (see [`ReorderDrag::slot`]). Never slot 0 when
/// the first visible column is pinned first: nothing goes before Name.
pub fn insertion_slot<C: TableColumn>(
    visible: &[C],
    offsets: &[Pixels],
    total: Pixels,
    x: Pixels,
) -> usize {
    let first = usize::from(visible.first().is_some_and(|c| c.pinned_first()));
    let dist = |k: usize| {
        (offsets.get(k).copied().unwrap_or(total) - x)
            .as_f32()
            .abs()
    };
    (first..=visible.len())
        .min_by(|&a, &b| dist(a).total_cmp(&dist(b)))
        .unwrap_or(first)
}

/// The fill of the reorder ghost: `header` at 85%.
pub fn ghost_fill(theme: &TlTheme) -> Hsla {
    theme.header.opacity(GHOST_OPACITY)
}

/// One visible header cell as the table prepared it for this frame.
#[derive(Clone, Debug)]
pub struct HeaderCell<C> {
    pub id: C,
    pub label: SharedString,
    pub align: ColumnAlign,
    /// Content x of the cell's left edge (before `x_offset`), and its width.
    pub x: Pixels,
    pub w: Pixels,
    /// `Some(ascending)` on the sort column.
    pub sort: Option<bool>,
    /// The cached total text and its validity. `None`: line 2 stays empty.
    pub total: Option<(SharedString, Validity)>,
    /// `row_hover`, or `row_selected` at 50% while pressed. Never on unsortable columns.
    pub fill: Option<Hsla>,
}

/// Header state kept by the table across frames.
pub struct HeaderState<C: TableColumn> {
    /// The header cell under the pointer.
    pub hover: Option<C>,
    pub press: Option<HeaderPress<C>>,
    /// The column whose resize band is under the pointer.
    pub boundary_hover: Option<C>,
    /// The boundary drag in progress.
    pub resize: Option<ResizeDrag<C>>,
    /// The cell drag in progress.
    pub reorder: Option<ReorderDrag<C>>,
    /// Content x of the reorder insertion line this frame: the boundary of `reorder.slot`.
    pub insert_x: Option<Pixels>,
    /// Window x of the header's left edge in the last frame. Set by the paint.
    origin: Rc<Cell<Pixels>>,
    /// Formatted totals, keyed by the header row `()` and the column.
    pub totals: CellTextCache<(), C>,
    /// This frame's visible cells. The `Vec` is reused across frames.
    pub cells: Vec<HeaderCell<C>>,
    /// The sort arrow of this frame: column, ascending, bounds in header content coordinates
    /// (before `x_offset`).
    pub arrow: Option<(C, bool, Bounds<Pixels>)>,
    /// Line 1 label widths, measured once per style version for the arrow on left-aligned
    /// columns.
    label_widths: FxHashMap<C, Pixels>,
    label_style: u64,
    /// Truncation flag of the totals. The header shows no truncation tooltip, so it is never
    /// read.
    clip_sink: Rc<Cell<bool>>,
}

impl<C: TableColumn> Default for HeaderState<C> {
    fn default() -> Self {
        Self {
            hover: None,
            press: None,
            boundary_hover: None,
            resize: None,
            reorder: None,
            insert_x: None,
            origin: Rc::new(Cell::new(Pixels::ZERO)),
            totals: CellTextCache::new(),
            cells: Vec::new(),
            arrow: None,
            label_widths: FxHashMap::default(),
            label_style: 0,
            clip_sink: Rc::new(Cell::new(false)),
        }
    }
}

impl<C: TableColumn> HeaderState<C> {
    /// Sets the hovered cell. True when it changed.
    pub fn set_hover(&mut self, col: Option<C>) -> bool {
        let changed = self.hover != col;
        self.hover = col;
        changed
    }

    /// Header content x of window x `x`, from the header bounds of the last frame.
    pub fn content_x(&self, x: Pixels, x_offset: Pixels) -> Pixels {
        x - self.origin.get() + x_offset
    }

    /// The pointer cursor over the header: column-resize over a resize band and during a
    /// boundary drag, else the arrow (UI-FND-003). The paint sets the same cursor through the
    /// band hitboxes.
    pub fn cursor(&self) -> CursorStyle {
        if self.boundary_hover.is_some() || self.resize.is_some() {
            CursorStyle::ResizeColumn
        } else {
            CursorStyle::Arrow
        }
    }

    /// The fill of header cell `col`: pressed wins over hover; unsortable columns get none.
    pub fn fill(&self, col: C, sortable: bool, theme: &TlTheme) -> Option<Hsla> {
        if !sortable {
            return None;
        }
        if let Some(p) = &self.press
            && p.col == col
            && !p.dragged
        {
            return Some(theme.row_selected.opacity(0.5));
        }
        (self.hover == Some(col)).then_some(theme.row_hover)
    }

    /// The width of `label` in the line 1 font. Shapes it only on the first call per column and
    /// style version.
    pub fn label_width(
        &mut self,
        col: C,
        label: &SharedString,
        style_version: u64,
        m: &UiMetrics,
        window: &Window,
    ) -> Pixels {
        if style_version != self.label_style {
            self.label_widths.clear();
            self.label_style = style_version;
        }
        *self.label_widths.entry(col).or_insert_with(|| {
            let run = TextRun {
                len: label.len(),
                font: gpui_kit::Font {
                    weight: FontWeight::MEDIUM,
                    ..font(UI_FAMILY)
                },
                color: gpui_kit::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(label.clone(), m.text.base, &[run], None)
                .width
        })
    }
}

/// The header tooltip of a column (UI-PRC-005): its explicit tooltip, else "Only measured for
/// visible rows" on an unsortable column, else the picker's full name when it differs from the
/// label, else none.
pub fn header_tooltip<C: TableColumn>(spec: &ColumnSpec<C>) -> Option<SharedString> {
    if let Some(tip) = &spec.tooltip {
        return Some(tip.clone());
    }
    if !spec.sortable {
        return Some(SharedString::new_static(UNSORTABLE_TOOLTIP));
    }
    (spec.full_name != spec.label).then(|| spec.full_name.clone())
}

/// Left edge of the sort arrow relative to the cell. Right-aligned: at the right padding edge.
/// Left-aligned: `sp1` after the label, which is cut to leave the arrow room. `label_w` is the
/// measured label width (only used on left-aligned columns).
pub fn sort_arrow_x(align: ColumnAlign, w: Pixels, label_w: Pixels, m: &UiMetrics) -> Pixels {
    let s = m.ui_scale;
    let pad = m.sp[4];
    let arrow = px(SORT_ARROW * s);
    let end = (w - pad - arrow).max(Pixels::ZERO);
    match align {
        ColumnAlign::Right => end,
        ColumnAlign::Left => {
            let room = (w - pad * 2.0 - px(SORT_ARROW_SHIFT * s)).max(Pixels::ZERO);
            (pad + label_w.min(room) + px(SORT_ARROW_GAP * s)).min(end)
        }
    }
}

/// Top and height of line 1: the top 28 px centred in `One`, y 4..22 in `Two`.
pub fn line1(lines: HeaderLines, m: &UiMetrics) -> (Pixels, Pixels) {
    let s = m.ui_scale;
    match lines {
        HeaderLines::One => (Pixels::ZERO, px(HEADER_H_ONE * s)),
        HeaderLines::Two => (px(LINE1_TOP * s), px(LINE_H * s)),
    }
}

/// Builds the header for one frame from the cells the table prepared in [`HeaderState`].
pub struct TlTreeTableHeader<'a, C: TableColumn> {
    pub layout: &'a ColumnLayout<C>,
    pub lines: HeaderLines,
    /// Shared with the body: how far the content is scrolled to the left.
    pub x_offset: Pixels,
    pub m: &'a UiMetrics,
    pub theme: &'a TlTheme,
    /// One physical pixel (`UiMetrics::hairline`).
    pub hairline: Pixels,
    pub state: &'a HeaderState<C>,
    pub listener: HeaderListener,
}

impl<C: TableColumn> TlTreeTableHeader<'_, C> {
    /// Header height: 28 for `One`, 44 for `Two`, × ui_scale.
    pub fn height(lines: HeaderLines, m: &UiMetrics) -> Pixels {
        let h = match lines {
            HeaderLines::One => HEADER_H_ONE,
            HeaderLines::Two => HEADER_H_TWO,
        };
        px(h * m.ui_scale)
    }

    /// The header element. `tooltip` is the tooltip of the hovered cell, if any.
    pub fn render(&self, tooltip: Option<SharedString>, window: &mut Window, cx: &mut App) -> Div {
        let m = self.m;
        let theme = self.theme;
        let s = m.ui_scale;
        let height = Self::height(self.lines, m);
        let (line1_top, line1_h) = line1(self.lines, m);
        let total = self.layout.total_width();
        let cells = &self.state.cells;

        let mut content = div()
            .absolute()
            .top_0()
            .left(-self.x_offset)
            .w(total)
            .h_full();
        for cell in cells {
            let id = cell.id;
            let label_w = if cell.sort.is_some() {
                (cell.w - px(SORT_ARROW_SHIFT * s)).max(Pixels::ZERO)
            } else {
                cell.w
            };
            let mut label = div();
            *label.text_style() = label_style(m, m.text.base, FontWeight::MEDIUM);
            let label = label
                .debug_selector(|| format!("TlTreeTableHeader-label-{}", id.key()))
                .absolute()
                .top(line1_top)
                .left(cell.x)
                .w(label_w)
                .h(line1_h)
                .px(m.sp[4])
                .flex()
                .items_center()
                .when(cell.align == ColumnAlign::Right, |d| d.justify_end())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(theme.text_primary)
                .child(cell.label.clone());
            content = content.child(label);

            if self.lines == HeaderLines::Two
                && let Some((text, validity)) = &cell.total
            {
                content = content.child(self.total_line(cell, text, *validity));
            }
        }

        let hovered = self
            .state
            .hover
            .and_then(|h| cells.iter().find(|c| c.id == h));
        if let (Some(cell), Some(tip)) = (hovered, tooltip) {
            let key = ElementId::NamedInteger(
                SharedString::new_static("tl-header-tip"),
                FxBuildHasher.hash_one(cell.id),
            );
            let trigger = div()
                .debug_selector(|| "TlTreeTableHeader-tip".into())
                .absolute()
                .top_0()
                .left(cell.x)
                .w(cell.w)
                .h_full();
            let state = HoverTooltip::state(key, window, cx);
            let trigger = HoverTooltip::attach(
                &state,
                trigger,
                DELAY_CONTROL,
                TlTooltip::text(tip).build(),
                window,
                cx,
            );
            content = content.child(trigger);
        }

        if let Some(drag) = &self.state.reorder
            && let Some(cell) = cells.iter().find(|c| c.id == drag.col)
        {
            content = content.child(self.ghost(cell, drag.x - drag.grab, line1_top, line1_h));
        }
        if let Some(x) = self.state.insert_x {
            content = content.child(
                insert_line(x, theme, m).debug_selector(|| "TlTreeTableHeader-insert".into()),
            );
        }

        div()
            .debug_selector(|| "TlTreeTableHeader".into())
            .relative()
            .flex_none()
            .w_full()
            .h(height)
            .overflow_hidden()
            .bg(theme.header)
            .child(self.canvas())
            .child(content)
    }

    /// The reorder ghost of `cell` with its left edge at content x `x`: `header` at 85%, the
    /// label, a 1 px `divider` border and `r_sm` corners.
    fn ghost(&self, cell: &HeaderCell<C>, x: Pixels, line1_top: Pixels, line1_h: Pixels) -> Div {
        let m = self.m;
        let mut label = div();
        *label.text_style() = label_style(m, m.text.base, FontWeight::MEDIUM);
        let label = label
            .absolute()
            .top(line1_top)
            .left_0()
            .w_full()
            .h(line1_h)
            .px(m.sp[4])
            .flex()
            .items_center()
            .when(cell.align == ColumnAlign::Right, |d| d.justify_end())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(self.theme.text_primary)
            .child(cell.label.clone());
        div()
            .debug_selector(|| "TlTreeTableHeader-ghost".into())
            .absolute()
            .top_0()
            .left(x)
            .w(cell.w)
            .h_full()
            .bg(ghost_fill(self.theme))
            .border(px(m.ui_scale))
            .border_color(self.theme.divider)
            .rounded(m.radius_small)
            .child(label)
    }

    /// Line 2 of `cell`: the total, right-aligned, numbers clipped and never cut.
    fn total_line(&self, cell: &HeaderCell<C>, text: &SharedString, validity: Validity) -> Div {
        let m = self.m;
        let ctx = CellCtx {
            m,
            theme: self.theme,
            selected: false,
            width: cell.w,
        };
        let line = if matches!(validity, Validity::Valid | Validity::Stale) {
            numeric_cell(text.clone(), validity, self.state.clip_sink.clone(), &ctx)
                .text_color(self.theme.text_secondary)
        } else {
            glyph_cell(validity, text, &ctx)
        };
        let id = cell.id;
        line.debug_selector(|| format!("TlTreeTableHeader-total-{}", id.key()))
            .absolute()
            .top(px(LINE2_TOP * m.ui_scale))
            .left(cell.x)
            .h(px(LINE_H * m.ui_scale))
    }

    /// The canvas behind the texts: fills, boundaries, bottom border, sort arrow, and one hitbox
    /// per cell that reports pointer events to the table.
    fn canvas(&self) -> impl gpui_kit::IntoElement {
        let x_offset = self.x_offset;
        let hairline = self.hairline;
        let divider = self.theme.divider;
        let accent = self.theme.accent;
        let arrow_color = self.theme.text_primary;
        let half_band = px(RESIZE_HALF_BAND * self.m.ui_scale);
        let resizing = self.state.resize.as_ref().map(|r| r.col);
        let resizing = resizing.and_then(|col| self.state.cells.iter().position(|c| c.id == col));
        // Copied out so the closures own them; inline for up to 24 columns.
        let cells: SmallVec<[(Pixels, Pixels, Option<Hsla>); 24]> = self
            .state
            .cells
            .iter()
            .map(|c| (c.x, c.w, c.fill))
            .collect();
        let arrow = self.state.arrow.map(|(_, asc, b)| (asc, b));
        let listener = self.listener.clone();
        let origin = self.state.origin.clone();

        canvas(
            {
                let cells = cells.clone();
                move |bounds: Bounds<Pixels>, window: &mut Window, _: &mut App| {
                    origin.set(bounds.left());
                    let left = bounds.left() - x_offset;
                    let cell_boxes: SmallVec<[Hitbox; 24]> = cells
                        .iter()
                        .map(|&(x, w, _)| {
                            let b = Bounds::new(
                                point(left + x, bounds.top()),
                                size(w, bounds.size.height),
                            );
                            window.insert_hitbox(b, HitboxBehavior::Normal)
                        })
                        .collect();
                    // Inserted after the cells, so a band is on top of the cells it overlaps.
                    let bands: SmallVec<[Hitbox; 24]> = cells
                        .iter()
                        .map(|&(x, w, _)| {
                            let b = Bounds::new(
                                point(left + x + w - half_band, bounds.top()),
                                size(half_band * 2.0, bounds.size.height),
                            );
                            window.insert_hitbox(b, HitboxBehavior::Normal)
                        })
                        .collect();
                    Rc::new(HeaderHitboxes {
                        cells: cell_boxes,
                        bands,
                    })
                }
            },
            move |bounds, hitboxes, window, cx| {
                let left = bounds.left() - x_offset;
                for (i, &(x, w, fill_color)) in cells.iter().enumerate() {
                    if let Some(color) = fill_color {
                        let b =
                            Bounds::new(point(left + x, bounds.top()), size(w, bounds.size.height));
                        window.paint_quad(fill(b, color));
                    }
                    // The boundary line sits on the column's right edge, inside the column.
                    let line = Bounds::new(
                        point(left + x + w - hairline, bounds.top()),
                        size(hairline, bounds.size.height),
                    );
                    let color = if resizing == Some(i) { accent } else { divider };
                    window.paint_quad(fill(line, color));
                }
                for band in &hitboxes.bands {
                    window.set_cursor_style(CursorStyle::ResizeColumn, band);
                }
                if resizing.is_some() {
                    // The pointer leaves the band during a drag; the cursor stays.
                    window.set_window_cursor_style(CursorStyle::ResizeColumn);
                }
                let bottom = Bounds::new(
                    point(bounds.left(), bounds.bottom() - hairline),
                    size(bounds.size.width, hairline),
                );
                window.paint_quad(fill(bottom, divider));
                if let Some((ascending, b)) = arrow {
                    let icon = if ascending {
                        TlIcon::ArrowUp
                    } else {
                        TlIcon::ArrowDown
                    };
                    let b = Bounds::new(point(left + b.left(), bounds.top() + b.top()), b.size);
                    let _ = window.paint_svg(
                        b,
                        icon.path(),
                        None,
                        TransformationMatrix::unit(),
                        arrow_color,
                        cx,
                    );
                }
                register_listeners(hitboxes, listener, window);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// The 2 px `accent` reorder insertion line centred on content x `x`, full height of its parent.
/// The table draws it over the header and over the body.
pub fn insert_line(x: Pixels, theme: &TlTheme, m: &UiMetrics) -> Div {
    let w = px(INSERT_LINE_W * m.ui_scale);
    div()
        .absolute()
        .top_0()
        .left(x - w / 2.0)
        .w(w)
        .h_full()
        .bg(theme.accent)
}

/// The header's hitboxes of one frame: one per cell and one resize band per cell's right edge.
struct HeaderHitboxes {
    cells: SmallVec<[Hitbox; 24]>,
    bands: SmallVec<[Hitbox; 24]>,
}

impl HeaderHitboxes {
    /// The band under the pointer, else `None`.
    fn band(&self, window: &Window) -> Option<usize> {
        self.bands.iter().position(|h| h.is_hovered(window))
    }

    /// The cell under the pointer, unless a band is.
    fn cell(&self, window: &Window) -> Option<usize> {
        if self.band(window).is_some() {
            return None;
        }
        self.cells.iter().position(|h| h.is_hovered(window))
    }
}

/// Reports pointer moves, left presses on a cell or a band and left releases to `listener`.
fn register_listeners(hitboxes: Rc<HeaderHitboxes>, listener: HeaderListener, window: &mut Window) {
    {
        let (hitboxes, listener) = (hitboxes.clone(), listener.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase.bubble() {
                listener(
                    HeaderEvent::Move {
                        hovered: hitboxes.cell(window),
                        boundary: hitboxes.band(window),
                        at: e.position,
                    },
                    window,
                    cx,
                );
            }
        });
    }
    {
        let listener = listener.clone();
        window.on_mouse_event(move |e: &MouseExitEvent, phase, window, cx| {
            if phase.bubble() {
                listener(
                    HeaderEvent::Move {
                        hovered: None,
                        boundary: None,
                        at: e.position,
                    },
                    window,
                    cx,
                );
            }
        });
    }
    {
        let (hitboxes, listener) = (hitboxes.clone(), listener.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            if e.button == MouseButton::Right {
                if hitboxes.band(window).is_some() || hitboxes.cell(window).is_some() {
                    listener(HeaderEvent::ContextMenu { at: e.position }, window, cx);
                }
                return;
            }
            if e.button != MouseButton::Left {
                return;
            }
            let ev = if let Some(pos) = hitboxes.band(window) {
                HeaderEvent::BoundaryDown {
                    pos,
                    at: e.position,
                    click_count: e.click_count,
                }
            } else if let Some(pos) = hitboxes.cell(window) {
                HeaderEvent::Down {
                    pos,
                    at: e.position,
                }
            } else {
                return;
            };
            listener(ev, window, cx);
        });
    }
    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
        if phase.bubble() && e.button == MouseButton::Left {
            listener(
                HeaderEvent::Up {
                    hovered: hitboxes.cell(window),
                    at: e.position,
                },
                window,
                cx,
            );
        }
    });
}

/// True when `to` is more than [`DRAG_SLOP`] × ui_scale from `from`.
pub fn beyond_slop(from: Point<Pixels>, to: Point<Pixels>, ui_scale: f32) -> bool {
    let dx = (to.x - from.x).as_f32();
    let dy = (to.y - from.y).as_f32();
    let slop = DRAG_SLOP * ui_scale;
    dx * dx + dy * dy > slop * slop
}

/// The rows of the header menu (UI-PRC-006).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeaderMenuItem {
    SortBy,
    SelectColumns,
    Hide,
    Separator,
    FitColumns,
    ResizeBehavior,
    Reset,
}

/// One header menu row as [`header_menu`] adds it to the `TlMenu`.
#[derive(Clone, PartialEq, Debug)]
pub struct HeaderMenuRow {
    pub item: HeaderMenuItem,
    /// Empty on the separator.
    pub label: SharedString,
    pub enabled: bool,
    /// The check mark of "Sort by": `col` is the sort column.
    pub checked: bool,
    /// On a disabled row: why. On an enabled "Hide This Column": "Hide <label>", which `TlMenu`
    /// cannot show yet on an enabled item.
    pub tooltip: Option<SharedString>,
}

/// The id the column actions carry. `SortByColumn` and `HideColumn` hold a `ColumnId`, so a
/// column of the system tables only has one when its key is also a process column key; the
/// table parses the key back into its own id set.
fn action_id<C: TableColumn>(id: C) -> Option<ColumnId> {
    ColumnId::from_str(id.key())
}

/// The header menu rows for a right-click on `col`, in menu order (UI-PRC-006). `fixed` drops
/// "Select Columns…", "Hide This Column" and "Reset to default".
pub fn header_menu_rows<C: TableColumn>(
    col: &ColumnSpec<C>,
    layout: &ColumnLayout<C>,
    sort_col: C,
    fixed: bool,
    cx: &App,
) -> SmallVec<[HeaderMenuRow; 7]> {
    let label_arg = [("label", col.label.as_ref().into())];
    let has_id = action_id(col.id).is_some();
    let row = |item, label: SharedString| HeaderMenuRow {
        item,
        label,
        enabled: true,
        checked: false,
        tooltip: None,
    };
    let mut rows = SmallVec::new();
    let sortable = col.sortable && has_id;
    rows.push(HeaderMenuRow {
        enabled: sortable,
        checked: sortable && col.id == sort_col,
        tooltip: (!col.sortable).then(|| SharedString::new_static(UNSORTABLE_TOOLTIP)),
        ..row(
            HeaderMenuItem::SortBy,
            tr_args(cx, "table-header-menu-sort-by", &label_arg),
        )
    });
    if !fixed {
        rows.push(row(
            HeaderMenuItem::SelectColumns,
            tr(cx, "table-header-menu-select-columns"),
        ));
        let why_not = if !col.hideable {
            Some(tr(cx, "table-tooltip-name-not-hideable"))
        } else if layout.visible_columns().len() <= 1 {
            Some(tr(cx, "table-toast-last-visible"))
        } else {
            None
        };
        rows.push(HeaderMenuRow {
            enabled: why_not.is_none() && has_id,
            tooltip: why_not
                .or_else(|| Some(tr_args(cx, "table-header-menu-hide-tooltip", &label_arg))),
            ..row(HeaderMenuItem::Hide, tr(cx, "table-header-menu-hide"))
        });
    }
    rows.push(row(HeaderMenuItem::Separator, SharedString::default()));
    rows.push(row(
        HeaderMenuItem::FitColumns,
        tr(cx, "table-header-menu-fit"),
    ));
    rows.push(row(
        HeaderMenuItem::ResizeBehavior,
        tr(cx, "table-header-menu-resize-behavior"),
    ));
    if !fixed {
        rows.push(row(
            HeaderMenuItem::Reset,
            tr(cx, "table-header-menu-reset"),
        ));
    }
    rows
}

/// The radio items of the "Resize Behavior" submenu: label, mode, and whether it is checked.
pub fn resize_behavior_rows(
    mode: ColumnResizeMode,
    cx: &App,
) -> [(SharedString, ColumnResizeMode, bool); 2] {
    [ColumnResizeMode::Proportional, ColumnResizeMode::LastColumn].map(|m| {
        let key = match m {
            ColumnResizeMode::Proportional => "table-header-menu-proportional",
            ColumnResizeMode::LastColumn => "table-header-menu-last-column",
        };
        (tr(cx, key), m, m == mode)
    })
}

/// The header context menu for `col` (UI-PRC-006): sort by it, pick columns, hide it, fit the
/// columns, choose the resize behaviour and reset the columns. 240 px wide at least. Built on
/// open only.
#[allow(
    clippy::too_many_arguments,
    reason = "the UI-PRC-006 inputs, plus the window and app the submenu and strings need"
)]
pub fn header_menu<C: TableColumn, S: TableSort<C>>(
    col: &ColumnSpec<C>,
    layout: &ColumnLayout<C>,
    sort: &S,
    fixed: bool,
    resize_mode: ColumnResizeMode,
    menu: TlMenu,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let id = action_id(col.id);
    let table = layout.table;
    let mut menu = menu.min_w(px(HEADER_MENU_MIN_W));
    for row in header_menu_rows(col, layout, sort.column(), fixed, cx) {
        if row.item == HeaderMenuItem::Separator {
            menu = menu.separator();
            continue;
        }
        if !row.enabled {
            menu = menu.item_disabled(row.label, row.tooltip);
            continue;
        }
        let action: Box<dyn Action> = match (row.item, id) {
            (HeaderMenuItem::SortBy, Some(id)) => {
                menu = menu.check_item(row.label, row.checked, Box::new(SortByColumn { id }));
                continue;
            }
            (HeaderMenuItem::ResizeBehavior, _) => {
                menu = menu.submenu(row.label, window, cx, move |sub, _, cx| {
                    resize_behavior_rows(resize_mode, cx).into_iter().fold(
                        sub,
                        |sub, (label, mode, on)| {
                            sub.radio_item(label, on, Box::new(SetColumnResize { mode }))
                        },
                    )
                });
                continue;
            }
            (HeaderMenuItem::Hide, Some(id)) => Box::new(HideColumn { id }),
            (HeaderMenuItem::SelectColumns, _) => Box::new(OpenColumnPicker { table }),
            (HeaderMenuItem::FitColumns, _) => Box::new(ResizeColumnsToFit { table }),
            (HeaderMenuItem::Reset, _) => Box::new(ResetColumns { table }),
            (HeaderMenuItem::SortBy | HeaderMenuItem::Hide | HeaderMenuItem::Separator, _) => {
                continue;
            }
        };
        menu = menu.item(row.label, action);
    }
    menu
}
