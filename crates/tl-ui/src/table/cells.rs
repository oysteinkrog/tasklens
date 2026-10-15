//! Generic cell painters shared by every table (UI-PRC-001, UI-PRC-002, UI-FND-051,
//! UI-FND-052, UI-FND-053, UI-FND-026, ADR-009).
//!
//! Every cell is a fixed-width `div` the row positions at its column's x offset, with 8 px
//! padding left and right (× ui_scale), vertically centred, clipped and on one line. The text
//! inside is a [`CellText`] element: it draws the shaped text without an extra `div` and, in
//! `prepaint`, compares the text's width with the width the cell has for it and sets the shared
//! `truncated` flag. The owner reads that flag in the next render and attaches the cell tooltip
//! only when it is set ([`attach_cell_tooltip`]), so a cell gets its tooltip from its second
//! frame on.
//!
//! | Painter | Text | Overflow |
//! |---|---|---|
//! | [`text_cell`] | `t_base`, left | end ellipsis (U+2026), full text in the tooltip |
//! | [`path_cell`] | `t_base`, left | already middle-ellipsized by the caller; clips |
//! | [`numeric_cell`] | `tnum`, right | clips at the right edge, never ellipsized; full text in the tooltip |
//! | [`glyph_cell`] | `TlValidityGlyph`, right | glyphs are short; never heat-coloured |
//!
//! [`paint_name_cell`] and [`paint_group_header_cell`] draw the generic Name cell of a tree table
//! (UI-PRC-003) and the group header band (R-105). Page-specific painters (heat colouring rules,
//! dim zeros) live in the pages.
//! Pages pass their colours through [`CellOut::background`], [`CellOut::text_color`] and
//! [`CellOut::dim`], and a numeric prefix through [`CellOut::prefix`].
//!
//! No painter formats text: texts arrive as `SharedString` from `CellTextCache`. No cell has an
//! element id; the tooltip state of a truncated cell is keyed by the id its owner passes to
//! [`attach_cell_tooltip`].

use std::cell::Cell;
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{
    App, Bounds, ContentMask, Div, Element, ElementId, Font, FontWeight, GlobalElementId,
    HighlightStyle, Hsla, ImageSource, InspectorElementId, InteractiveElement, IntoElement,
    LayoutId, ParentElement, Pixels, Point, SharedString, Style, Styled, StyledImage, StyledText,
    TextRun, TextStyleRefinement, Window, div, font, img, point, px, relative, size, svg,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::validity::Validity;

use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::validity::{glyph, glyph_allows_heat, glyph_start};
use crate::pages::sys_status::{DotStyle, LabelTone, StatusLook, cell_colors};
use crate::table::columns::ColumnKind;
use crate::table::tree_table::{
    CHEVRON, CHEVRON_HIT_W, ICON_GAP, INDENT, IconSlot, NAME_PAD, NameCell, NameIcon, RowFlags,
    STALE_OPACITY,
};
use crate::text::fonts::UI_FAMILY;
use crate::text::style::{TNUM, label_style, numeric_style, t_base};
use crate::text::untrusted::{debug_assert_clean, split_fffd_runs};
use crate::theme::{TlTheme, UiMetrics};

/// Filled by `TreeTableDelegate::cell`. The table calls `cell()` with `want_text = false`, looks
/// up `CellTextCache` with (row, col, bits, validity); only on a miss it calls `cell()` again
/// with `want_text = true` and the delegate writes `text`.
#[derive(Default)]
pub struct CellOut {
    /// Value bits: `f32::to_bits as u64`, or the raw integer.
    pub bits: u64,
    pub validity: Validity,
    /// Text cells that hold an `Arc<str>` (names, users): no formatting, no copy.
    pub arc: Option<Arc<str>>,
    /// Scratch, reused across cells; written only when `want_text`.
    pub text: String,
    pub want_text: bool,
    /// Page override (heat colouring, UI-PRC-004). Drawn under the selection, never on glyphs.
    pub background: Option<Hsla>,
    /// Page override (an exited process row's secondary text).
    pub text_color: Option<Hsla>,
    /// Text colour kept on a selected row, in place of `text_selected_secondary`.
    pub selected_text_color: Option<Hsla>,
    /// A status dot drawn before the value, in the colours of a status cell. Not drawn on glyphs.
    pub dot: Option<DotStyle>,
    /// Page override (dim zero values, PROC-041): the text draws at [`DIM_OPACITY`].
    pub dim: bool,
    /// Drawn before a numeric value in its own colour (the process table's "Σ ", UI-PRC-002).
    /// `text_selected_secondary` on a selected row. Never drawn on glyphs.
    pub prefix: Option<(SharedString, Hsla)>,
    /// The end of the text in its own colour (the PID's " +N", an Image path's " (deleted)").
    pub suffix: Option<CellSuffix>,
    /// Optional explicit cell tooltip.
    pub tooltip: Option<SharedString>,
    /// Set by the table only on the hovered row, once its tooltips can show. A delegate
    /// formats a tooltip that allocates (an age, an error) only when this is set.
    pub want_tooltip: bool,
}

/// The last `len` bytes of a cell's text, drawn in their own colour (UI-PRC-004). The rest of
/// the text keeps the cell's colour. Ignored when `len` is not a char boundary of the text.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CellSuffix {
    pub len: usize,
    pub color: Hsla,
    /// The colour on a selected row.
    pub selected_color: Hsla,
}

