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
//! Resize and reorder (E18-010, E18-011) extend [`HeaderState`].

use std::cell::Cell;
use std::hash::BuildHasher as _;
use std::rc::Rc;

use gpui_kit::assets::IconNamed as _;
use gpui_kit::{
    App, Bounds, Div, ElementId, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement,
    MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    Pixels, Point, SharedString, Styled, TextRun, TransformationMatrix, Window, canvas, div, fill,
    font, point, prelude::FluentBuilder, px, size,
};
use rustc_hash::{FxBuildHasher, FxHashMap};
use smallvec::SmallVec;
use tl_model::validity::Validity;

use crate::components::icons::TlIcon;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::table::cells::{CellCtx, glyph_cell, numeric_cell};
use crate::table::columns::{ColumnAlign, ColumnLayout, ColumnSpec, TableColumn};
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
/// A press that moves more than this is a drag, not a click (reorder is E18-011).
pub const DRAG_SLOP: f32 = 4.0;
/// Header tooltip of every unsortable column (UI-PRC-005).
pub const UNSORTABLE_TOOLTIP: &str = "Only measured for visible rows";

/// What the header's hitboxes report to the table. `pos` is a visible column position.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum HeaderEvent {
    /// The pointer moved; `hovered` is the header cell under it, if any.
    Move {
        hovered: Option<usize>,
        at: Point<Pixels>,
    },
    /// Left press on a header cell.
    Down { pos: usize, at: Point<Pixels> },
    /// Left release anywhere in the window; `hovered` as for `Move`.
    Up {
        hovered: Option<usize>,
        at: Point<Pixels>,
    },
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
        let arrow_color = self.theme.text_primary;
        // Copied out so the closures own them; inline for up to 24 columns.
        let cells: SmallVec<[(Pixels, Pixels, Option<Hsla>); 24]> = self
            .state
            .cells
            .iter()
            .map(|c| (c.x, c.w, c.fill))
            .collect();
        let arrow = self.state.arrow.map(|(_, asc, b)| (asc, b));
        let listener = self.listener.clone();

        canvas(
            {
                let cells = cells.clone();
                move |bounds: Bounds<Pixels>, window: &mut Window, _: &mut App| {
                    let hitboxes: SmallVec<[Hitbox; 24]> = cells
                        .iter()
                        .map(|&(x, w, _)| {
                            let b = Bounds::new(
                                point(bounds.left() + x - x_offset, bounds.top()),
                                size(w, bounds.size.height),
                            );
                            window.insert_hitbox(b, HitboxBehavior::Normal)
                        })
                        .collect();
                    Rc::new(hitboxes)
                }
            },
            move |bounds, hitboxes, window, cx| {
                let left = bounds.left() - x_offset;
                for &(x, w, fill_color) in &cells {
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
                    window.paint_quad(fill(line, divider));
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

/// Reports pointer moves, left presses on a cell and left releases to `listener`.
fn register_listeners(
    hitboxes: Rc<SmallVec<[Hitbox; 24]>>,
    listener: HeaderListener,
    window: &mut Window,
) {
    let hovered =
        |hitboxes: &[Hitbox], window: &Window| hitboxes.iter().position(|h| h.is_hovered(window));
    {
        let (hitboxes, listener) = (hitboxes.clone(), listener.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase.bubble() {
                let hovered = hovered(&hitboxes, window);
                listener(
                    HeaderEvent::Move {
                        hovered,
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
            if phase.bubble()
                && e.button == MouseButton::Left
                && let Some(pos) = hovered(&hitboxes, window)
            {
                listener(
                    HeaderEvent::Down {
                        pos,
                        at: e.position,
                    },
                    window,
                    cx,
                );
            }
        });
    }
    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
        if phase.bubble() && e.button == MouseButton::Left {
            let hovered = hovered(&hitboxes, window);
            listener(
                HeaderEvent::Up {
                    hovered,
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