impl CellOut {
    /// Clears every field for the next cell. Keeps the capacity of `text`.
    pub fn reset(&mut self) {
        self.bits = 0;
        self.validity = Validity::default();
        self.arc = None;
        self.text.clear();
        self.want_text = false;
        self.background = None;
        self.text_color = None;
        self.selected_text_color = None;
        self.dot = None;
        self.dim = false;
        self.prefix = None;
        self.suffix = None;
        self.tooltip = None;
        self.want_tooltip = false;
    }
}

/// Per-row painting context. The row sets `width` to the column width before each cell.
pub struct CellCtx<'a> {
    pub m: &'a UiMetrics,
    pub theme: &'a TlTheme,
    pub selected: bool,
    pub width: Pixels,
}

impl CellCtx<'_> {
    /// `text_primary`, or `text_selected_primary` on a selected row.
    pub fn primary(&self) -> Hsla {
        if self.selected {
            self.theme.text_selected_primary
        } else {
            self.theme.text_primary
        }
    }

    /// `text_secondary`, or `text_selected_secondary` on a selected row.
    pub fn secondary(&self) -> Hsla {
        if self.selected {
            self.theme.text_selected_secondary
        } else {
            self.theme.text_secondary
        }
    }
}

/// A text cell with an end ellipsis. `truncated` is set in `prepaint` when the text does not fit;
/// pass it to [`attach_cell_tooltip`] in the next render.
///
/// In debug builds, panics when `text` holds a control character (UI-FND-053 rule 1).
pub fn text_cell(text: SharedString, ctx: &CellCtx, truncated: Rc<Cell<bool>>) -> Div {
    debug_assert_clean(&text);
    cell_box(t_base(ctx.m), ctx)
        .debug_selector(|| "TlCell-text".into())
        .text_color(ctx.primary())
        .text_ellipsis()
        .child(CellText::new(
            text,
            Align::Start,
            Fit::Ellipsis,
            Some(truncated),
        ))
}

/// A path cell. `text` is already cut by `text::ellipsis::middle_ellipsis` through
/// `CellTextCache::path`; a path that still does not fit is clipped. The caller attaches the full
/// path as the tooltip when the cached text differs from the source.
pub fn path_cell(text: SharedString, ctx: &CellCtx) -> Div {
    debug_assert_clean(&text);
    cell_box(t_base(ctx.m), ctx)
        .debug_selector(|| "TlCell-path".into())
        .text_color(ctx.primary())
        .child(CellText::new(text, Align::Start, Fit::Clip, None))
}

/// A numeric cell: right-aligned with tabular figures. A Valid or Stale value that does not fit
/// starts at the left padding edge and is clipped at the right one, never ellipsized
/// (UI-FND-051 rule 4); `clipped` is then set in `prepaint`. Every other validity draws its glyph
/// through [`glyph_cell`] and clears `clipped`.
pub fn numeric_cell(
    text: SharedString,
    validity: Validity,
    clipped: Rc<Cell<bool>>,
    ctx: &CellCtx,
) -> Div {
    let out = CellOut {
        validity,
        ..CellOut::default()
    };
    value_cell(ColumnKind::Numeric, text, &out, clipped, ctx)
}

/// Every filled cell of a tree table other than Name: for a validity other than Valid and
/// Stale, a [`glyph_cell`] (numeric) or [`glyph_cell_start`] (text, path) with `clipped`
/// cleared, else the [`text_cell`], [`path_cell`] or
/// [`numeric_cell`] of `kind`. [`CellOut::prefix`] goes before the value in its own colour and
/// [`CellOut::suffix`] colours the end of the text; both use `text_selected_secondary`
/// (the prefix) or the suffix's selected colour on a selected row. The table applies
/// [`cell_text_color`] and [`cell_background`] on top.
pub fn value_cell(
    kind: ColumnKind,
    text: SharedString,
    out: &CellOut,
    clipped: Rc<Cell<bool>>,
    ctx: &CellCtx,
) -> Div {
    if !matches!(out.validity, Validity::Valid | Validity::Stale) {
        clipped.set(false);
        return match kind {
            ColumnKind::Numeric => glyph_cell(out.validity, &text, ctx),
            ColumnKind::Text | ColumnKind::Path => glyph_cell_start(out.validity, &text, ctx),
        };
    }
    let suffix = out.suffix.map(|s| SuffixRun {
        len: s.len,
        color: if ctx.selected {
            s.selected_color
        } else {
            s.color
        },
    });
    let mut cell = match kind {
        ColumnKind::Text => {
            debug_assert_clean(&text);
            cell_box(t_base(ctx.m), ctx)
                .debug_selector(|| "TlCell-text".into())
                .text_color(ctx.primary())
                .text_ellipsis()
        }
        ColumnKind::Path => {
            debug_assert_clean(&text);
            cell_box(t_base(ctx.m), ctx)
                .debug_selector(|| "TlCell-path".into())
                .text_color(ctx.primary())
        }
        ColumnKind::Numeric => {
            let color = if out.validity == Validity::Stale {
                ctx.secondary()
            } else {
                ctx.primary()
            };
            cell_box(numeric_style(ctx.m, ctx.m.text.base), ctx)
                .debug_selector(|| "TlCell-numeric".into())
                .text_color(color)
        }
    };
    if let Some(dot) = out.dot {
        let stale = out.validity == Validity::Stale;
        let look = StatusLook {
            dot,
            label: LabelTone::Primary,
        };
        if let Some(color) = cell_colors(look, ctx.selected, stale, ctx.theme).dot {
            let d = div()
                .debug_selector(|| "TlCell-dot".into())
                .flex_none()
                .size(px(8.0 * ctx.m.ui_scale))
                .rounded_full()
                .mr(ctx.m.sp[3]);
            cell = cell.child(match dot {
                DotStyle::Ring(_) => d.border(px(1.5 * ctx.m.ui_scale)).border_color(color),
                _ => d.bg(color),
            });
        }
    }
    if let Some((prefix, color)) = &out.prefix {
        let color = if ctx.selected {
            ctx.theme.text_selected_secondary
        } else {
            *color
        };
        cell = cell.child(
            div()
                .debug_selector(|| "TlCell-prefix".into())
                .flex_none()
                .text_color(color)
                .child(prefix.clone()),
        );
    }
    let text = match kind {
        ColumnKind::Text => CellText::new(text, Align::Start, Fit::Ellipsis, Some(clipped)),
        ColumnKind::Path => CellText::new(text, Align::Start, Fit::Clip, None),
        ColumnKind::Numeric => CellText::new(text, Align::End, Fit::Clip, Some(clipped)),
    };
    cell.child(text.with_suffix(suffix))
}

/// A cell showing `TlValidityGlyph`: the glyph for any validity other than Valid, right-aligned
/// like a number so a change from `…` to a value moves nothing. Never heat-coloured
/// ([`cell_background`]).
pub fn glyph_cell(validity: Validity, value_text: &SharedString, ctx: &CellCtx) -> Div {
    cell_box(numeric_style(ctx.m, ctx.m.text.base), ctx)
        .debug_selector(|| "TlCell-glyph".into())
        .child(glyph(validity, value_text, ctx.selected, ctx.m, ctx.theme))
}

/// [`glyph_cell`] for a left-aligned (text or path) column: the glyph sits at the left padding
/// edge, where the text it replaces starts.
pub fn glyph_cell_start(validity: Validity, value_text: &SharedString, ctx: &CellCtx) -> Div {
    cell_box(numeric_style(ctx.m, ctx.m.text.base), ctx)
        .debug_selector(|| "TlCell-glyph".into())
        .child(glyph_start(
            validity,
            value_text,
            ctx.selected,
            ctx.m,
            ctx.theme,
        ))
}

/// The tooltip for a cell whose text was cut: `TlTooltip::text(full)` when `truncated` is set,
/// else `None`.
pub fn cell_tooltip_if_truncated(full: SharedString, truncated: &Cell<bool>) -> Option<TlTooltip> {
    truncated.get().then(|| TlTooltip::text(full))
}

/// Attaches the 500 ms hover tooltip with `full` to `cell` when `truncated` was set by the
/// cell's last `prepaint`. `id` keys the tooltip state; it is not an element id on the cell.
/// A cell that fits gets nothing attached.
pub fn attach_cell_tooltip(
    cell: Div,
    id: impl Into<ElementId>,
    full: SharedString,
    truncated: &Cell<bool>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let Some(tooltip) = cell_tooltip_if_truncated(full, truncated) else {
        return cell;
    };
    let state = HoverTooltip::state(id, window, cx);
    HoverTooltip::attach(&state, cell, DELAY_CONTROL, tooltip.build(), window, cx)
}

/// The page background for a cell: `out.background`, except on a selected row (the selected
/// fill wins) and on glyph cells (no heat on glyphs).
pub fn cell_background(out: &CellOut, ctx: &CellCtx) -> Option<Hsla> {
    if ctx.selected || !glyph_allows_heat(out.validity) {
        return None;
    }
    out.background
}

/// Text opacity of a cell with [`CellOut::dim`] set.
pub const DIM_OPACITY: f32 = 0.3;

/// The page text colour for a cell: `out.text_color`, which becomes `text_selected_secondary`
/// on a selected row unless `out.selected_text_color` is set, at [`DIM_OPACITY`] when `out.dim` is set (from the row's primary colour
/// when there is no `text_color`). Glyphs and a prefix keep their own colours.
pub fn cell_text_color(out: &CellOut, ctx: &CellCtx) -> Option<Hsla> {
    let selected_color = out.selected_text_color.filter(|_| ctx.selected);
    let color = selected_color.or_else(|| {
        out.text_color.map(|color| {
            if ctx.selected {
                ctx.theme.text_selected_secondary
            } else {
                color
            }
        })
    });
    if out.dim {
        return Some(color.unwrap_or_else(|| ctx.primary()).opacity(DIM_OPACITY));
    }
    color
}

/// Applies [`cell_background`] and [`cell_text_color`] to a cell built by a painter.
pub fn with_overrides(mut cell: Div, out: &CellOut, ctx: &CellCtx) -> Div {
    if let Some(background) = cell_background(out, ctx) {
        cell = cell.bg(background);
    }
    if let Some(color) = cell_text_color(out, ctx) {
        cell = cell.text_color(color);
    }
    cell
}

/// The box every cell shares: column width, 8 px padding, vertically centred, clipped, one line.
fn cell_box(text_style: TextStyleRefinement, ctx: &CellCtx) -> Div {
    let mut cell = div();
    *cell.text_style() = text_style;
    cell.w(ctx.width)
        .h_full()
        .flex()
        .items_center()
        .px(ctx.m.sp[4])
        .overflow_hidden()
        .whitespace_nowrap()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Start,
    End,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// GPUI's end ellipsis, from the cell's `text_ellipsis()`.
    Ellipsis,
    /// The whole text is shaped and clipped at the content edge.
    Clip,
}

/// The end of a [`CellText`] in its own colour, already chosen for the row's selection.
#[derive(Clone, Copy)]
struct SuffixRun {
    len: usize,
    color: Hsla,
}

/// One line of cell text that fills the cell's content width. It positions the shaped text
/// itself (no wrapper `div`), clips it to the content box and reports whether it was cut.
struct CellText {
    text: SharedString,
    inner: StyledText,
    align: Align,
    fit: Fit,
    cut: Option<Rc<Cell<bool>>>,
}

impl CellText {
    fn new(text: SharedString, align: Align, fit: Fit, cut: Option<Rc<Cell<bool>>>) -> Self {
        Self {
            inner: StyledText::new(text.clone()),
            text,
            align,
            fit,
            cut,
        }
    }

    /// Draws the last `suffix.len` bytes in `suffix.color`; the rest inherits the cell's text
    /// style. No change when the length is 0, too long or not on a char boundary.
    fn with_suffix(mut self, suffix: Option<SuffixRun>) -> Self {
        let Some(suffix) = suffix else {
            return self;
        };
        let len = self.text.len();
        let Some(start) = len.checked_sub(suffix.len) else {
            return self;
        };
        if suffix.len == 0 || !self.text.is_char_boundary(start) {
            return self;
        }
        let style = HighlightStyle {
            color: Some(suffix.color),
            ..HighlightStyle::default()
        };
        self.inner = StyledText::new(self.text.clone()).with_highlights([(start..len, style)]);
        self
    }

    /// Whether GPUI replaced the end of the text with the ellipsis. `shown_len` and
    /// `last_glyph` describe the shaped line; `available` is the content width.
    fn ellipsized(
        &self,
        shown_len: usize,
        last_glyph: Option<usize>,
        available: Pixels,
        window: &Window,
    ) -> bool {
        let len = self.text.len();
        if shown_len != len {
            return true;
        }
        // Same length: either the text is whole, or exactly 3 bytes were replaced by the 3-byte
        // "…". A cut line ends with the "…" glyph at byte len - 3. A whole line ends there only
        // when its last cluster is 3 bytes (a CJK letter, "…", an "ffi" ligature); only then is
        // the full text measured, which hits the layout cache GPUI filled in this frame.
        if len < 3 || last_glyph != Some(len - 3) {
            return false;
        }
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let run = style.to_run(len);
        window
            .text_system()
            .shape_text(self.text.clone(), font_size, &[run], None, None)
            .ok()
            .and_then(|lines| lines.first().map(|line| line.width()))
            .is_some_and(|full| full > available)
    }
}

impl IntoElement for CellText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for CellText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let (text_id, ()) = self.inner.request_layout(None, None, window, cx);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        // Shrinks beside a prefix; alone it fills the content width.
        style.min_size.width = px(0.).into();
        (window.request_layout(style, [text_id], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.inner.layout().clone();
        let lines = layout.line_layouts();
        let (shown_width, shown_len, last_glyph) = match lines.first() {
            Some(line) => {
                let line = &line.unwrapped_layout;
                let last = line
                    .runs
                    .iter()
                    .flat_map(|run| run.glyphs.iter().map(|g| g.index))
                    .max();
                (line.width, line.len, last)
            }
            None => (Pixels::ZERO, 0, None),
        };
        let available = bounds.size.width;
        let x = match self.align {
            Align::End if shown_width <= available => bounds.right() - shown_width,
            _ => bounds.left(),
        };
        let text_bounds = Bounds::new(
            point(x, bounds.top()),
            size(shown_width, layout.line_height()),
        );
        self.inner
            .prepaint(None, None, text_bounds, &mut (), window, cx);

        let Some(cut) = &self.cut else { return };
        let was_cut = match self.fit {
            Fit::Clip => {
                debug_assert_eq!(
                    shown_len,
                    self.text.len(),
                    "a clipped cell inherited text_ellipsis"
                );
                shown_width > available
            }
            // GPUI may also leave a text whole that is a fraction of a pixel too wide.
            Fit::Ellipsis => {
                shown_width > available || self.ellipsized(shown_len, last_glyph, available, window)
            }
        };
        cut.set(was_cut);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mask = (self.fit == Fit::Clip).then_some(ContentMask { bounds });
        window.with_content_mask(mask, |window| {
            self.inner
                .paint(None, None, bounds, &mut (), &mut (), window, cx);
        });
    }
}

/// Gap between the primary and the secondary Name text (sp4).
const NAME_SECONDARY_GAP: f32 = 8.0;
/// The secondary Name text is hidden when less than this is left for it.
const NAME_SECONDARY_MIN: f32 = 40.0;
/// Gap before each state glyph (sp2).
const NAME_GLYPH_GAP: f32 = 4.0;
/// Inset of the trailing element from the cell's right edge (sp2).
const NAME_TRAILING_INSET: f32 = 4.0;
/// Group header row: chevron x and label x (R-105, UI-PRC-002).
const GROUP_CHEVRON_X: f32 = 8.0;
const GROUP_LABEL_X: f32 = 30.0;

/// Icon size of a Name cell slot, unscaled: 24 for `Large`, 16 for `Small`.
pub fn icon_px(slot: IconSlot) -> f32 {
    match slot {
        IconSlot::Large => 24.0,
        IconSlot::Small => 16.0,
    }
}

/// x of the chevron box's left edge at `depth`, × ui_scale: `20 + depth × 20 + 6` (R-190).
pub fn name_indent(depth: u8, ui_scale: f32) -> Pixels {
    px((NAME_PAD + f32::from(depth) * INDENT + ICON_GAP) * ui_scale)
}

/// x of the guide line of tree level `k`: the chevron centre of that level, `20 + k × 20 + 14`.
fn guide_x(k: u8, ui_scale: f32) -> f32 {
    (NAME_PAD + f32::from(k) * INDENT + ICON_GAP + CHEVRON / 2.0) * ui_scale
}

/// The connector quads of one Name cell (PROC-017, UI-PRC-003), in window coordinates for a
/// cell whose top-left corner is `origin`. Lines are one physical pixel wide and snapped to the
/// physical pixel grid (`device_scale` = `window.scale_factor()`).
///
/// For each level k below `depth - 1` whose bit is set in `ancestor_continues`, a line over
/// the full row height at the chevron centre of level k. For the row's own level, at the chevron
/// centre of the parent level: a line from the row top to the centre (to the bottom unless
/// `LAST_SIBLING`) and a line from there to the chevron's left edge. Depth-0 rows have no parent
/// level and draw no elbow. With `IN_GROUP` the level-0 line runs through the group header's
/// chevron centre, x 16, instead.
pub fn guide_quads(
    depth: u8,
    flags: RowFlags,
    origin: Point<Pixels>,
    row_h: Pixels,
    ui_scale: f32,
    device_scale: f32,
) -> SmallVec<[Bounds<Pixels>; 8]> {
    let line = px(1.0 / device_scale);
    let snap = |v: f32| px((v * device_scale).round() / device_scale);
    let in_group = flags.has(RowFlags::IN_GROUP);
    let x_at = |k: u8| {
        let x = if in_group && k == 0 {
            (GROUP_CHEVRON_X + CHEVRON / 2.0) * ui_scale
        } else {
            guide_x(k, ui_scale)
        };
        snap(f32::from(origin.x) + x)
    };
    let top = origin.y;
    let mut quads = SmallVec::new();
    // The parent level is the row's own elbow, drawn below.
    for k in 0..depth.saturating_sub(1) {
        if flags.ancestor_continues_at(k) {
            quads.push(Bounds::new(point(x_at(k), top), size(line, row_h)));
        }
    }
    if depth == 0 {
        return quads;
    }
    let x = x_at(depth - 1);
    let centre = snap(f32::from(top + row_h / 2.0));
    let bottom = if flags.has(RowFlags::LAST_SIBLING) {
        centre + line
    } else {
        top + row_h
    };
    quads.push(Bounds::new(point(x, top), size(line, bottom - top)));
    let chevron_left = snap(f32::from(origin.x + name_indent(depth, ui_scale)));
    if chevron_left > x {
        quads.push(Bounds::new(point(x, centre), size(chevron_left - x, line)));
    }
    quads
}

/// First strong isolate and pop directional isolate (UAX #9).
const FSI: char = '\u{2068}';
const PDI: char = '\u{2069}';

/// The text and runs a Name cell shapes for `primary` (UI-FND-053 rule 3). With a count run
/// after a non-empty name, the name is wrapped in FSI and PDI (from `cache`, built once) and the
/// runs grow to cover the two marks: FSI joins the first run, PDI the run that ends where the
/// count starts (PDI sits before the name's trailing whitespace, which that run holds). Without one, `primary` and [`primary_runs`] as they are.
pub fn name_primary_text(
    primary: &SharedString,
    highlight_fffd: bool,
    count_run: Option<&Range<usize>>,
    normal: Hsla,
    warning: Hsla,
    cache: &mut IconSourceCache,
) -> (SharedString, Option<SmallVec<[TextRun; 4]>>) {
    let runs = primary_runs(primary, highlight_fffd, count_run, normal, warning);
    let count_start = count_run
        .filter(|r| r.start > 0 && r.start < r.end && r.end <= primary.len())
        .map(|r| r.start);
    let Some(start) = count_start else {
        return (primary.clone(), runs);
    };
    let Some(mut runs) = runs else {
        return (primary.clone(), None);
    };
    // `primary_runs` cuts at the count's start, so one run ends exactly there.
    let mut end = 0;
    let Some(before) = runs.iter().position(|r| {
        end += r.len;
        end == start
    }) else {
        return (primary.clone(), Some(runs));
    };
    runs[0].len += FSI.len_utf8();
    runs[before].len += PDI.len_utf8();
    (cache.isolated_name(primary, start), Some(runs))
}

/// Text runs of a Name cell's primary text: U+FFFD runs in `warning` when `highlight_fffd`, the
/// live count run with `tnum`. `None` when one plain run does (the common case).
fn primary_runs(
    text: &str,
    highlight_fffd: bool,
    count_run: Option<&Range<usize>>,
    normal: Hsla,
    warning: Hsla,
) -> Option<SmallVec<[TextRun; 4]>> {
    let fffd = highlight_fffd && text.contains(char::REPLACEMENT_CHARACTER);
    let count = count_run.filter(|r| r.start < r.end && r.end <= text.len());
    if !fffd && count.is_none() {
        return None;
    }
    let base = font(UI_FAMILY);
    let tnum = Font {
        features: TNUM.clone(),
        ..base.clone()
    };
    let mut runs = SmallVec::new();
    let pieces = if fffd {
        split_fffd_runs(text)
    } else {
        smallvec::smallvec![(0..text.len(), false)]
    };
    for (range, is_fffd) in pieces {
        let color = if is_fffd { warning } else { normal };
        // Cut each piece at the count run's edges.
        let mut start = range.start;
        let mut cuts: SmallVec<[usize; 3]> = SmallVec::new();
        if let Some(c) = count {
            cuts.extend(
                [c.start, c.end]
                    .into_iter()
                    .filter(|&b| b > start && b < range.end),
            );
        }
        cuts.push(range.end);
        for end in cuts {
            let in_count = count.is_some_and(|c| start >= c.start && end <= c.end);
            runs.push(TextRun {
                len: end - start,
                font: if in_count { tnum.clone() } else { base.clone() },
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            start = end;
        }
    }
    Some(runs)
}

/// The image sources of one table's file icons, by icon path, and the bidi-isolated texts of its
/// Name cells, by primary text. `img()` takes its source by value, so without this cache every
/// app row would build a new `Arc<Path>` each frame; the same holds for the isolated name
/// string. An entry is built on first use and cloned (one `Arc` count) after that.
///
/// Each entry remembers the last table version that used it. When the version changes, entries
/// the ending version did not use are dropped, so entries of rows that are gone do not pile up.
#[derive(Default)]
pub struct IconSourceCache {
    map: FxHashMap<Arc<str>, (ImageSource, u64)>,
    /// Primary text to (isolated text, count start, last version).
    names: FxHashMap<SharedString, (SharedString, usize, u64)>,
    version: u64,
    /// Image sources built since the table was created.
    #[cfg(any(test, feature = "test-hooks"))]
    pub built: u32,
    /// Isolated name texts built since the table was created.
    #[cfg(any(test, feature = "test-hooks"))]
    pub names_built: u32,
}

impl IconSourceCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts table version `version`. Drops every entry the previous version did not use.
    pub fn begin_version(&mut self, version: u64) {
        if version == self.version {
            return;
        }
        let ending = self.version;
        self.map.retain(|_, (_, used)| *used == ending);
        self.names.retain(|_, (_, _, used)| *used == ending);
        self.version = version;
    }

    /// The image source for the icon at `path`, built once and reused.
    pub fn get(&mut self, path: &Arc<str>) -> ImageSource {
        let version = self.version;
        if let Some((source, used)) = self.map.get_mut(&**path) {
            *used = version;
            return source.clone();
        }
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.built += 1;
        }
        let source = ImageSource::from(Arc::<Path>::from(Path::new(&**path)));
        self.map.insert(path.clone(), (source.clone(), version));
        source
    }

    /// `text` with its name part `..count_start` wrapped in FSI (U+2068) and PDI (U+2069), built
    /// once and reused (UI-FND-053 rule 3). Inside the isolate the name keeps its own direction;
    /// outside it the cell has no strong character before the count, so the paragraph is LTR and
    /// the count stays on the right of an RTL name. The isolate ends before the name's trailing
    /// whitespace: inside an RTL isolate that space would draw at the name's left edge.
    pub fn isolated_name(&mut self, text: &SharedString, count_start: usize) -> SharedString {
        let version = self.version;
        if let Some((isolated, start, used)) = self.names.get_mut(text)
            && *start == count_start
        {
            *used = version;
            return isolated.clone();
        }
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.names_built += 1;
        }
        let mut isolated = String::with_capacity(text.len() + 2 * FSI.len_utf8());
        isolated.push(FSI);
        let name_end = text[..count_start].trim_end().len();
        isolated.push_str(&text[..name_end]);
        isolated.push(PDI);
        isolated.push_str(&text[name_end..]);
        let isolated = SharedString::from(isolated);
        self.names
            .insert(text.clone(), (isolated.clone(), count_start, version));
        isolated
    }

    /// Icon paths held now.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The primary text colour and the icon opacity of a Name cell: `text_secondary` (or
/// `text_selected_secondary`) and [`STALE_OPACITY`] on an `EXITED` row (UI-PRC-003),
/// `text_secondary` at full opacity on a `SECONDARY_NAME` row, else `text_primary` (or the
/// corresponding selected colour) and 1. The chevron is the same on all three.
pub fn name_look(flags: RowFlags, ctx: &CellCtx) -> (Hsla, f32) {
    if flags.has(RowFlags::EXITED) {
        (ctx.secondary(), STALE_OPACITY)
    } else if flags.has(RowFlags::SECONDARY_NAME) {
        (ctx.secondary(), 1.0)
    } else {
        (ctx.primary(), 1.0)
    }
}

/// The Name cell (UI-PRC-003, R-190): the 16 px chevron box (always reserved) at
/// `20 + depth × 20 + 6`, gap 6, the icon (16 or 24) and gap 6 when there is one, then the
/// primary text, the secondary text, the state glyphs and the trailing element. The connector
/// lines are not part of the cell: `TlTreeTable` paints them with the row's column separators
/// ([`guide_quads`]).
///
/// `window` measures the primary text, and only when there is a secondary text. `tips` is
/// `Some(key)` on the hovered row: the state glyphs then get their tooltips, keyed by `key`.
/// File icons take their image source from `icons`, and a name with a count its bidi-isolated
/// text ([`name_primary_text`]). `weight` is the primary text's weight (normal for every row
/// but a few, such as a process's main thread at 500).
#[allow(clippy::too_many_arguments)]
pub fn paint_name_cell(
    cell: NameCell,
    weight: FontWeight,
    depth: u8,
    expandable: Option<bool>,
    flags: RowFlags,
    ctx: &CellCtx,
    tips: Option<u64>,
    icons: &mut IconSourceCache,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let m = ctx.m;
    let s = m.ui_scale;
    let indent = name_indent(depth, s);
    let mut root = div();
    *root.text_style() = t_base(m);
    root = root
        .debug_selector(|| "TlNameCell".into())
        .relative()
        .w(ctx.width)
        .h_full()
        .overflow_hidden()
        .whitespace_nowrap();

    // The chevron's hover area is 20 wide, centred on its 16 px box.
    let hit_extra = px((CHEVRON_HIT_W - CHEVRON) / 2.0 * s);
    let mut chevron_slot = div()
        .group("tl-name-chevron")
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h_full()
        .w(px(CHEVRON_HIT_W * s))
        .ml(-hit_extra)
        .mr(-hit_extra);
    if let Some(expanded) = expandable {
        let name = if expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        chevron_slot = chevron_slot.child(
            svg()
                .path(name.path())
                .flex_none()
                .size(px(CHEVRON * s))
                .text_color(ctx.secondary())
                .group_hover("tl-name-chevron", |st| st.text_color(ctx.primary())),
        );
    }

    let mut content = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .pl(indent)
        .pr(m.sp[4])
        .child(chevron_slot);

    let gap = px(ICON_GAP * s);
    let icon_w = px(icon_px(cell.icon_size) * s);
    let icon_color = ctx.secondary();
    let (primary_color, icon_opacity) = name_look(flags, ctx);
    let has_icon = !matches!(cell.icon, NameIcon::None);
    match cell.icon {
        NameIcon::File(path) => {
            let fallback_color = ctx.theme.text_secondary;
            content = content.child(
                img(icons.get(&path))
                    .flex_none()
                    .ml(gap)
                    .size(icon_w)
                    .opacity(icon_opacity)
                    .with_fallback(move || {
                        svg()
                            .path(IconName::Binary.path())
                            .size(icon_w)
                            .text_color(fallback_color)
                            .into_any_element()
                    }),
            );
        }
        NameIcon::Lucide(name) => {
            content = content.child(
                svg()
                    .path(name.path())
                    .flex_none()
                    .ml(gap)
                    .size(icon_w)
                    .opacity(icon_opacity)
                    .text_color(icon_color),
            );
        }
        NameIcon::None => {}
    }

    let (shaped, runs) = name_primary_text(
        &cell.primary,
        cell.highlight_fffd,
        cell.count_run.as_ref(),
        primary_color,
        ctx.theme.status.warning,
        icons,
    );
    let text = match runs {
        Some(mut runs) => {
            for run in &mut runs {
                run.font.weight = weight;
            }
            StyledText::new(shaped).with_runs(runs.into_vec())
        }
        None => StyledText::new(shaped),
    };
    content = content.child(
        div()
            .debug_selector(|| "TlNameCell-primary".into())
            .ml(gap)
            .min_w_0()
            .flex_shrink(1.)
            .overflow_hidden()
            .text_ellipsis()
            .text_color(primary_color)
            .font_weight(weight)
            .child(text),
    );

    if let Some(secondary) = cell.secondary {
        let font_size = m.text.base;
        let run = TextRun {
            len: cell.primary.len(),
            font: Font {
                weight,
                ..font(UI_FAMILY)
            },
            color: primary_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let primary_w = window
            .text_system()
            .shape_line(cell.primary.clone(), font_size, &[run], None)
            .width;
        let icon_room = if has_icon { icon_w + gap } else { px(0.) };
        let text_x = indent + px(CHEVRON * s) + gap + icon_room;
        let room = ctx.width - text_x - m.sp[4];
        if room >= primary_w + px((NAME_SECONDARY_GAP + NAME_SECONDARY_MIN) * s) {
            content = content.child(
                div()
                    .debug_selector(|| "TlNameCell-secondary".into())
                    .ml(px(NAME_SECONDARY_GAP * s))
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(ctx.secondary())
                    .child(secondary),
            );
        }
    }

    for (i, (name, tip)) in cell.glyphs.into_iter().enumerate() {
        let mut glyph = div().flex_none().ml(px(NAME_GLYPH_GAP * s)).child(
            svg()
                .path(name.path())
                .size(m.icon.xs)
                .text_color(ctx.secondary()),
        );
        if let Some(key) = tips {
            let id = ElementId::NamedInteger(
                SharedString::new_static("tl-name-glyph"),
                key.wrapping_add(i as u64),
            );
            let state = HoverTooltip::state(id, window, cx);
            glyph = HoverTooltip::attach(
                &state,
                glyph,
                DELAY_CONTROL,
                TlTooltip::text(tip).build(),
                window,
                cx,
            );
        }
        content = content.child(glyph);
    }

    root = root.child(content);
    if let Some(trailing) = cell.trailing {
        root = root.child(
            div()
                .absolute()
                .top_0()
                .right(px(NAME_TRAILING_INSET * s))
                .h_full()
                .flex()
                .items_center()
                .child(trailing),
        );
    }
    root
}

/// The Name cell of a group header row (R-105, UI-PRC-002): the chevron at x 8 in
/// `text_primary` and the label at x 30 in `t_md` 14 weight 600. `ctx.width` is the width the
/// label may use, normally the rest of the row.
pub fn paint_group_header_cell(cell: NameCell, expanded: Option<bool>, ctx: &CellCtx) -> Div {
    let m = ctx.m;
    let s = m.ui_scale;
    let mut root = div()
        .debug_selector(|| "TlGroupHeaderCell".into())
        .relative()
        .w(ctx.width)
        .h_full()
        .overflow_hidden()
        .whitespace_nowrap();
    if let Some(expanded) = expanded {
        let name = if expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        root = root.child(
            div()
                .absolute()
                .top_0()
                .left(px(GROUP_CHEVRON_X * s))
                .h_full()
                .flex()
                .items_center()
                .child(
                    svg()
                        .path(name.path())
                        .size(px(CHEVRON * s))
                        .text_color(ctx.theme.text_primary),
                ),
        );
    }
    let mut text = div();
    *text.text_style() = label_style(m, m.text.md, FontWeight::SEMIBOLD);
    let mut row = div()
        .absolute()
        .top_0()
        .left(px(GROUP_LABEL_X * s))
        .right(m.sp[4])
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .child(
            text.debug_selector(|| "TlGroupHeaderCell-label".into())
                .min_w_0()
                .flex_shrink(1.)
                .text_ellipsis()
                .text_color(ctx.theme.text_primary)
                .child(cell.primary),
        );
    for (name, _tip) in cell.glyphs {
        row = row.child(
            svg()
                .debug_selector(|| "TlGroupHeaderCell-glyph".into())
                .path(name.path())
                .size(m.icon.sm)
                .flex_none()
                .ml(m.sp[3])
                .text_color(ctx.theme.status.warning),
        );
    }
    if let Some(secondary) = cell.secondary {
        row = row.child(
            div()
                .debug_selector(|| "TlGroupHeaderCell-secondary".into())
                .ml(m.sp[3])
                .min_w_0()
                .flex_shrink(1.)
                .text_ellipsis()
                .text_color(ctx.theme.text_secondary)
                .font_weight(FontWeight::NORMAL)
                .child(secondary),
        );
    }
    root.child(row)
}

#[cfg(test)]
mod tests {
    use gpui_kit::px;
    use tl_model::settings::AppearanceSettings;

    use super::*;
    use crate::theme::ThemeMode;
    use crate::theme::metrics::compute;
    use crate::theme::tests::test_theme;

    #[test]
    fn reset_clears_every_field() {
        let mut out = CellOut {
            bits: 7,
            validity: Validity::Valid,
            arc: Some("bash".into()),
            text: String::with_capacity(64),
            want_text: true,
            background: Some(gpui_kit::red()),
            text_color: Some(gpui_kit::red()),
            selected_text_color: Some(gpui_kit::red()),
            dot: Some(DotStyle::Filled(LabelTone::Danger)),
            dim: true,
            prefix: Some(("Σ ".into(), gpui_kit::red())),
            suffix: Some(CellSuffix {
                len: 3,
                color: gpui_kit::red(),
                selected_color: gpui_kit::red(),
            }),
            tooltip: Some("tip".into()),
            want_tooltip: true,
        };
        out.text.push_str("12.5%");
        out.reset();
        assert_eq!(out.bits, 0);
        assert_eq!(out.validity, Validity::WarmingUp);
        assert!(out.arc.is_none() && out.background.is_none() && out.text_color.is_none());
        assert!(out.selected_text_color.is_none() && out.dot.is_none());
        assert!(out.tooltip.is_none() && !out.want_text && !out.want_tooltip);
        assert!(!out.dim && out.prefix.is_none() && out.suffix.is_none());
        assert!(out.text.is_empty());
        assert!(out.text.capacity() >= 64, "the scratch keeps its capacity");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "U+0007")]
    fn control_char_debug_assert() {
        let m = compute(&AppearanceSettings::default(), 0);
        let theme = test_theme(ThemeMode::Dark);
        let ctx = CellCtx {
            m: &m,
            theme: &theme,
            selected: false,
            width: px(120.),
        };
        let _ = text_cell("bell\u{7}".into(), &ctx, Rc::default());
    }
}
